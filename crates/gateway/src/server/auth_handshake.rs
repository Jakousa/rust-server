use std::sync::Arc;

use mmo_core::guid::Guid;
use mmo_schema::client::{auth_error_codes, AuthResponsePacket};
use mmo_storage::cache::{SessionCacheBackend, SessionData};

use super::{GatewayError, GatewayInterceptHandler};
use crate::auth::EdgeAuthenticator;
use crate::router::SessionRouter;

/// Result of a successful client authentication and initial session registration.
#[derive(Debug, Clone)]
pub struct AuthenticatedSession {
    pub guid: Guid,
    pub assigned_node: String,
    pub auth_response: AuthResponsePacket,
}

/// Resolves the destination simulation node address for a player during initial authentication or handover.
pub async fn resolve_initial_node(
    router: &SessionRouter,
    cache: Option<&Arc<dyn SessionCacheBackend>>,
    default_node: Option<&str>,
    guid: Guid,
) -> Result<String, GatewayError> {
    if let Some(route) = router.get_route_by_guid(guid) {
        return Ok(route);
    }

    if let Some(existing_session_id) = router.get_session_by_guid(guid) {
        if let Some(addr) = router.get_route_by_session(existing_session_id) {
            return Ok(addr);
        }
        if let Some(c) = cache {
            if let Some(data) = c
                .get_session(existing_session_id)
                .await
                .map_err(GatewayError::Storage)?
            {
                let addr = data.node_address.clone();
                router.register_session(existing_session_id, guid, addr.clone());
                return Ok(addr);
            }
        }
    }

    if let Some(default_addr) = default_node {
        return Ok(default_addr.to_string());
    }

    Err(GatewayError::NoRouteFound {
        session_id: 0,
        guid: Some(guid),
    })
}

/// Authenticates a client token, resolves the authoritative initial node, registers the session in
/// the router and distributed cache, and subscribes to default social topics ("chat:global", "whisper:{guid}").
///
/// Shared across TCP, QUIC, and WebTransport transport handshakes for unified authentication behavior.
#[allow(clippy::result_large_err)]
pub async fn authenticate_and_register_session(
    authenticator: &EdgeAuthenticator,
    token: &str,
    session_id: u64,
    router: &SessionRouter,
    default_node_address: Option<&str>,
    cache: Option<&Arc<dyn SessionCacheBackend>>,
    intercept_handler: Option<&GatewayInterceptHandler>,
) -> Result<AuthenticatedSession, (GatewayError, AuthResponsePacket)> {
    let auth_res = match authenticator.authenticate(token) {
        Ok(res) => res,
        Err(e) => {
            let err_resp = AuthResponsePacket {
                success: false,
                error_code: e.to_error_code(),
                session_id: 0,
                player_guid: 0,
                assigned_node: String::new(),
                error_message: e.to_string(),
            };
            return Err((GatewayError::Auth(e), err_resp));
        }
    };

    let initial_node =
        match resolve_initial_node(router, cache, default_node_address, auth_res.guid).await {
            Ok(node) => node,
            Err(_) => {
                let err_resp = AuthResponsePacket {
                    success: false,
                    error_code: auth_error_codes::CLUSTER_UNAVAILABLE,
                    session_id: 0,
                    player_guid: 0,
                    assigned_node: String::new(),
                    error_message: "Cluster unavailable".to_string(),
                };
                return Err((
                    GatewayError::NoRouteFound {
                        session_id,
                        guid: Some(auth_res.guid),
                    },
                    err_resp,
                ));
            }
        };

    router.register_session(session_id, auth_res.guid, initial_node.clone());

    if let Some(c) = cache {
        let cache_clone = (*c).clone();
        let node_clone = initial_node.clone();
        let session_data = SessionData {
            session_id,
            guid: auth_res.guid.raw(),
            node_address: node_clone,
            expires_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + 3600,
        };
        tokio::spawn(async move {
            let _ = cache_clone.set_session(&session_data, 3600).await;
        });
    }

    if let Some(handler) = intercept_handler {
        handler
            .subscribe_session_topic(session_id, "chat:global".to_string())
            .await;
        handler
            .subscribe_session_topic(session_id, format!("whisper:{}", auth_res.guid.raw()))
            .await;
    }

    Ok(AuthenticatedSession {
        guid: auth_res.guid,
        assigned_node: initial_node.clone(),
        auth_response: AuthResponsePacket {
            success: true,
            error_code: auth_error_codes::SUCCESS,
            session_id,
            player_guid: auth_res.guid.raw(),
            assigned_node: initial_node,
            error_message: String::new(),
        },
    })
}
