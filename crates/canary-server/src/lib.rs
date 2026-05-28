//! Axum server wiring for Canary.
//!
//! This crate adapts the stable wire contracts from `canary-http` to concrete
//! HTTP responses. Domain decisions and body shapes stay out of the router.

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{
        HeaderValue, Response, StatusCode,
        header::{CONTENT_TYPE, HeaderName},
    },
    routing::get,
};
use canary_http::public::{
    DependencyStatus, PublicResponse, healthz_response, openapi_response, readyz_response,
};
use serde::Serialize;

/// Snapshot of dependency readiness for the public readiness endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicReadiness {
    database: DependencyStatus,
    supervisor: DependencyStatus,
}

impl PublicReadiness {
    /// Build readiness from explicit dependency statuses.
    pub const fn new(database: DependencyStatus, supervisor: DependencyStatus) -> Self {
        Self {
            database,
            supervisor,
        }
    }

    /// Convenience constructor for a fully ready process.
    pub const fn ready() -> Self {
        Self::new(DependencyStatus::Ok, DependencyStatus::Ok)
    }
}

/// Router for Canary's public unauthenticated endpoints.
pub fn public_router(readiness: PublicReadiness) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/v1/openapi.json", get(openapi))
        .with_state(readiness)
}

async fn healthz() -> Response<Body> {
    json_response(healthz_response())
}

async fn readyz(State(readiness): State<PublicReadiness>) -> Response<Body> {
    json_response(readyz_response(readiness.database, readiness.supervisor))
}

async fn openapi() -> Response<Body> {
    text_response(openapi_response())
}

fn json_response<T>(contract: PublicResponse<T>) -> Response<Body>
where
    T: Serialize,
{
    match serde_json::to_vec(&contract.body) {
        Ok(body) => response(contract.status, contract.content_type, Body::from(body)),
        Err(_) => internal_server_error(),
    }
}

fn text_response(contract: PublicResponse<&'static str>) -> Response<Body> {
    response(
        contract.status,
        contract.content_type,
        Body::from(contract.body),
    )
}

fn response(status: u16, content_type: &'static str, body: Body) -> Response<Body> {
    let mut response = Response::new(body);
    *response.status_mut() = status_code(status);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

fn internal_server_error() -> Response<Body> {
    response(
        StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        "text/plain; charset=utf-8",
        Body::from("internal server error"),
    )
}

fn status_code(status: u16) -> StatusCode {
    match StatusCode::from_u16(status) {
        Ok(status) => status,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Headers set by the public adapter.
pub const PUBLIC_CONTENT_TYPE: HeaderName = CONTENT_TYPE;

#[cfg(test)]
mod tests {
    use std::error::Error;

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header::CONTENT_TYPE},
    };
    use canary_http::public::{APPLICATION_JSON, OPENAPI_JSON};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn healthz_adapts_the_public_contract() -> Result<(), Box<dyn Error>> {
        let response = public_router(PublicReadiness::ready())
            .oneshot(Request::get("/healthz").body(Body::empty())?)
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static(APPLICATION_JSON))
        );
        assert_eq!(json_body(response).await?, json!({"status": "ok"}));

        Ok(())
    }

    #[tokio::test]
    async fn readyz_returns_ready_when_all_dependencies_are_ok() -> Result<(), Box<dyn Error>> {
        let response = public_router(PublicReadiness::ready())
            .oneshot(Request::get("/readyz").body(Body::empty())?)
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await?,
            json!({
                "status": "ready",
                "checks": {
                    "database": "ok",
                    "supervisor": "ok"
                }
            })
        );

        Ok(())
    }

    #[tokio::test]
    async fn readyz_returns_503_when_any_dependency_fails() -> Result<(), Box<dyn Error>> {
        let cases = [
            PublicReadiness::new(DependencyStatus::Error, DependencyStatus::Ok),
            PublicReadiness::new(DependencyStatus::Ok, DependencyStatus::Error),
            PublicReadiness::new(DependencyStatus::Error, DependencyStatus::Error),
        ];

        for readiness in cases {
            let response = public_router(readiness)
                .oneshot(Request::get("/readyz").body(Body::empty())?)
                .await?;
            let status = response.status();
            let body = json_body(response).await?;

            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["status"], "not_ready");
        }

        Ok(())
    }

    #[tokio::test]
    async fn openapi_serves_the_checked_in_document_unchanged() -> Result<(), Box<dyn Error>> {
        let response = public_router(PublicReadiness::ready())
            .oneshot(Request::get("/api/v1/openapi.json").body(Body::empty())?)
            .await?;
        let content_type = response.headers().get(CONTENT_TYPE).cloned();
        let body = to_bytes(response.into_body(), usize::MAX).await?;

        assert_eq!(
            content_type,
            Some(HeaderValue::from_static(APPLICATION_JSON))
        );
        assert_eq!(body.as_ref(), OPENAPI_JSON.as_bytes());

        Ok(())
    }

    #[tokio::test]
    async fn public_router_does_not_mount_private_routes() -> Result<(), Box<dyn Error>> {
        let response = public_router(PublicReadiness::ready())
            .oneshot(Request::get("/api/v1/query").body(Body::empty())?)
            .await?;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        Ok(())
    }

    async fn json_body(response: Response<Body>) -> Result<Value, Box<dyn Error>> {
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice(&bytes)?;

        Ok(body)
    }
}
