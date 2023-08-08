use std::sync::Arc;

use axum::extract::FromRequestParts;
use bytes::Bytes;
use hyper::http::request::Parts;

use crate::database::factory::DbFactory;
use crate::error::Error;
use crate::namespace::NamespaceFactory;

use super::AppState;

pub struct DbFactoryExtractor<D>(pub Arc<dyn DbFactory<Db = D>>);

#[async_trait::async_trait]
impl<F> FromRequestParts<AppState<F>> for DbFactoryExtractor<F::Database>
where
    F: NamespaceFactory,
{
    type Rejection = Error;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState<F>,
    ) -> Result<Self, Self::Rejection> {
        let host = parts
            .headers
            .get("host")
            .ok_or_else(|| Error::InvalidHost("missing host header".into()))?
            .as_bytes();
        let host_str = std::str::from_utf8(host)
            .map_err(|_| Error::InvalidHost("host header is not valid UTF-8".into()))?;
        let ns = split_namespace(host_str)?;
        Ok(Self(
            state
                .namespaces
                .with(ns, |ns| ns.db_factory.clone())
                .await?,
        ))
    }
}

pub fn split_namespace(host: &str) -> crate::Result<Bytes> {
    let (ns, _) = host.split_once('.').ok_or_else(|| {
        Error::InvalidHost("host header should be in the format <namespace>.<...>".into())
    })?;
    let ns = Bytes::copy_from_slice(ns.as_bytes());
    Ok(ns)
}
