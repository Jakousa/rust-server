//! Unified HTTP Telemetry Server providing Prometheus metrics scraping
//! and Kubernetes liveness (/healthz) and readiness (/ready) probing.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use thiserror::Error;

static RECORDER_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

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
}

/// Unified administrative HTTP server hosting `/metrics`, `/healthz`, and `/ready`.
#[derive(Clone)]
pub struct TelemetryServer {
    handle: PrometheusHandle,
    ready: Arc<AtomicBool>,
}

impl TelemetryServer {
    /// Initializes or retrieves the singleton `TelemetryServer` with a global Prometheus recorder.
    ///
    /// Thread-safe and re-entrant: caches the `PrometheusHandle` across parallel unit tests
    /// to avoid `SetRecorderError` collisions.
    pub fn init() -> Result<Self, TelemetryError> {
        let handle = RECORDER_HANDLE.get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("Failed to install Prometheus recorder")
        });

        Ok(Self {
            handle: handle.clone(),
            ready: Arc::new(AtomicBool::new(false)),
        })
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

    /// Builds the `axum::Router` hosting `/metrics`, `/healthz`, and `/ready`.
    pub fn router(&self) -> Router {
        let state = AppState {
            handle: self.handle.clone(),
            ready: Arc::clone(&self.ready),
        };

        Router::new()
            .route("/metrics", get(metrics_handler))
            .route("/healthz", get(healthz_handler))
            .route("/ready", get(ready_handler))
            .with_state(state)
    }

    /// Spawns the background HTTP admin server listening on `addr`.
    pub fn spawn(&self, addr: SocketAddr) -> tokio::task::JoinHandle<()> {
        let router = self.router();
        tokio::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[Telemetry] Failed to bind admin HTTP listener on {}: {}", addr, e);
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

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
