//! The web client (`web/dist`, built from `web/` with `scripts/web-build.sh`), embedded in the
//! binary. Debug builds read the files from disk, so a rebuilt client shows up without
//! recompiling the server.

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/dist"]
struct Dist;

/// Same-origin everything; inline styles are allowed for the client's transitions; media and
/// workers may use blob: URLs (hls.js).
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
     media-src 'self' blob:; worker-src 'self' blob:; connect-src 'self'; font-src 'self'; object-src 'none'; \
     frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

/// Serves a file of the client; unknown paths get `index.html` (client-side routing).
pub async fn serve(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let (file, name) = match Dist::get(path).filter(|_| !path.is_empty()) {
        Some(f) => (f, path),
        None => match Dist::get("index.html") {
            Some(f) => (f, "index.html"),
            None => return (StatusCode::NOT_FOUND, "the web client is not built").into_response(),
        },
    };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    // Vite puts a content hash into asset names: those never change.
    let hashed = name.starts_with("assets/");
    let cache = if hashed { "public, max-age=31536000, immutable" } else { "no-cache" };
    // The others are revalidated at each page load: usually that's all it takes.
    let etag = (!hashed).then(|| {
        let hash: String = file.metadata.sha256_hash().iter().map(|b| format!("{b:02x}")).collect();
        format!("\"{hash}\"")
    });
    let unchanged = etag.as_ref().is_some_and(|e| {
        headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).is_some_and(|sent| sent.contains(e.as_str()))
    });
    let mut res = if unchanged {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        // Embedded files are `'static` in release builds: no copy per request.
        let body = match file.data {
            std::borrow::Cow::Borrowed(b) => Body::from(Bytes::from_static(b)),
            std::borrow::Cow::Owned(v) => Body::from(v),
        };
        (StatusCode::OK, body).into_response()
    };
    let h = res.headers_mut();
    if let Some(v) = etag.and_then(|e| HeaderValue::from_str(&e).ok()) {
        h.insert(header::ETAG, v);
    }
    if let Ok(v) = HeaderValue::from_str(mime.as_ref()) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    h.insert("permissions-policy", HeaderValue::from_static("camera=(), microphone=(), geolocation=()"));
    res
}
