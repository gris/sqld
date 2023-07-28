use std::sync::Arc;

use color_eyre::eyre::Context;
use futures::Future;
use parking_lot::Mutex;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::oneshot;

use crate::allocation::ConnectionHandle;

use self::proto::{PipelineRequestBody, PipelineResponseBody};

use super::error::{HranaError, ProtocolError, StreamError};

pub mod proto;
mod request;
mod stream;

pub struct Server {
    self_url: Option<String>,
    baton_key: [u8; 32],
    stream_state: Mutex<stream::ServerStreamState>,
}

#[derive(Debug)]
pub enum Route {
    GetIndex,
    PostPipeline,
}

impl Server {
    pub fn new(self_url: Option<String>) -> Self {
        Self {
            self_url,
            baton_key: rand::random(),
            stream_state: Mutex::new(stream::ServerStreamState::new()),
        }
    }

    pub async fn run_expire(&self) {
        stream::run_expire(self).await
    }
}

fn handle_index() -> crate::Result<hyper::Response<hyper::Body>, HranaError> {
    Ok(text_response(
        hyper::StatusCode::OK,
        "Hello, this is HTTP API v2 (Hrana over HTTP)".into(),
    ))
}

pub async fn handle_pipeline<F, Fut>(
    server: Arc<Server>,
    req: PipelineRequestBody,
    ret: oneshot::Sender<crate::Result<PipelineResponseBody, HranaError>>,
    mk_conn: F,
) -> crate::Result<(), HranaError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = crate::Result<ConnectionHandle>>,
{
    let mut stream_guard = stream::acquire(server.clone(), req.baton.as_deref(), mk_conn).await?;

    tokio::spawn(async move {
        let f = async move {
            let mut results = Vec::with_capacity(req.requests.len());
            for request in req.requests.into_iter() {
                let result = request::handle(&mut stream_guard, request)
                    .await?;
                results.push(result);
            }

            Ok(proto::PipelineResponseBody {
                baton: stream_guard.release(),
                base_url: server.self_url.clone(),
                results,
            })
        };

        let _ = ret.send(f.await);
    });

    Ok(())
}

async fn read_request_json<T: DeserializeOwned>(
    req: hyper::Request<hyper::Body>,
) -> color_eyre::Result<T> {
    let req_body = hyper::body::to_bytes(req.into_body())
        .await
        .context("Could not read request body")?;
    let req_body = serde_json::from_slice(&req_body)
        .map_err(|err| ProtocolError::Deserialize { source: err })
        .context("Could not deserialize JSON request body")?;
    Ok(req_body)
}

fn protocol_error_response(err: ProtocolError) -> hyper::Response<hyper::Body> {
    text_response(hyper::StatusCode::BAD_REQUEST, err.to_string())
}

fn stream_error_response(err: StreamError) -> hyper::Response<hyper::Body> {
    json_response(
        hyper::StatusCode::INTERNAL_SERVER_ERROR,
        &proto::Error {
            message: err.to_string(),
            code: err.code().into(),
        },
    )
}

fn json_response<T: Serialize>(
    status: hyper::StatusCode,
    resp_body: &T,
) -> hyper::Response<hyper::Body> {
    let resp_body = serde_json::to_vec(resp_body).unwrap();
    hyper::Response::builder()
        .status(status)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(hyper::Body::from(resp_body))
        .unwrap()
}

fn text_response(status: hyper::StatusCode, resp_body: String) -> hyper::Response<hyper::Body> {
    hyper::Response::builder()
        .status(status)
        .header(hyper::http::header::CONTENT_TYPE, "text/plain")
        .body(hyper::Body::from(resp_body))
        .unwrap()
}
