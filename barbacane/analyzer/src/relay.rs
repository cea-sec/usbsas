//! Mode relais (borne reliée à un analyseur central) : écoute en local sur
//! 127.0.0.1 pour usbsas et l'interface, comme l'analyseur local, et
//! transmet tout au serveur central en TLS avec le jeton de la borne.
//! usbsas n'est pas modifié : il ne sait envoyer ni jeton ni certificat.
//!
//! Exceptions traitées localement :
//! - `stage` : télécharge `files.tar` du serveur et le déballe dans le
//!   dossier de préparation local ;
//! - lecture d'une clé admin (`import/arm`) : les paquets `*.bcu` du tar
//!   envoyé par usbsas sont déposés localement avant transmission.

use crate::config::Relay;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, post},
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;

struct RelayApp {
    upstream: String,
    token: String,
    http: reqwest::Client,
    stage_dir: PathBuf,
    import_dir: PathBuf,
    work_dir: PathBuf,
    import_armed: AtomicBool,
}

type Shared = Arc<RelayApp>;

pub fn router(
    r: &Relay,
    stage_dir: PathBuf,
    import_dir: PathBuf,
    work_dir: PathBuf,
) -> anyhow::Result<Router> {
    let token = std::fs::read_to_string(&r.token_file)?.trim().to_string();
    let mut b = reqwest::Client::builder()
        .use_rustls_tls()
        .https_only(r.upstream.starts_with("https://"));
    if let Some(ca) = &r.ca_cert {
        // Certificat (ou AC) du serveur central épinglé ; les AC du système ne sont pas utilisées.
        b = b.tls_certs_only([reqwest::Certificate::from_pem(&std::fs::read(ca)?)?]);
    }
    let app = Arc::new(RelayApp {
        upstream: r.upstream.trim_end_matches('/').to_string(),
        token,
        http: b.build()?,
        stage_dir,
        import_dir,
        work_dir,
        import_armed: AtomicBool::new(false),
    });
    Ok(Router::new()
        .route("/", any(|| async { "barbacane-analyzer (relais)" }))
        .route("/api/scanbundle/{uid}", post(upload))
        .route("/barbacane/v1/scans/{id}/stage", post(stage))
        .route("/barbacane/v1/import/arm", post(arm))
        .fallback(forward)
        .with_state(app))
}

fn err(code: StatusCode, msg: impl ToString) -> Response {
    (code, Json(json!({"error": msg.to_string()}))).into_response()
}

async fn send(app: &RelayApp, req: reqwest::RequestBuilder) -> Response {
    match req.bearer_auth(&app.token).send().await {
        Ok(r) => {
            let mut out = Response::builder().status(r.status().as_u16());
            if let Some(ct) = r.headers().get(header::CONTENT_TYPE) {
                out = out.header(header::CONTENT_TYPE, ct);
            }
            out.body(Body::from_stream(r.bytes_stream()))
                .unwrap_or_else(|e| err(StatusCode::BAD_GATEWAY, e))
        }
        Err(e) => {
            log::error!("serveur central injoignable : {e}");
            err(
                StatusCode::BAD_GATEWAY,
                format!("serveur d'analyse injoignable : {e}"),
            )
        }
    }
}

/// Transmission telle quelle (méthode, chemin, requête, corps).
async fn forward(State(app): State<Shared>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let url = format!(
        "{}{}",
        app.upstream,
        parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
    );
    let mut rb = app.http.request(parts.method, url);
    if let Some(ct) = parts.headers.get(header::CONTENT_TYPE) {
        rb = rb.header(header::CONTENT_TYPE, ct);
    }
    send(
        &app,
        rb.body(reqwest::Body::wrap_stream(body.into_data_stream())),
    )
    .await
}

/// Tar usbsas : mis sur disque (dépôt éventuel des paquets de mise à jour), puis transmis.
async fn upload(State(app): State<Shared>, Path(uid): Path<String>, body: Body) -> Response {
    let tmp = match tempfile::Builder::new()
        .prefix("relais-")
        .tempdir_in(&app.work_dir)
    {
        Ok(t) => t,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let path = tmp.path().join("bundle.tar");
    let res: anyhow::Result<()> = async {
        let mut f = tokio::fs::File::create(&path).await?;
        let mut s = body.into_data_stream();
        while let Some(c) = s.next().await {
            f.write_all(&c?).await?;
        }
        f.flush().await?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        return err(StatusCode::BAD_REQUEST, e);
    }
    if app.import_armed.swap(false, Ordering::SeqCst) {
        let (p, dir) = (path.clone(), app.import_dir.clone());
        if let Err(e) = tokio::task::spawn_blocking(move || extract_updates(&p, &dir))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r)
        {
            log::error!("dépôt des paquets de mise à jour : {e:#}");
        }
    }
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let stream = tokio_util::io::ReaderStream::new(file);
    let rb = app
        .http
        .post(format!("{}/api/scanbundle/{uid}", app.upstream))
        .header(header::CONTENT_LENGTH, len)
        .body(reqwest::Body::wrap_stream(stream));
    let r = send(&app, rb).await;
    drop(tmp);
    r
}

/// Paquets `*.bcu` / `*.bcu.minisig` à la racine de la clé (sous `data/` dans le tar usbsas).
fn extract_updates(tar: &std::path::Path, dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut ar = tar::Archive::new(std::fs::File::open(tar)?);
    for e in ar.entries()? {
        let mut e = e?;
        let name = String::from_utf8_lossy(&e.path_bytes())
            .trim_start_matches('/')
            .to_string();
        let Some(base) = name.strip_prefix("data/") else {
            continue;
        };
        if !base.contains('/')
            && (base.ends_with(".bcu") || base.ends_with(".bcu.minisig"))
            && e.header().entry_type().is_file()
        {
            std::io::copy(&mut e, &mut std::fs::File::create(dir.join(base))?)?;
        }
    }
    Ok(())
}

async fn arm(State(app): State<Shared>) -> Response {
    app.import_armed.store(true, Ordering::SeqCst);
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct StageBody {
    device: String,
}

async fn stage(
    State(app): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<StageBody>,
) -> Response {
    let url = format!("{}/barbacane/v1/scans/{id}/files.tar", app.upstream);
    let r = match app
        .http
        .get(url)
        .query(&[("device", &b.device)])
        .bearer_auth(&app.token)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            return err(
                StatusCode::BAD_GATEWAY,
                format!("serveur d'analyse : HTTP {}", r.status()),
            );
        }
        Err(e) => return err(StatusCode::BAD_GATEWAY, e),
    };
    let tmp = match tempfile::Builder::new()
        .prefix("relais-")
        .tempdir_in(&app.work_dir)
    {
        Ok(t) => t,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let path = tmp.path().join("files.tar");
    let res: anyhow::Result<()> = async {
        let mut f = tokio::fs::File::create(&path).await?;
        let mut s = r.bytes_stream();
        while let Some(c) = s.next().await {
            let c: bytes::Bytes = c?;
            f.write_all(&c).await?;
        }
        f.flush().await?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        return err(StatusCode::BAD_GATEWAY, e);
    }
    let dir = app.stage_dir.clone();
    match tokio::task::spawn_blocking(move || crate::stage::unpack_tar(&path, &dir)).await {
        Ok(Ok(n)) => Json(json!({"files": n})).into_response(),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}
