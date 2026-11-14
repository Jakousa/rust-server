//! Shared TLS certificate loading, dynamic SAN collection, and WebSocket TLS setup.
//!
//! Provides unified Subject Alternative Name (SAN) resolution across WebTransport and
//! WebSocket TLS listeners, optional external PEM certificate loading, and self-signed
//! fallback generation.

use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::quic::generate_self_signed_cert;
use crate::websocket::WebSocketError;

/// Environment variable specifying the PEM certificate chain file path for WebSocket TLS (`wss://`).
pub const ENV_WS_TLS_CERT_PATH: &str = "WS_TLS_CERT_PATH";
/// Environment variable specifying the PEM private key file path for WebSocket TLS (`wss://`).
pub const ENV_WS_TLS_KEY_PATH: &str = "WS_TLS_KEY_PATH";
/// Environment variable specifying comma-separated extra SAN entries for WebSocket TLS certificates.
pub const ENV_WS_TLS_ALT_NAMES: &str = "WS_TLS_ALT_NAMES";
/// Environment variable specifying comma-separated extra SAN entries for WebTransport certificates.
pub const ENV_WEBTRANSPORT_ALT_NAMES: &str = "WEBTRANSPORT_ALT_NAMES";

const DEFAULT_TLS_ALT_NAMES: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
const HOST_IP_ENV_KEYS: [&str; 2] = ["HOST_IP", "PUBLIC_IP"];

fn push_unique_san(alt_names: &mut Vec<String>, candidate: &str) {
    let trimmed = candidate.trim();
    if !trimmed.is_empty() && !alt_names.iter().any(|existing| existing == trimmed) {
        alt_names.push(trimmed.to_string());
    }
}

/// Collects Subject Alternative Names (SANs) from process environment variables.
///
/// Always includes `localhost`, `127.0.0.1`, and `::1`, appends non-empty `HOST_IP` and
/// `PUBLIC_IP` values, and expands comma-separated entries from `extra_env_keys`.
pub fn collect_tls_alt_names(extra_env_keys: &[&str]) -> Vec<String> {
    collect_tls_alt_names_from(extra_env_keys, |key| std::env::var(key).ok())
}

/// Collects deduplicated Subject Alternative Names (SANs) using a custom environment lookup.
///
/// Always seeds `["localhost", "127.0.0.1", "::1"]`, checks `HOST_IP` and `PUBLIC_IP`, and
/// splits comma-separated entries for each key in `extra_env_keys`.
pub fn collect_tls_alt_names_from<F>(extra_env_keys: &[&str], mut get_env: F) -> Vec<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut alt_names: Vec<String> = DEFAULT_TLS_ALT_NAMES
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    for env_key in HOST_IP_ENV_KEYS {
        if let Some(val) = get_env(env_key) {
            push_unique_san(&mut alt_names, &val);
        }
    }

    for env_key in extra_env_keys {
        if let Some(extra_names) = get_env(env_key) {
            for name in extra_names.split(',') {
                push_unique_san(&mut alt_names, name);
            }
        }
    }

    alt_names
}

/// Loads an X.509 certificate chain and private key from PEM files.
pub fn load_pem_certs_and_key(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), WebSocketError> {
    let cert_iter = CertificateDer::pem_file_iter(cert_path).map_err(|err| {
        WebSocketError::Tls(format!(
            "Failed to read TLS certificate PEM {}: {err}",
            cert_path.display()
        ))
    })?;
    let mut certs = Vec::new();
    for item in cert_iter {
        let cert = item.map_err(|err| {
            WebSocketError::Tls(format!(
                "Failed to parse TLS certificate PEM {}: {err}",
                cert_path.display()
            ))
        })?;
        certs.push(cert);
    }
    if certs.is_empty() {
        return Err(WebSocketError::Tls(format!(
            "No X.509 certificates found in PEM file: {}",
            cert_path.display()
        )));
    }

    let key = PrivateKeyDer::from_pem_file(key_path).map_err(|err| {
        WebSocketError::Tls(format!(
            "Failed to load TLS private key PEM {}: {err}",
            key_path.display()
        ))
    })?;

    Ok((certs, key))
}

