//! Unified HTTP Telemetry Server providing Prometheus metrics scraping
//! and Kubernetes liveness (/healthz) and readiness (/ready) probing.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use thiserror::Error;

static RECORDER_HANDLE: OnceLock<Result<PrometheusHandle, String>> = OnceLock::new();

/// Telemetry initialization or runtime errors.
#[derive(Error, Debug)]
pub enum TelemetryError {
    #[error("Failed to initialize Prometheus recorder: {0}")]
    RecorderInit(String),
    #[error("Failed to bind telemetry HTTP server: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
struct AppState {
    handle: PrometheusHandle,
    ready: Arc<AtomicBool>,
    cert_hash: Arc<RwLock<Option<String>>>,
}

/// Unified administrative HTTP server hosting `/metrics`, `/healthz`, `/ready`, and `/cert-hash`.
#[derive(Clone)]
pub struct TelemetryServer {
    handle: PrometheusHandle,
    ready: Arc<AtomicBool>,
    cert_hash: Arc<RwLock<Option<String>>>,
}

impl TelemetryServer {
    /// Initializes or retrieves the singleton `TelemetryServer` with a global Prometheus recorder.
    ///
    /// Thread-safe and re-entrant: caches the `PrometheusHandle` across parallel unit tests
    /// to avoid `SetRecorderError` collisions.
    pub fn init() -> Result<Self, TelemetryError> {
        let handle_res = RECORDER_HANDLE.get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .map_err(|e| e.to_string())
        });

        let handle = handle_res
            .as_ref()
            .map_err(|e| TelemetryError::RecorderInit(e.clone()))?
            .clone();

        Ok(Self {
            handle,
            ready: Arc::new(AtomicBool::new(false)),
            cert_hash: Arc::new(RwLock::new(None)),
        })
    }

    /// Sets the active SHA-256 certificate fingerprint exposed via `GET /cert-hash`.
    pub fn set_cert_hash(&self, hash: impl Into<String>) {
        if let Ok(mut lock) = self.cert_hash.write() {
            *lock = Some(hash.into());
        }
    }

    /// Returns the currently configured certificate hash, if set.
    pub fn cert_hash(&self) -> Option<String> {
        self.cert_hash.read().ok().and_then(|lock| lock.clone())
    }

    /// Marks the service as ready to serve traffic (`/ready` returns 200).
    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
    }

    /// Marks the service as unready (`/ready` returns 503).
    pub fn mark_unready(&self) {
        self.ready.store(false, Ordering::SeqCst);
    }

    /// Returns `true` if the service is currently marked ready.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Renders current Prometheus metrics as text exposition.
    pub fn render_metrics(&self) -> String {
        self.handle.render()
    }

    /// Builds the `axum::Router` hosting `/metrics`, `/healthz`, `/ready`, and `/cert-hash`.
    pub fn router(&self) -> Router {
        let state = AppState {
            handle: self.handle.clone(),
            ready: Arc::clone(&self.ready),
            cert_hash: Arc::clone(&self.cert_hash),
        };

        Router::new()
            .route("/metrics", get(metrics_handler))
            .route("/healthz", get(healthz_handler))
            .route("/ready", get(ready_handler))
            .route("/cert-hash", get(cert_hash_handler))
            .with_state(state)
    }

    /// Spawns the background HTTP admin server listening on `addr`.
    pub fn spawn(&self, addr: SocketAddr) -> tokio::task::JoinHandle<()> {
        let router = self.router();
        tokio::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!(
                        "[Telemetry] Failed to bind admin HTTP listener on {}: {}",
                        addr, e
                    );
                    return;
                }
            };
            if let Err(e) = axum::serve(listener, router).await {
                eprintln!("[Telemetry] Admin HTTP server error: {}", e);
            }
        })
    }
}

async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.handle.render(),
    )
}

async fn healthz_handler() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

async fn ready_handler(State(state): State<AppState>) -> impl IntoResponse {
    if state.ready.load(Ordering::SeqCst) {
        (StatusCode::OK, "READY")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "NOT_READY")
    }
}

async fn cert_hash_handler(State(state): State<AppState>) -> impl IntoResponse {
    let hash_opt = state.cert_hash.read().ok().and_then(|lock| lock.clone());
    match hash_opt {
        Some(hash) => (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, "application/json"),
                (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            ],
            serde_json::to_string(&serde_json::json!({ "cert_hash": hash })).unwrap_or_default(),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            [
                (axum::http::header::CONTENT_TYPE, "application/json"),
                (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            ],
            serde_json::to_string(
                &serde_json::json!({ "error": "Certificate hash not configured" }),
            )
            .unwrap_or_default(),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_telemetry_cert_hash_endpoint() {
        let server = TelemetryServer::init().unwrap();
        let router = server.router();

        // 1. Initial GET /cert-hash without set_cert_hash should return 404 NOT_FOUND
        let req = Request::builder()
            .uri("/cert-hash")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "*"
        );

        // 2. Set certificate hash and verify 200 OK + JSON body + CORS
        let expected_hash = "12:34:56:78:90:ab:cd:ef";
        server.set_cert_hash(expected_hash);
        assert_eq!(server.cert_hash(), Some(expected_hash.to_string()));

        let req = Request::builder()
            .uri("/cert-hash")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "*"
        );

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["cert_hash"], expected_hash);
    }

    #[tokio::test]
    async fn test_telemetry_healthz_and_ready_transitions() {
        let server = TelemetryServer::init().unwrap();
        assert!(!server.is_ready());

        let router = server.router();

        // 1. Healthz should return 200 OK
        let req = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 2. Ready should initially return 503 Service Unavailable
        let req = Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        // 3. Mark ready -> Ready should return 200 OK
        server.mark_ready();
        assert!(server.is_ready());
        let req = Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 4. Mark unready -> Ready should return 503 again
        server.mark_unready();
        assert!(!server.is_ready());
        let req = Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_telemetry_metrics_render() {
        let server = TelemetryServer::init().unwrap();

        // Increment a test counter
        metrics::counter!("mmo_test_sample_counter_total").increment(42);

        let router = server.router();
        let req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("mmo_test_sample_counter_total"));
        assert!(body.contains("42"));
    }
}
