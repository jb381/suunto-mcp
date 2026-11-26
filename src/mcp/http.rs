use super::{
    MAX_REQUEST_BYTES, PROTOCOLS, dispatch, is_initialize, negotiated_version, rpc_error,
    sessions::{Outgoing, Sessions, Transport},
};
use crate::{App, stores};
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{OriginalUri, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{convert::Infallible, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use uuid::Uuid;

const SESSION: &str = "mcp-session-id";
const VERSION: &str = "mcp-protocol-version";
const SSE: &str = "text/event-stream";
const JSON: &str = "application/json";
const CONCURRENT_WORK: usize = 4;

#[derive(Clone)]
struct Http {
    app: Arc<App>,
    sessions: Arc<Sessions>,
    work: Arc<Semaphore>,
    bearer_hash: Option<[u8; 32]>,
}

pub fn router(app: Arc<App>, legacy_sse: bool) -> Router {
    let state = Http {
        sessions: Sessions::new(app.config.max_sessions, app.config.session_ttl),
        work: Arc::new(Semaphore::new(CONCURRENT_WORK)),
        bearer_hash: (!app.config.bearer.is_empty())
            .then(|| Sha256::digest(&app.config.bearer).into()),
        app,
    };
    let mcp = if legacy_sse {
        Router::new()
            .route("/sse", get(open_sse))
            .route("/messages", post(sse_message))
            .route("/messages/", post(sse_message))
    } else {
        Router::new()
            .route("/mcp", post(streamable).delete(delete_session))
            .route("/mcp/", post(streamable).delete(delete_session))
    }
    .route_layer(middleware::from_fn_with_state(state.clone(), authorize));

    let surface = stores::WEBHOOK_ENDPOINTS.iter().fold(
        mcp.route("/health", get(health)),
        |router, (endpoint, _)| router.route(endpoint, post(webhook)),
    );
    surface.with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({"status":"ok"}))
}

async fn authorize(State(state): State<Http>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|header| header.to_str().ok())
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.as_str())
        });
    if let Some(origin) = headers.get(header::ORIGIN) {
        let origin_authority = origin
            .to_str()
            .ok()
            .and_then(|origin| origin.parse::<axum::http::Uri>().ok())
            .filter(|url| matches!(url.scheme_str(), Some("https" | "http")))
            .and_then(|url| url.authority().map(|value| value.as_str().to_owned()));
        if !matches!((origin_authority.as_deref(), host),
            (Some(origin), Some(host)) if origin.eq_ignore_ascii_case(host))
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    if let Some(expected) = state.bearer_hash {
        let authorized = headers
            .get(header::AUTHORIZATION)
            .and_then(|header| header.to_str().ok())
            .and_then(|value| value.split_once(' '))
            .is_some_and(|(scheme, token)| {
                let supplied: [u8; 32] = Sha256::digest(token).into();
                scheme.eq_ignore_ascii_case("bearer") && bool::from(expected.ct_eq(&supplied))
            });
        if !authorized {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer")],
            )
                .into_response();
        }
    } else if !host.is_some_and(loopback_host) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

fn loopback_host(host: &str) -> bool {
    host.parse::<axum::http::uri::Authority>()
        .is_ok_and(|authority| {
            let host = authority.host();
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        })
}

fn protocol_error(status: StatusCode, code: i32, message: &str) -> Response {
    (status, Json(rpc_error(&Value::Null, code, message))).into_response()
}

fn busy() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, "1")],
    )
        .into_response()
}

fn accepts(headers: &HeaderMap, kind: &str) -> bool {
    let accept = headers.get_all(header::ACCEPT);
    accept.iter().any(|header| {
        header.to_str().is_ok_and(|value| {
            value.split(',').any(|part| {
                matches!(part.split(';').next().map(str::trim), Some(candidate)
            if candidate.eq_ignore_ascii_case(kind) || candidate == "*/*")
            })
        })
    })
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(JSON))
}

fn version(headers: &HeaderMap) -> Option<Option<&str>> {
    match headers.get(VERSION) {
        Some(header) => header
            .to_str()
            .ok()
            .filter(|value| PROTOCOLS.contains(value))
            .map(Some),
        None => Some(None),
    }
}

fn session_id(headers: &HeaderMap) -> Option<Uuid> {
    headers.get(SESSION)?.to_str().ok()?.parse().ok()
}

async fn read_body(state: &Http, body: Body) -> Result<(Bytes, OwnedSemaphorePermit), StatusCode> {
    let permit = state
        .work
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let body = tokio::time::timeout(Duration::from_secs(10), to_bytes(body, MAX_REQUEST_BYTES))
        .await
        .map_err(|_| StatusCode::REQUEST_TIMEOUT)?
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    Ok((body, permit))
}

fn body_error(status: StatusCode) -> Response {
    if status == StatusCode::SERVICE_UNAVAILABLE {
        busy()
    } else {
        status.into_response()
    }
}

struct ResponseBytes {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}

impl AsRef<[u8]> for ResponseBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

