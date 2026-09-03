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
    alert::AlertNotifier,
    app_config::AgentConfig,
    audit_log,
    auth::{verify_request, AuthConfig, AuthError, NonceStore},
    power::{run_power_action, PowerAction, PowerResult},
};
use pc_remote_signing::PowerAction as SharedPowerAction;

#[derive(Clone)]
pub struct AppState {
    auth: Arc<AuthConfig>,
    nonces: Arc<NonceStore>,
    dry_run: bool,
    /// 認証失敗アラートの送信先。config未設定なら None(通知しないだけ)。
    alert: Option<Arc<AlertNotifier>>,
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
    let alert = AlertNotifier::from_config(&config).map(Arc::new);
    let state = AppState {
        auth: Arc::new(AuthConfig {
            secret: config.shared_secret.into_bytes(),
            allowed_skew_seconds: config.allowed_skew_seconds,
        }),
        nonces: Arc::new(NonceStore::default()),
        dry_run: config.dry_run,
        alert,
    };

    let mut router = Router::new().route("/status", get(status));
    for action in SharedPowerAction::ALL {
        router = match action {
            SharedPowerAction::Reboot => router.route(action.path(), post(reboot)),
            SharedPowerAction::Shutdown => router.route(action.path(), post(shutdown)),
        };
    }
    router.layer(DefaultBodyLimit::max(128)).with_state(state)
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

/// `command`のblockingパートでの失敗。どちらも500を返すが、`Audit`は
/// 電源操作を実行していないことを意味する。
enum CommandFailure {
    Audit(std::io::Error),
    Power(anyhow::Error),
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

            // 監査ログ(open+writeln+sync_all)も電源操作(shutdown.exeの起動)も
            // blocking I/O。asyncワーカースレッドを塞がないよう、まとめて
            // blockingスレッドへ逃がす。fail-closed(監査ログを残せないなら
            // 電源操作を実行しない)の順序はこのクロージャ内で維持している。
            let dry_run = state.dry_run;
            let outcome =
                tokio::task::spawn_blocking(move || -> Result<PowerResult, CommandFailure> {
                    audit_log::append(action.slug(), dry_run, "accepted")
                        .map_err(CommandFailure::Audit)?;

                    let result = run_power_action(action, dry_run);
                    let label = if result.is_ok() { "ok" } else { "failed" };
                    if let Err(err) = audit_log::append(action.slug(), dry_run, label) {
                        tracing::error!("failed to write audit log result: {err}");
                    }
                    result.map_err(CommandFailure::Power)
                })
                .await;

            match outcome {
                Ok(Ok(result)) => (StatusCode::OK, Json(result)).into_response(),
                Ok(Err(CommandFailure::Audit(err))) => {
                    tracing::error!("failed to write audit log: {err}");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to write audit log",
                    )
                        .into_response()
                }
                Ok(Err(CommandFailure::Power(err))) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("power command failed: {err}"),
                )
                    .into_response(),
                Err(join_err) => {
                    // blockingタスク自体が落ちた場合、電源操作まで到達したか
                    // 判別できない。監査ログを見て判断する。
                    tracing::error!("power command task failed: {join_err}");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "power command task failed",
                    )
                        .into_response()
                }
            }
        }
        Err(err) => {
            tracing::warn!("power command authentication failed: {err}");
            if let Some(alert) = state.alert.as_ref() {
                alert.record_auth_failure();
            }
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
