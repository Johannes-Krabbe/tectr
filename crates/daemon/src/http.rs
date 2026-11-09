//! Plain HTTP on the daemon's port, next to the websocket: the MCP proxy agents reach HTTP
//! servers through, and the address a browser comes back to after logging in to one.
//!
//! It shares the loopback listener, so it adds no port and nothing beyond loopback. A
//! connection is told apart by its first request line before the websocket handshake reads it.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::core::Core;

/// MCP requests are small JSON-RPC messages. A body this big is a mistake or an attack.
const MAX_BODY: usize = 16 * 1024 * 1024;

/// The request headers an MCP client needs the server to see. Anything else, such as cookies
/// or the agent's own Authorization, stays with the proxy.
const FORWARDED_REQUEST: [&str; 5] = [
    "accept",
    "content-type",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
];

/// The response headers an MCP client needs. `www-authenticate` stays out on purpose: an agent
/// that saw it would try to log in by itself, and logging in is the daemon's job.
const FORWARDED_RESPONSE: [&str; 4] = [
    "content-type",
    "mcp-session-id",
    "mcp-protocol-version",
    "cache-control",
];

type Body = BoxBody<Bytes, std::io::Error>;

/// The path of the first request on a connection, if it's an HTTP request for this module.
/// Waits for the whole first line without reading it, so the websocket handshake still can.
pub async fn peek_path(stream: &TcpStream, wait: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut buffer = [0u8; 8192];
    loop {
        let read = tokio::time::timeout_at(deadline, stream.peek(&mut buffer))
            .await
            .ok()?
            .ok()?;
        if read == 0 {
            return None;
        }
        if let Some(end) = buffer[..read].windows(2).position(|pair| pair == b"\r\n") {
            let line = std::str::from_utf8(&buffer[..end]).ok()?;
            let path = line.split(' ').nth(1)?;
            return (path.starts_with("/mcp/") || path.starts_with("/oauth/"))
                .then(|| path.to_owned());
        }
        if read == buffer.len() || tokio::time::Instant::now() >= deadline {
            return None;
        }
        // Peeking again returns at once with the same bytes, so give the rest time to arrive.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

pub async fn serve(core: Arc<Core>, stream: TcpStream) -> Result<()> {
    let service = hyper::service::service_fn(move |request| handle(core.clone(), request));
    hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .await?;
    Ok(())
}

async fn handle(core: Arc<Core>, request: Request<Incoming>) -> Result<Response<Body>, Infallible> {
    let path = request.uri().path().to_owned();
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let response = match segments.as_slice() {
        // Browsers send an Origin, and a web page must never reach the proxy.
        ["mcp", ..] if request.headers().contains_key("origin") => {
            text(StatusCode::FORBIDDEN, "browsers can't use the MCP proxy")
        }
        ["mcp", secret, server_id] => {
            let (secret, server_id) = (secret.to_string(), server_id.to_string());
            proxy(&core, &secret, &server_id, request).await
        }
        ["oauth", "callback"] if request.method() == Method::GET => {
            callback(&core, request.uri().query().unwrap_or("")).await
        }
        _ => text(StatusCode::NOT_FOUND, "not found"),
    };
    Ok(response)
}

fn full(body: impl Into<Bytes>) -> Body {
    Full::new(body.into())
        .map_err(|never: Infallible| match never {})
        .boxed()
}

fn text(status: StatusCode, message: &str) -> Response<Body> {
    let mut response = Response::new(full(message.to_owned()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// An answer in JSON-RPC's shape, so an agent shows the reason instead of a bare status.
fn rpc_error(status: StatusCode, message: &str) -> Response<Body> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": { "code": -32001, "message": message },
    });
    let mut response = Response::new(full(body.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response
}

async fn proxy(
    core: &Arc<Core>,
    secret: &str,
    server_id: &str,
    request: Request<Incoming>,
) -> Response<Body> {
    let Some(target) = core.mcp_proxy_target(secret, server_id) else {
        return rpc_error(
            StatusCode::NOT_FOUND,
            "this MCP server isn't available to this session",
        );
    };
    let (parts, body) = request.into_parts();
    // Kept whole, so a request can go again after a refresh.
    let body = match Limited::new(body, MAX_BODY).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return rpc_error(StatusCode::PAYLOAD_TOO_LARGE, "the request is too big"),
    };
    let Ok(method) = reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) else {
        return rpc_error(StatusCode::METHOD_NOT_ALLOWED, "unsupported method");
    };

    let mut retried = false;
    loop {
        let token = core.mcp_access_token(&target.server_id, retried).await;
        let mut upstream = core
            .http()
            .request(method.clone(), &target.url)
            .body(body.clone());
        for name in FORWARDED_REQUEST {
            if let Some(value) = parts.headers.get(name) {
                upstream = upstream.header(name, value.as_bytes());
            }
        }
        for (name, value) in &target.headers {
            upstream = upstream.header(name, value);
        }
        if let Some(token) = &token {
            upstream = upstream.header("authorization", format!("Bearer {token}"));
        }
        let response = match upstream.send().await {
            Ok(response) => response,
            Err(error) => {
                return rpc_error(
                    StatusCode::BAD_GATEWAY,
                    &format!("tectr couldn't reach the MCP server: {error}"),
                );
            }
        };
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if token.is_some() && !retried {
                retried = true;
                continue;
            }
            core.mcp_needs_login(&target.server_id);
            return rpc_error(
                StatusCode::UNAUTHORIZED,
                "this MCP server needs a login. Log in to it in tectr's MCP settings",
            );
        }
        return relay(response);
    }
}

/// Streams the server's answer back as it comes, which an event stream needs.
fn relay(response: reqwest::Response) -> Response<Body> {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers: Vec<(HeaderName, HeaderValue)> = FORWARDED_RESPONSE
        .iter()
        .filter_map(|name| {
            let value = response.headers().get(*name)?;
            Some((
                HeaderName::from_static(name),
                HeaderValue::from_bytes(value.as_bytes()).ok()?,
            ))
        })
        .collect();
    let stream = response
        .bytes_stream()
        .map(|chunk| chunk.map(Frame::data).map_err(std::io::Error::other));
    let mut relayed = Response::new(BodyExt::boxed(StreamBody::new(stream)));
    *relayed.status_mut() = status;
    relayed.headers_mut().extend(headers);
    relayed
}

async fn callback(core: &Arc<Core>, query: &str) -> Response<Body> {
    let params: Vec<(String, String)> = url_pairs(query);
    let param = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let outcome = match (param("state"), param("code"), param("error")) {
        (Some(state), _, Some(error)) => {
            core.cancel_mcp_login(state);
            let detail = param("error_description").unwrap_or(error);
            Err(format!("The server didn't log you in: {detail}"))
        }
        (Some(state), Some(code), None) => core
            .complete_mcp_login(state, code)
            .await
            .map_err(|error| error.message),
        _ => Err("This address needs the state and code the server sends back.".to_owned()),
    };
    let (status, message) = match outcome {
        Ok(name) => (
            StatusCode::OK,
            format!("Logged in to {name}. You can close this tab."),
        ),
        Err(message) => (StatusCode::BAD_REQUEST, message),
    };
    let page = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>tectr</title><p>{}</p>",
        escape_html(&message)
    );
    let mut response = Response::new(full(page));
    *response.status_mut() = status;
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
}

