#![allow(clippy::disallowed_methods)] // RustEmbed generates calls to std::path::Path::canonicalize

use std::borrow::Cow;

use axum::{
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

use crate::assets::{asset_response, is_navigation_path, normalize_path, not_found};

// `web/dist/` is a committed build of the SPA whose source lives beside it in
// `web/src/`. Cargo interpolates `CARGO_MANIFEST_DIR`, so the path needs no help from
// `build.rs`.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/web/dist/"]
struct Assets;

/// The embedded SPA bundle as `(bundle-relative path, bytes)`.
///
/// Lazy, and one file at a time: the export writes each as it arrives rather than
/// gathering the bundle first. The bytes stay borrowed from the binary's own data —
/// in a release build `Cow::Borrowed`, so nothing is copied to hand a file over.
/// Reads through `Assets::get` rather than yielding the `EmbeddedFile` so callers need
/// no `rust_embed` dependency.
pub(crate) fn iter_assets() -> impl Iterator<Item = (String, Cow<'static, [u8]>)> {
    Assets::iter().filter_map(|path| {
        let path = path.to_string();
        Assets::get(&path).map(|file| (path, file.data))
    })
}

/// Fallback handler for the embedded SPA.
///
/// Tries the requested file, then falls back to `index.html` so SPA hash
/// routes resolve client-side. A missing *file* 404s rather than falling back, for
/// the reason in [`is_navigation_path`] — and all the more so here, since the
/// embedded bundle carries no parquet at all.
pub async fn serve_assets(uri: Uri) -> Response {
    let path = normalize_path(uri.path());

    if let Some(file) = Assets::get(&path) {
        return asset_response(&path, file.data.into_owned(), None);
    }
    if !is_navigation_path(uri.path()) {
        return not_found();
    }
    if let Some(file) = Assets::get("index.html") {
        return asset_response("index.html", file.data.into_owned(), None);
    }
    (StatusCode::NOT_FOUND, "dbt docs SPA bundle is empty").into_response()
}
