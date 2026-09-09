//! The web UI: a single page served at `/`, with its stylesheet, script and
//! artwork embedded in the binary.
//!
//! Everything is compiled in with `include_bytes!`, so the executable stays
//! self-contained on every platform - nothing to install beside it, nothing
//! fetched from the internet at run time. The page talks to `/api/v1` with
//! plain `fetch`, the same calls a script would make.

use std::sync::OnceLock;

use axum::extract::Path;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/favicon.ico", get(|h: HeaderMap| asset(h, "favicon.ico")))
        .route(
            "/assets/{name}",
            get(|h: HeaderMap, Path(n): Path<String>| asset(h, n)),
        )
}

/// The page, with the version stamped in. Rendered once: the template is a
/// compile-time constant and the version never changes while running.
fn page() -> &'static str {
    static PAGE: OnceLock<String> = OnceLock::new();
    PAGE.get_or_init(|| include_str!("../web/index.html").replace("{{VERSION}}", crate::version()))
}

async fn index() -> Response {
    ([(header::CACHE_CONTROL, "no-cache")], Html(page())).into_response()
}

struct Asset {
    bytes: &'static [u8],
    content_type: &'static str,
}

fn lookup(name: &str) -> Option<Asset> {
    let (bytes, content_type): (&'static [u8], &'static str) = match name {
        "style.css" => (
            include_bytes!("../web/style.css"),
            "text/css; charset=utf-8",
        ),
        "app.js" => (
            include_bytes!("../web/app.js"),
            "text/javascript; charset=utf-8",
        ),
        "faint-light-logo.png" => (include_bytes!("../web/faint-light-logo.png"), "image/png"),
        "nightsky-observer-logo.png" => (
            include_bytes!("../web/nightsky-observer-logo.png"),
            "image/png",
        ),
        "favicon-32.png" => (include_bytes!("../web/favicon-32.png"), "image/png"),
        "favicon-180.png" => (include_bytes!("../web/favicon-180.png"), "image/png"),
        "favicon.ico" => (include_bytes!("../web/favicon.ico"), "image/x-icon"),
        _ => return None,
    };
    Some(Asset {
        bytes,
        content_type,
    })
}

/// Serve one embedded file. The ETag is a hash of the bytes, so a browser
/// revalidates cheaply and still picks up a new build straight away.
async fn asset(headers: HeaderMap, name: impl AsRef<str>) -> Response {
    let Some(a) = lookup(name.as_ref()) else {
        return (StatusCode::NOT_FOUND, "no such asset").into_response();
    };
    let etag = format!("\"{}\"", sha1_smol::Sha1::from(a.bytes).digest());
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        [
            (header::CONTENT_TYPE, a.content_type.to_string()),
            (header::CACHE_CONTROL, "no-cache".to_string()),
            (header::ETAG, etag),
        ],
        a.bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_carries_the_version_and_the_api_links() {
        let p = page();
        assert!(p.contains(crate::version()));
        assert!(!p.contains("{{VERSION}}"));
        assert!(p.contains("/api/v1/solve"));
        assert!(p.contains("faint_light"), "the release check greps for it");
    }

    #[test]
    fn every_asset_the_page_references_is_embedded() {
        let p = page();
        let mut seen = 0;
        for chunk in p.split("/assets/").skip(1) {
            let name: String = chunk
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c))
                .collect();
            assert!(
                lookup(&name).is_some(),
                "{name} is referenced but not embedded"
            );
            seen += 1;
        }
        assert!(
            seen >= 3,
            "expected the stylesheet, the script and the logo"
        );
        assert!(lookup("favicon.ico").is_some());
        assert!(lookup("../Cargo.toml").is_none());
    }
}