fn url_pairs(query: &str) -> Vec<(String, String)> {
    reqwest::Url::parse(&format!("http://x/?{query}"))
        .map(|url| url.query_pairs().into_owned().collect())
        .unwrap_or_default()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    async fn peeked(request: &[u8]) -> Option<String> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let request = request.to_vec();
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            // Split in two, so the line arrives in pieces as it can over a real connection.
            let (first, rest) = request.split_at(request.len() / 2);
            stream.write_all(first).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            stream.write_all(rest).await.unwrap();
            stream
        });
        let (stream, _) = listener.accept().await.unwrap();
        let path = peek_path(&stream, Duration::from_secs(2)).await;
        drop(client.await.unwrap());
        path
    }

    #[tokio::test]
    async fn only_proxy_and_login_requests_are_taken_from_the_websocket() {
        assert_eq!(
            peeked(b"POST /mcp/s/x HTTP/1.1\r\nHost: a\r\n\r\n")
                .await
                .as_deref(),
            Some("/mcp/s/x")
        );
        assert_eq!(
            peeked(b"GET /oauth/callback?code=1 HTTP/1.1\r\n\r\n")
                .await
                .as_deref(),
            Some("/oauth/callback?code=1")
        );
        assert_eq!(
            peeked(b"GET / HTTP/1.1\r\nUpgrade: websocket\r\n\r\n").await,
            None
        );
    }

    #[test]
    fn the_callback_page_never_renders_markup_from_the_server() {
        assert_eq!(escape_html("<b>&\"</b>"), "&lt;b&gt;&amp;&quot;&lt;/b&gt;");
        assert_eq!(
            url_pairs("code=a%2Bb&state=x"),
            [
                ("code".to_owned(), "a+b".to_owned()),
                ("state".to_owned(), "x".to_owned())
            ]
        );
    }
}