fn rpc_response(response: Value, sse: bool, permit: OwnedSemaphorePermit) -> Response {
    let text = response.to_string();
    let bytes = if sse {
        format!("event: message\ndata: {text}\n\n").into_bytes()
    } else {
        text.into_bytes()
    };
    let body = Body::from(Bytes::from_owner(ResponseBytes {
        bytes,
        _permit: permit,
    }));
    ([(header::CONTENT_TYPE, if sse { SSE } else { JSON })], body).into_response()
}

async fn streamable(State(state): State<Http>, headers: HeaderMap, body: Body) -> Response {
    if !is_json(&headers) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let sse = !accepts(&headers, JSON);
    if sse && !accepts(&headers, SSE) {
        return StatusCode::NOT_ACCEPTABLE.into_response();
    }
    let Some(request_version) = version(&headers) else {
        return protocol_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "Unsupported MCP-Protocol-Version",
        );
    };
    let (body, permit) = match read_body(&state, body).await {
        Ok(body) => body,
        Err(status) => return body_error(status),
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(_) => return protocol_error(StatusCode::BAD_REQUEST, -32700, "Parse error"),
    };
    if is_initialize(&message) && !headers.contains_key(SESSION) {
        let version = negotiated_version(&message["params"]);
        let response = dispatch(&state.app, message).await.unwrap();
        if response.get("error").is_some() {
            return (StatusCode::BAD_REQUEST, Json(response)).into_response();
        }
        let Some((id, _)) = state.sessions.insert(Transport::Http { version }) else {
            return busy();
        };
        let mut response = rpc_response(response, sse, permit);
        response
            .headers_mut()
            .insert(SESSION, HeaderValue::from_str(&id.to_string()).unwrap());
        return response;
    }
    if !headers.contains_key(SESSION) {
        return protocol_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "Mcp-Session-Id is required",
        );
    }
    let Some(lease) = session_id(&headers).and_then(|id| state.sessions.lease(id)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Transport::Http { version } = lease.transport else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if request_version.is_some_and(|value| value != version) || is_initialize(&message) {
        return protocol_error(StatusCode::BAD_REQUEST, -32600, "Invalid session request");
    }
    let response = dispatch(&state.app, message).await;
    drop(lease);
    match response {
        Some(response) => rpc_response(response, sse, permit),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

async fn delete_session(State(state): State<Http>, headers: HeaderMap) -> Response {
    let Some(request_version) = version(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !headers.contains_key(SESSION) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some((id, lease)) =
        session_id(&headers).and_then(|id| state.sessions.lease(id).map(|lease| (id, lease)))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match lease.transport {
        Transport::Http { version } if request_version.is_none_or(|value| value == version) => {
            state.sessions.remove(id);
            StatusCode::OK.into_response()
        }
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}

struct CloseSse {
    id: Uuid,
    sessions: Arc<Sessions>,
}

impl Drop for CloseSse {
    fn drop(&mut self) {
        self.sessions.remove(self.id);
    }
}

async fn open_sse(State(state): State<Http>) -> Response {
    let (sender, mut receiver) = mpsc::channel(1);
    let Some((id, cancelled)) = state.sessions.insert(Transport::Sse(sender)) else {
        return busy();
    };
    let guard = CloseSse {
        id,
        sessions: state.sessions,
    };
    let stream = async_stream::stream! {
        let _guard = guard;
        let endpoint = format!("/messages/?session_id={id}");
        yield Ok::<_, Infallible>(Event::default().event("endpoint").data(endpoint));
        loop {
            tokio::select! {
                biased;
                () = cancelled.cancelled() => break,
                output = receiver.recv() => {
                    let Some(Outgoing { response, work }) = output else { break };
                    yield Ok(Event::default().event("message").data(response.to_string()));
                    drop(work);
                }
            }
        }
    };
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[derive(Deserialize)]
struct SessionQuery {
    session_id: Uuid,
}

async fn sse_message(
    State(state): State<Http>,
    Query(query): Query<SessionQuery>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !is_json(&headers) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let (body, permit) = match read_body(&state, body).await {
        Ok(body) => body,
        Err(status) => return body_error(status),
    };
    let Some(lease) = state.sessions.lease(query.session_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Transport::Sse(ref sender) = lease.transport else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let queue_slot = match sender.try_reserve() {
        Ok(permit) => permit,
        Err(mpsc::error::TrySendError::Full(())) => return busy(),
        Err(mpsc::error::TrySendError::Closed(())) => return StatusCode::NOT_FOUND.into_response(),
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(_) => return protocol_error(StatusCode::BAD_REQUEST, -32700, "Parse error"),
    };
    if let Some(response) = dispatch(&state.app, message).await {
        queue_slot.send(Outgoing {
            response,
            work: permit,
        });
    }
    StatusCode::ACCEPTED.into_response()
}

async fn webhook(
    State(state): State<Http>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let (body, _permit) = match read_body(&state, body).await {
        Ok(body) => body,
        Err(status) => return body_error(status),
    };
    let (status, result) = stores::receive_webhook(&state.app, uri.path(), &headers, &body).await;
    (status, Json(result)).into_response()
}
