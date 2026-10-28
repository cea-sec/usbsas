//! HTTP handlers.
//!
//! See `README.md` for the API description.

use crate::{
    clamav::Clamav,
    error::{Error, Result},
    scan::Scanner,
    storage::{self, Encoding, Storage, validate_id},
};
use actix_files::NamedFile;
use actix_web::{
    HttpRequest, HttpResponse, Responder, dev::ServerHandle, get, head, http::header, post, web,
};
use serde_json::json;
use std::sync::{Arc, OnceLock};

/// Header carrying the size of the bundle once decoded.
pub const UNCOMPRESSED_LENGTH_HEADER: &str = "X-Uncompressed-Content-Length";

/// State shared by all the workers.
pub struct AppState {
    pub storage: Arc<Storage>,
    pub scanner: Arc<Scanner>,
    pub clamav: Arc<Clamav>,
    /// Whether `GET /shutdown` is honored.
    pub shutdown_enabled: bool,
    /// Set once the server is started, used by `GET /shutdown`.
    pub server: OnceLock<ServerHandle>,
}

fn content_length(req: &HttpRequest) -> Result<Option<u64>> {
    req.headers()
        .get(header::CONTENT_LENGTH)
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| Error::BadRequest("invalid Content-Length".into()))
        })
        .transpose()
}

/// `POST /api/scanbundle/{user_id}`: submit a tar for analysis.
///
/// Answers `{"id": "<bundle id>", "status": "uploaded"}` once the body is
/// fully received; the analysis then runs in the background.
#[post("/api/scanbundle/{user_id}")]
async fn scan_bundle(
    req: HttpRequest,
    body: web::Payload,
    user_id: web::Path<String>,
    data: web::Data<AppState>,
) -> Result<impl Responder> {
    validate_id("user", &user_id)?;
    if data.clamav.is_stopping() {
        return Err(Error::Unavailable("antivirus stopped"));
    }
    let bundle_id = data
        .scanner
        .submit(&user_id, body, content_length(&req)?)
        .await?;
    Ok(HttpResponse::Ok().json(json!({ "id": bundle_id, "status": "uploaded" })))
}

/// `GET /api/scanbundle/{user_id}/{bundle_id}`: poll an analysis.
///
/// `{"id": ..., "status": "processing"}` while running, then the full report
/// (only once). 404 if unknown or submitted by another user.
#[get("/api/scanbundle/{user_id}/{bundle_id}")]
async fn scan_result(
    params: web::Path<(String, String)>,
    data: web::Data<AppState>,
) -> Result<impl Responder> {
    let (user_id, bundle_id) = params.into_inner();
    validate_id("user", &user_id)?;
    validate_id("bundle", &bundle_id)?;
    Ok(HttpResponse::Ok().json(data.scanner.poll(&user_id, &bundle_id)?))
}

/// `POST /api/uploadbundle/{user_id}`: store a tar for a user.
///
/// The bundle is stored in `WORKING-DIR/<user id>/<bundle id>.tar`, and can
/// then be downloaded at `/api/downloadbundle/<user id>/<bundle id>`.
/// Answers `{"id": "<bundle id>", "status": "uploaded"}`.
#[post("/api/uploadbundle/{user_id}")]
async fn upload_bundle(
    req: HttpRequest,
    body: web::Payload,
    user_id: web::Path<String>,
    data: web::Data<AppState>,
) -> Result<impl Responder> {
    validate_id("user", &user_id)?;
    let bundle_id = storage::new_bundle_id();
    let dest = data.storage.user_bundle_path(&user_id, &bundle_id);
    let size = data
        .storage
        .receive(body, content_length(&req)?, &dest)
        .await?;
    log::info!(
        "[{user_id}] bundle {bundle_id} stored in {} ({size} bytes)",
        dest.display()
    );
    Ok(HttpResponse::Ok().json(json!({ "id": bundle_id, "status": "uploaded" })))
}

/// `HEAD /api/downloadbundle/{user_id}/{bundle_id}`: size of a bundle.
///
/// The `X-Uncompressed-Content-Length` header contains the size of the tar
/// (decompressed if the bundle is gzipped, whatever its size).
#[head("/api/downloadbundle/{user_id}/{bundle_id}")]
async fn head_bundle_size(
    params: web::Path<(String, String)>,
    data: web::Data<AppState>,
) -> Result<impl Responder> {
    let (user_id, bundle_id) = params.into_inner();
    validate_id("user", &user_id)?;
    validate_id("bundle", &bundle_id)?;
    let bundle = data.storage.find_bundle(&user_id, &bundle_id).await?;
    let size = data.storage.uncompressed_size(&bundle).await?;
    log::debug!(
        "[{user_id}] size of {}: {size} bytes",
        bundle.path.display()
    );
    Ok(HttpResponse::Ok()
        .insert_header((UNCOMPRESSED_LENGTH_HEADER, size))
        .finish())
}

/// `GET /api/downloadbundle/{user_id}/{bundle_id}`: download a bundle.
///
/// Gzipped bundles are sent as is with `Content-Encoding: gzip`. Range
/// requests are supported.
#[get("/api/downloadbundle/{user_id}/{bundle_id}")]
async fn download_bundle(
    params: web::Path<(String, String)>,
    data: web::Data<AppState>,
) -> Result<impl Responder> {
    let (user_id, bundle_id) = params.into_inner();
    validate_id("user", &user_id)?;
    validate_id("bundle", &bundle_id)?;
    let bundle = data.storage.find_bundle(&user_id, &bundle_id).await?;
    log::info!("[{user_id}] download of {}", bundle.path.display());
    let path = bundle.path.clone();
    let file = tokio::task::spawn_blocking(move || NamedFile::open(path))
        .await
        .map_err(|err| Error::Internal(format!("open task failed: {err}")))??;
    Ok(match bundle.encoding {
        Encoding::Gzip => file.set_content_encoding(header::ContentEncoding::Gzip),
        Encoding::Tar => file,
    })
}

/// `GET /shutdown`: stop clamd and the server (used by the integration
/// tests). Disabled with `--disable-shutdown`.
#[get("/shutdown")]
async fn shutdown(data: web::Data<AppState>) -> Result<impl Responder> {
    if !data.shutdown_enabled {
        return Err(Error::NotFound("page"));
    }
    log::info!("shutdown requested");
    let clamav = Arc::clone(&data.clamav);
    tokio::task::spawn_blocking(move || clamav.shutdown())
        .await
        .map_err(|err| Error::Internal(err.to_string()))?;
    if let Some(server) = data.server.get().cloned() {
        // Graceful: lets this response (and the other in-flight requests)
        // complete.
        actix_web::rt::spawn(async move { server.stop(true).await });
    }
    Ok(HttpResponse::Ok().finish())
}

/// Register all the routes.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(scan_bundle)
        .service(scan_result)
        .service(upload_bundle)
        .service(head_bundle_size)
        .service(download_bundle)
        .service(shutdown);
}
