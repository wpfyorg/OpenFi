//! The Preact UI (`web/dist`) is compiled into the binary — one file to copy to the router.
//! `npm run build` gzips every asset and drops the originals (`web/scripts/gzip-dist.mjs`), so the
//! binary carries only `*.gz` and serves them with `Content-Encoding: gzip`. Plain files are still
//! served if present (e.g. a dist built without the gzip step).

use axum::{
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web/dist/"]
struct Assets;

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    // Unknown paths are client-side routes: serve the SPA shell.
    let path = if exists(path) { path } else { "index.html" };

    let mime = mime_guess::from_path(path).first_or_text_plain();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let headers = [
        (header::CONTENT_TYPE, mime.as_ref().to_owned()),
        (header::CACHE_CONTROL, cache.to_owned()),
    ];
    if let Some(file) = Assets::get(&format!("{path}.gz")) {
        let mut resp = (headers, file.data).into_response();
        resp.headers_mut()
            .insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
        return resp;
    }
    match Assets::get(path) {
        Some(file) => (headers, file.data).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "UI not built — run `npm run build` in web/",
        )
            .into_response(),
    }
}

fn exists(path: &str) -> bool {
    Assets::get(path).is_some() || Assets::get(&format!("{path}.gz")).is_some()
}
