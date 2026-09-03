use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::net::TcpListener;

use crate::{
    app_config::AgentConfig,
    audit_log,
    auth::{verify_request, AuthConfig, AuthError, NonceStore},
    power::{run_power_action, PowerAction},
};

#[derive(Clone)]
pub struct AppState {
    auth: Arc<AuthConfig>,
    nonces: Arc<NonceStore>,
    dry_run: bool,
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    agent_online: bool,
    agent: &'static str,
    status: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandRequest {
    confirm: bool,
}

pub fn router(config: AgentConfig) -> Router {
    let state = AppState {
        auth: Arc::new(AuthConfig {
            secret: config.shared_secret.into_bytes(),
            allowed_skew_seconds: config.allowed_skew_seconds,
        }),
        nonces: Arc::new(NonceStore::default()),
        dry_run: config.dry_run,
    };

    Router::new()
        .route("/status", get(status))
        .route("/reboot", post(reboot))
        .route("/shutdown", post(shutdown))
        .layer(DefaultBodyLimit::max(128))
        .with_state(state)
}

pub async fn serve(config: AgentConfig) -> anyhow::Result<()> {
    serve_with_shutdown(config, std::future::pending()).await
}

/// `shutdown` が完了すると、進行中のリクエストを終えてから止まる(graceful shutdown)。
/// Windows Serviceとして動く場合、SCMのSTOP制御をこの`shutdown`へつなぐ。
pub async fn serve_with_shutdown(
    config: AgentConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let addr: SocketAddr = config.bind.parse()?;
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, router(config))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

async fn status() -> Json<StatusResponse> {
    Json(StatusResponse {
        agent_online: true,
        agent: "m5stack-pc-bridge",
        status: "ok",
    })
}

async fn reboot(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    command(state, method, uri, headers, body, PowerAction::Reboot).await
}

async fn shutdown(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    command(state, method, uri, headers, body, PowerAction::Shutdown).await
}

async fn command(
    state: AppState,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
    action: PowerAction,
) -> Response {
    match verify_headers(&state, &method, &uri, &headers, &body) {
        Ok(()) => {
            let Ok(request) = serde_json::from_slice::<CommandRequest>(&body) else {
                return (StatusCode::BAD_REQUEST, "request body must be valid JSON")
                    .into_response();
            };
            if !request.confirm {
                return (StatusCode::BAD_REQUEST, "confirm must be true").into_response();
            }

            // fail-closed: 監査ログを残せない場合は電源操作そのものを実行しない。
            if let Err(err) = audit_log::append(action.as_str(), state.dry_run, "accepted") {
                tracing::error!("failed to write audit log: {err}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to write audit log",
                )
                    .into_response();
            }

            match run_power_action(action, state.dry_run) {
                Ok(result) => {
                    if let Err(err) = audit_log::append(action.as_str(), state.dry_run, "ok") {
                        tracing::error!("failed to write audit log result: {err}");
                    }
                    (StatusCode::OK, Json(result)).into_response()
                }
                Err(err) => {
                    if let Err(log_err) =
                        audit_log::append(action.as_str(), state.dry_run, "failed")
                    {
                        tracing::error!("failed to write audit log result: {log_err}");
                    }
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("power command failed: {err}"),
                    )
                        .into_response()
                }
            }
        }
        Err(err) => {
            tracing::warn!("power command authentication failed: {err}");
            (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
        }
    }
}

fn verify_headers(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), AuthError> {
    let timestamp = header_str(headers, "x-timestamp")?
        .parse::<i64>()
        .map_err(|_| AuthError::MissingHeader)?;
    let nonce = header_str(headers, "x-nonce")?;
    let signature = header_str(headers, "x-signature")?;

    verify_request(
        &state.auth,
        &state.nonces,
        method.as_str(),
        uri.path(),
        timestamp,
        nonce,
        body,
        signature,
        OffsetDateTime::now_utc(),
    )
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, AuthError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or(AuthError::MissingHeader)
}
