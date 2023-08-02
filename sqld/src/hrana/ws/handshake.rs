//! This file handles web socket handshakes.

use anyhow::{anyhow, bail, Context as _, Result};
use bytes::Bytes;
use futures::{SinkExt as _, StreamExt as _};
use hyper::Request;
use tokio_tungstenite::tungstenite;
use tungstenite::http;

use crate::database::Database;
use crate::http::db_factory::split_namespace;

use super::super::Version;
use super::Upgrade;

#[derive(Debug)]
pub enum WebSocket {
    Tcp(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>),
    Upgraded(tokio_tungstenite::WebSocketStream<hyper::upgrade::Upgraded>),
}

fn extract_namespace<B>(req: &Request<B>) -> anyhow::Result<Bytes> {
    let host = req.headers().get("host").context("missing host header")?;
    Ok(split_namespace(std::str::from_utf8(host.as_bytes())?)?)
}

pub async fn handshake_tcp<D: Database>(socket: tokio::net::TcpStream) -> Result<(WebSocket, Version, Bytes)> {
    let mut version = None;
    let mut namespace = None;
    let callback = |req: &http::Request<()>, resp: http::Response<()>| {
        let (mut resp_parts, _) = resp.into_parts();
        resp_parts
            .headers
            .insert("server", http::HeaderValue::from_static("sqld-hrana-tcp"));
        
        match extract_namespace(&req) {
            Ok(ns) => { namespace = Some(ns); },
            Err(e) => return Err(http::Response::from_parts(resp_parts, Some(e.to_string()))),
        }

        match negotiate_version(req.headers(), &mut resp_parts.headers) {
            Ok(version_) => {
                version = Some(version_);
                Ok(http::Response::from_parts(resp_parts, ()))
            }
            Err(resp_body) => Err(http::Response::from_parts(resp_parts, Some(resp_body))),
        }
    };

    let ws_config = Some(get_ws_config());
    let stream =
        tokio_tungstenite::accept_hdr_async_with_config(socket, callback, ws_config).await?;
    Ok((WebSocket::Tcp(stream), version.unwrap(), namespace.unwrap()))
}

pub async fn handshake_upgrade(upgrade: Upgrade) -> Result<(WebSocket, Version, Bytes)> {
    let mut req = upgrade.request;

    let host = req.headers().get("host").context("missing host header")?;
    let ns = split_namespace(std::str::from_utf8(host.as_bytes())?)?;

    let ws_config = Some(get_ws_config());
    let (mut resp, stream_fut_version_res) = match hyper_tungstenite::upgrade(&mut req, ws_config) {
        Ok((mut resp, stream_fut)) => match negotiate_version(req.headers(), resp.headers_mut()) {
            Ok(version) => (resp, Ok((stream_fut, version, ns))),
            Err(msg) => {
                *resp.status_mut() = http::StatusCode::BAD_REQUEST;
                *resp.body_mut() = hyper::Body::from(msg.clone());
                (
                    resp,
                    Err(anyhow!("Could not negotiate subprotocol: {}", msg)),
                )
            }
        },
        Err(err) => {
            let resp = http::Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body(hyper::Body::from(format!("{err}")))
                .unwrap();
            (
                resp,
                Err(anyhow!(err).context("Protocol error in HTTP upgrade")),
            )
        }
    };

    resp.headers_mut().insert(
        "server",
        http::HeaderValue::from_static("sqld-hrana-upgrade"),
    );
    if upgrade.response_tx.send(resp).is_err() {
        bail!("Could not send the HTTP upgrade response")
    }

    let (stream_fut, version, ns) = stream_fut_version_res?;
    let stream = stream_fut
        .await
        .context("Could not upgrade HTTP request to a WebSocket")?;
    Ok((WebSocket::Upgraded(stream), version, ns))
}

fn negotiate_version(
    req_headers: &http::HeaderMap,
    resp_headers: &mut http::HeaderMap,
) -> Result<Version, String> {
    if let Some(protocol_hdr) = req_headers.get("sec-websocket-protocol") {
        let supported_by_client = protocol_hdr
            .to_str()
            .unwrap_or("")
            .split(',')
            .map(|p| p.trim());

        let mut hrana1_supported = false;
        let mut hrana2_supported = false;
        for protocol_str in supported_by_client {
            hrana1_supported |= protocol_str.eq_ignore_ascii_case("hrana1");
            hrana2_supported |= protocol_str.eq_ignore_ascii_case("hrana2");
        }

        let version = if hrana2_supported {
            Version::Hrana2
        } else if hrana1_supported {
            Version::Hrana1
        } else {
            return Err("Only 'hrana1' and 'hrana2' subprotocols are supported".into());
        };

        resp_headers.append(
            "sec-websocket-protocol",
            http::HeaderValue::from_str(&version.to_string()).unwrap(),
        );
        Ok(version)
    } else {
        // Sec-WebSocket-Protocol header not present, assume that the client wants hrana1
        // According to RFC 6455, we must not set the Sec-WebSocket-Protocol response header
        Ok(Version::Hrana1)
    }
}

fn get_ws_config() -> tungstenite::protocol::WebSocketConfig {
    tungstenite::protocol::WebSocketConfig {
        max_send_queue: Some(1 << 20),
        ..Default::default()
    }
}

impl WebSocket {
    pub async fn recv(&mut self) -> Option<tungstenite::Result<tungstenite::Message>> {
        match self {
            Self::Tcp(stream) => stream.next().await,
            Self::Upgraded(stream) => stream.next().await,
        }
    }

    pub async fn send(&mut self, msg: tungstenite::Message) -> tungstenite::Result<()> {
        match self {
            Self::Tcp(stream) => stream.send(msg).await,
            Self::Upgraded(stream) => stream.send(msg).await,
        }
    }
}
