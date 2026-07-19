use std::net::SocketAddr;

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use crate::{
    Application,
    error::CoreError,
    protocol::{PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope},
};

#[derive(Clone)]
struct HttpState {
    app: Application,
    shutdown: Option<ShutdownSignal>,
}

#[derive(Clone)]
struct ShutdownSignal(std::sync::Arc<std::sync::Mutex<Option<oneshot::Sender<()>>>>);

impl ShutdownSignal {
    fn trigger(&self) {
        if let Some(sender) = self.0.lock().expect("shutdown mutex poisoned").take() {
            let _ = sender.send(());
        }
    }
}

pub fn http_router(app: Application) -> Router {
    http_router_with_shutdown(app, None)
}

fn http_router_with_shutdown(app: Application, shutdown: Option<ShutdownSignal>) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/rpc", post(rpc))
        .with_state(HttpState { app, shutdown })
}

pub async fn run_http(app: Application, bind: SocketAddr) -> anyhow::Result<()> {
    if !bind.ip().is_loopback() {
        anyhow::bail!("refusing to listen on non-loopback address {bind}");
    }
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let signal = ShutdownSignal(std::sync::Arc::new(std::sync::Mutex::new(Some(
        shutdown_tx,
    ))));
    axum::serve(listener, http_router_with_shutdown(app, Some(signal)))
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await?;
    Ok(())
}

async fn health(State(state): State<HttpState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "protocol_version": PROTOCOL_VERSION,
        "database": state.app.storage().path().display().to_string(),
    }))
}

async fn rpc(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            String::new(),
            &CoreError::Unauthorized,
        );
    };
    let actor = match state.app.authenticate(token) {
        Ok(actor) => actor,
        Err(error) => return error_response(error.status(), String::new(), &error),
    };
    let request = match serde_json::from_slice::<RequestEnvelope>(&body) {
        Ok(request) => request,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                String::new(),
                &CoreError::Validation(format!("invalid JSON request: {error}")),
            );
        }
    };
    let requests_shutdown = request.action == "core.shutdown";
    let response = state.app.execute(&actor, request).await;
    if requests_shutdown && response.ok {
        if let Some(shutdown) = &state.shutdown {
            shutdown.trigger();
        }
    }
    Json(response).into_response()
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

fn error_response(status: StatusCode, request_id: String, error: &CoreError) -> Response {
    (status, Json(ResponseEnvelope::failure(request_id, error))).into_response()
}

pub async fn run_stdio(app: Application, token: &str) -> anyhow::Result<()> {
    let actor = app.authenticate(token)?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<RequestEnvelope>(&line) {
            Ok(request) => app.execute(&actor, request).await,
            Err(error) => ResponseEnvelope::failure(
                String::new(),
                &CoreError::Validation(format!("invalid JSON Lines request: {error}")),
            ),
        };
        let mut encoded = serde_json::to_vec(&response)?;
        encoded.push(b'\n');
        stdout.write_all(&encoded).await?;
        stdout.flush().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::*;
    use crate::Storage;

    #[tokio::test]
    async fn health_is_public_but_rpc_requires_token() {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path().join("transport.db"));
        storage.migrate().unwrap();
        storage.bootstrap_admin("owner").unwrap();
        let app = Application::new(storage, None, "text-embedding-3-small".into(), 512);
        let router = http_router(app);

        let health = router
            .clone()
            .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        let rpc = router
            .oneshot(
                Request::post("/v1/rpc")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"request_id":"1","protocol_version":"1.0","action":"health"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rpc.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn malformed_authenticated_rpc_returns_protocol_error() {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path().join("malformed.db"));
        storage.migrate().unwrap();
        let admin = storage.bootstrap_admin("owner").unwrap().unwrap();
        let app = Application::new(storage, None, "text-embedding-3-small".into(), 512);
        let response = http_router(app)
            .oneshot(
                Request::post("/v1/rpc")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", admin.token))
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let envelope: ResponseEnvelope = serde_json::from_slice(&body).unwrap();
        assert!(!envelope.ok);
        assert_eq!(envelope.error.unwrap().code, "validation_error");
    }
}
