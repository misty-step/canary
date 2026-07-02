//! Static human dashboard shell.
//!
//! The dashboard is intentionally a same-origin static client over Canary's
//! existing API. It does not mint keys, proxy reads, or expose any new data
//! surface; operators paste a scoped API key into the browser session.

use axum::{
    Router,
    body::Body,
    http::{
        Response, StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue},
    },
    routing::get,
};

const DASHBOARD_HTML: &str = include_str!("../static/dashboard/index.html");
const DASHBOARD_CSS: &str = include_str!("../static/dashboard/dashboard.css");
const DASHBOARD_JS: &str = include_str!("../static/dashboard/app.js");
const AESTHETIC_CSS: &str = include_str!("../static/dashboard/aesthetic.css");

/// Router for Canary's human dashboard shell.
pub fn dashboard_router() -> Router {
    Router::new()
        .route("/ui", get(index))
        .route("/ui/", get(index))
        .route("/ui/dashboard.css", get(dashboard_css))
        .route("/ui/aesthetic.css", get(aesthetic_css))
        .route("/ui/app.js", get(app_js))
}

async fn index() -> Response<Body> {
    static_response("text/html; charset=utf-8", DASHBOARD_HTML)
}

async fn dashboard_css() -> Response<Body> {
    static_response("text/css; charset=utf-8", DASHBOARD_CSS)
}

async fn aesthetic_css() -> Response<Body> {
    static_response("text/css; charset=utf-8", AESTHETIC_CSS)
}

async fn app_js() -> Response<Body> {
    static_response("text/javascript; charset=utf-8", DASHBOARD_JS)
}

fn static_response(content_type: &'static str, body: &'static str) -> Response<Body> {
    match Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, HeaderValue::from_static(content_type))
        .header(CACHE_CONTROL, HeaderValue::from_static("no-store"))
        .body(Body::from(body))
    {
        Ok(response) => response,
        Err(_) => {
            let mut response = Response::new(Body::from("dashboard response error"));
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
    }
}