fn non_empty_env<F>(get_env: &mut F, key: &str) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    get_env(key).and_then(|val| {
        let trimmed = val.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Builds a `rustls::ServerConfig` for WebSocket TLS (`wss://`) from process environment variables.
///
/// If both `WS_TLS_CERT_PATH` and `WS_TLS_KEY_PATH` are set, loads the external PEM certificate
/// chain and private key. If neither is set, generates a self-signed certificate populated with
/// dynamic SANs from `HOST_IP`, `PUBLIC_IP`, `WS_TLS_ALT_NAMES`, and `WEBTRANSPORT_ALT_NAMES`.
pub fn build_ws_tls_server_config() -> Result<Arc<rustls::ServerConfig>, WebSocketError> {
    build_ws_tls_server_config_from_env(|key| std::env::var(key).ok())
}

/// Builds a `rustls::ServerConfig` for WebSocket TLS (`wss://`) using a custom environment lookup.
///
/// Rejects partial configuration where only one of `WS_TLS_CERT_PATH` or `WS_TLS_KEY_PATH`
/// is supplied.
pub fn build_ws_tls_server_config_from_env<F>(
    mut get_env: F,
) -> Result<Arc<rustls::ServerConfig>, WebSocketError>
where
    F: FnMut(&str) -> Option<String>,
{
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cert_path = non_empty_env(&mut get_env, ENV_WS_TLS_CERT_PATH);
    let key_path = non_empty_env(&mut get_env, ENV_WS_TLS_KEY_PATH);

    let (certs, key) = match (cert_path, key_path) {
        (Some(cert), Some(key)) => load_pem_certs_and_key(Path::new(&cert), Path::new(&key))?,
        (Some(_), None) | (None, Some(_)) => {
            return Err(WebSocketError::Tls(format!(
                "Both {ENV_WS_TLS_CERT_PATH} and {ENV_WS_TLS_KEY_PATH} must be set together"
            )));
        }
        (None, None) => {
            let alt_names = collect_tls_alt_names_from(
                &[ENV_WS_TLS_ALT_NAMES, ENV_WEBTRANSPORT_ALT_NAMES],
                get_env,
            );
            let alt_name_refs: Vec<&str> = alt_names.iter().map(String::as_str).collect();
            generate_self_signed_cert(&alt_name_refs)
                .map_err(|err| WebSocketError::Tls(err.to_string()))?
        }
    };

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|err| WebSocketError::Tls(err.to_string()))?;

    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR_ID: AtomicU64 = AtomicU64::new(1);

    struct TempTestDir(PathBuf);

    impl TempTestDir {
        fn new(prefix: &str) -> Self {
            let id = NEXT_TEST_DIR_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("{prefix}-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_test_pem_pair(dir: &Path, alt_names: Vec<String>) -> (PathBuf, PathBuf) {
        let certified = rcgen::generate_simple_self_signed(alt_names).unwrap();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, certified.cert.pem()).unwrap();
        std::fs::write(&key_path, certified.key_pair.serialize_pem()).unwrap();
        (cert_path, key_path)
    }

    #[test]
    fn test_collect_tls_alt_names_from_defaults_and_dedup() {
        let env_map: HashMap<&str, &str> = HashMap::from([
            ("HOST_IP", " 10.0.1.5 "),
            ("PUBLIC_IP", "203.0.113.10"),
            (
                "WS_TLS_ALT_NAMES",
                "gateway.example.com, localhost, , 10.0.1.5, edge-2.local",
            ),
            ("WEBTRANSPORT_ALT_NAMES", "wt.example.com, edge-2.local"),
        ]);

        let sans =
            collect_tls_alt_names_from(&[ENV_WS_TLS_ALT_NAMES, ENV_WEBTRANSPORT_ALT_NAMES], |k| {
                env_map.get(k).map(|v| (*v).to_string())
            });

        assert_eq!(
            sans,
            vec![
                "localhost",
                "127.0.0.1",
                "::1",
                "10.0.1.5",
                "203.0.113.10",
                "gateway.example.com",
                "edge-2.local",
                "wt.example.com",
            ]
        );
    }

    #[test]
    fn test_load_pem_certs_and_key_valid_and_error_cases() {
        let tmp = TempTestDir::new("mmo-gw-tls");
        let (cert_path, key_path) =
            write_test_pem_pair(tmp.path(), vec!["gateway.test.local".to_string()]);

        let (certs, _key) = load_pem_certs_and_key(&cert_path, &key_path).unwrap();
        assert_eq!(certs.len(), 1);

        // Missing certificate file returns structured Tls error
        let missing = tmp.path().join("does-not-exist.pem");
        let err = load_pem_certs_and_key(&missing, &key_path).unwrap_err();
        assert!(matches!(err, WebSocketError::Tls(_)));

        // Empty certificate PEM returns error
        let empty_cert = tmp.path().join("empty.pem");
        std::fs::write(&empty_cert, "").unwrap();
        let err = load_pem_certs_and_key(&empty_cert, &key_path).unwrap_err();
        assert!(err.to_string().contains("No X.509 certificates found"));

        // Invalid private key PEM returns error
        let bad_key = tmp.path().join("bad-key.pem");
        std::fs::write(&bad_key, "not a pem key").unwrap();
        let err = load_pem_certs_and_key(&cert_path, &bad_key).unwrap_err();
        assert!(matches!(err, WebSocketError::Tls(_)));
    }

    #[test]
    fn test_build_ws_tls_server_config_from_env_pem_partial_and_fallback() {
        let tmp = TempTestDir::new("mmo-gw-tls");
        let (cert_path, key_path) = write_test_pem_pair(tmp.path(), vec!["wss.prod.local".into()]);

        // Both PEM env vars set -> loads PEM into ServerConfig
        let cert_str = cert_path.to_string_lossy().into_owned();
        let key_str = key_path.to_string_lossy().into_owned();
        let pem_env: HashMap<&str, &str> = HashMap::from([
            (ENV_WS_TLS_CERT_PATH, cert_str.as_str()),
            (ENV_WS_TLS_KEY_PATH, key_str.as_str()),
        ]);
        let cfg = build_ws_tls_server_config_from_env(|k| pem_env.get(k).map(|v| (*v).to_string()));
        assert!(cfg.is_ok());

        // Partial env vars -> explicit error
        let partial_env: HashMap<&str, &str> =
            HashMap::from([(ENV_WS_TLS_CERT_PATH, cert_str.as_str())]);
        let err =
            build_ws_tls_server_config_from_env(|k| partial_env.get(k).map(|v| (*v).to_string()))
                .unwrap_err();
        assert!(err.to_string().contains("must be set together"));

        // Neither set (or empty strings) -> self-signed fallback with dynamic SANs
        let fallback_env: HashMap<&str, &str> = HashMap::from([
            (ENV_WS_TLS_CERT_PATH, "  "),
            ("PUBLIC_IP", "198.51.100.42"),
            (ENV_WS_TLS_ALT_NAMES, "ws.fallback.local"),
        ]);
        let fallback_cfg =
            build_ws_tls_server_config_from_env(|k| fallback_env.get(k).map(|v| (*v).to_string()));
        assert!(fallback_cfg.is_ok());
    }
}
