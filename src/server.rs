use std::{io, sync::Arc};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{
        HeaderValue, Method, StatusCode,
        header::{CACHE_CONTROL, CONTENT_LENGTH},
    },
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio::{net::TcpListener, sync::Semaphore, time::timeout};
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::read_tools::ReadTools;
use crate::tines::TinesClient;
use crate::tools::contract::{BODY_DEADLINE, LocalLimits, MAX_CONCURRENT_POST_BODIES};

#[derive(Clone)]
struct Ingress {
    limits: LocalLimits,
    post_slots: Arc<Semaphore>,
    cancellation: CancellationToken,
}

pub fn router(config: &Config, cancellation_token: CancellationToken) -> Router {
    router_with_client(config, None, cancellation_token)
}

pub fn router_with_client(
    config: &Config,
    tines: Option<TinesClient>,
    cancellation_token: CancellationToken,
) -> Router {
    let ingress = Ingress {
        limits: config.local_limits,
        post_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_POST_BODIES)),
        cancellation: cancellation_token.clone(),
    };
    let server_config = StreamableHttpServerConfig::default()
        .with_max_request_body_bytes(config.local_limits.max_request_bytes())
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(config.allowed_hosts.clone())
        .with_allowed_origins(config.allowed_origins.clone())
        .enforce_origin_validation()
        .with_cancellation_token(cancellation_token);
    let limits = config.local_limits;
    let mcp = StreamableHttpService::new(
        move || Ok(ReadTools::with_limits(tines.clone(), limits)),
        LocalSessionManager::default().into(),
        server_config,
    );

    Router::new()
        .route("/healthz", get(healthz))
        .nest_service("/mcp", mcp)
        .layer(from_fn_with_state(ingress, limit_mcp_request_body))
}

async fn limit_mcp_request_body(
    State(ingress): State<Ingress>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path() != "/mcp" && !request.uri().path().starts_with("/mcp/") {
        return next.run(request).await;
    }

    let response = if request.method() == Method::POST {
        // No queue and no body polling before admission. Keep the permit through
        // dispatch so parsed requests cannot accumulate behind tool execution.
        let Ok(_permit) = ingress.post_slots.try_acquire() else {
            return no_store(StatusCode::SERVICE_UNAVAILABLE.into_response());
        };
        let limit = ingress.limits.max_request_bytes();
        if request
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length > limit as u64)
        {
            return no_store(StatusCode::PAYLOAD_TOO_LARGE.into_response());
        }
        let (parts, body) = request.into_parts();
        let body = tokio::select! {
            biased;
            _ = ingress.cancellation.cancelled() => {
                return no_store(StatusCode::SERVICE_UNAVAILABLE.into_response());
            }
            result = timeout(BODY_DEADLINE, to_bytes(body, limit)) => match result {
                Err(_) => return no_store(StatusCode::REQUEST_TIMEOUT.into_response()),
                Ok(Err(_)) => return no_store(StatusCode::PAYLOAD_TOO_LARGE.into_response()),
                Ok(Ok(body)) => body,
            }
        };
        next.run(Request::from_parts(parts, Body::from(body))).await
    } else {
        // SSE GETs never acquire a POST permit or buffer a request body.
        next.run(request).await
    };
    no_store(response)
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub async fn run(
    listener: TcpListener,
    config: &Config,
    cancellation_token: CancellationToken,
) -> io::Result<()> {
    let tines = config
        .tines
        .as_ref()
        .map(TinesClient::new)
        .transpose()
        .map_err(io::Error::other)?;
    let app = router_with_client(config, tines, cancellation_token.clone());
    axum::serve(listener, app)
        .with_graceful_shutdown(cancellation_token.cancelled_owned())
        .await
}

async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{Notify, mpsc};
    use tower::ServiceExt;

    #[tokio::test]
    async fn long_lived_gets_do_not_consume_post_admission() {
        let release = Arc::new(Notify::new());
        let (entered_tx, mut entered_rx) = mpsc::channel(8);
        let handler_release = release.clone();
        let app = Router::new()
            .route(
                "/mcp",
                get(move || {
                    let entered = entered_tx.clone();
                    let release = handler_release.clone();
                    async move {
                        entered.send(()).await.unwrap();
                        release.notified().await;
                        StatusCode::NO_CONTENT
                    }
                })
                .post(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(from_fn_with_state(
                Ingress {
                    limits: LocalLimits::default(),
                    post_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_POST_BODIES)),
                    cancellation: CancellationToken::new(),
                },
                limit_mcp_request_body,
            ));
        let mut gets = Vec::new();
        for _ in 0..8 {
            gets.push(tokio::spawn(app.clone().oneshot(
                Request::builder().uri("/mcp").body(Body::empty()).unwrap(),
            )));
        }
        for _ in 0..8 {
            timeout(std::time::Duration::from_secs(1), entered_rx.recv())
                .await
                .unwrap()
                .unwrap();
        }
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        release.notify_waiters();
        for get in gets {
            assert_eq!(get.await.unwrap().unwrap().status(), StatusCode::NO_CONTENT);
        }
    }
}
