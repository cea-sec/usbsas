//! barbacane-analyzer : analyseur compatible usbsas (ClamAV + YARA,
//! extraction récursive des archives en bac à sable).

mod av;
mod config;
mod extract;
mod job;
mod relay;
mod report;
mod scan;
mod stage;

use anyhow::Context;
use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use job::{Answer, Job, Status};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

struct App {
    conf: config::Config,
    engines: av::Engines,
    extractor: extract::Extractor,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    /// La prochaine analyse est la lecture d'une clé admin (paquets de mise à jour)
    import_armed: std::sync::atomic::AtomicBool,
}

type Shared = Arc<App>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "-h" || a == "--help") {
        println!("barbacane-analyzer [-c FICHIER.toml]  (défaut : /etc/barbacane/analyzer.toml)");
        return Ok(());
    }
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "info"));
    let conf_path = std::env::args()
        .skip_while(|a| a != "-c")
        .nth(1)
        .unwrap_or_else(|| "/etc/barbacane/analyzer.toml".into());
    let conf =
        config::Config::load(&conf_path).with_context(|| format!("configuration {conf_path}"))?;
    std::fs::create_dir_all(&conf.work_dir)
        .with_context(|| format!("dossier de travail {}", conf.work_dir.display()))?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    if let Some(r) = &conf.relay {
        let router = relay::router(
            r,
            conf.stage_dir.clone(),
            conf.import_dir.clone(),
            conf.work_dir.clone(),
        )?;
        let listener = tokio::net::TcpListener::bind(&conf.listen).await?;
        log::info!(
            "relais : écoute sur {}, analyse par {}",
            conf.listen,
            r.upstream
        );
        axum::serve(listener, router).await?;
        return Ok(());
    }
    let engines = av::Engines::new(conf.clamd_socket.clone(), conf.yara_rules_dir.as_deref())
        .context("règles YARA")?;
    match engines.clamav_version() {
        Ok(v) => log::info!("clamd joignable : {v:?}"),
        Err(e) => log::warn!("clamd injoignable pour l'instant : {e:#}"),
    }
    let extractor = extract::Extractor {
        seven_zip: conf.seven_zip.clone(),
        bwrap: conf.bwrap.clone(),
        timeout: Duration::from_secs(conf.limits.extract_timeout),
        memory: conf.limits.extract_memory,
    };
    let listen = conf.listen.clone();
    let server = conf.server.clone();
    let app: Shared = Arc::new(App {
        conf,
        engines,
        extractor,
        jobs: Mutex::new(HashMap::new()),
        import_armed: Default::default(),
    });
    let router = Router::new()
        .route("/", get(|| async { "barbacane-analyzer" }))
        .route("/api/scanbundle/{uid}", post(upload))
        .route("/api/scanbundle/{uid}/{id}", get(usbsas_result))
        .route("/barbacane/v1/status", get(status))
        .route("/barbacane/v1/users/{uid}/current", get(current))
        .route("/barbacane/v1/scans/{id}", get(progress))
        .route("/barbacane/v1/scans/{id}/password", post(password))
        .route("/barbacane/v1/scans/{id}/report", get(detailed))
        .route("/barbacane/v1/scans/{id}/stage", post(stage_req))
        .route("/barbacane/v1/scans/{id}/files.tar", get(files_tar))
        .route("/barbacane/v1/import/arm", post(arm_import))
        .with_state(app);
    match server {
        None => {
            let listener = tokio::net::TcpListener::bind(&listen)
                .await
                .with_context(|| format!("écoute sur {listen}"))?;
            log::info!("écoute sur {listen}");
            axum::serve(listener, router).await?;
        }
        Some(srv) => {
            // Mode serveur : TLS obligatoire, chaque borne s'authentifie par son jeton.
            let tokens = Arc::new(load_tokens(&srv.tokens_file)?);
            let router = router.layer(axum::middleware::from_fn(move |req, next| {
                auth(tokens.clone(), req, next)
            }));
            let tls =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(&srv.tls_cert, &srv.tls_key)
                    .await?;
            log::info!("mode serveur : écoute TLS sur {listen}");
            axum_server::bind_rustls(listen.parse::<std::net::SocketAddr>()?, tls)
                .serve(router.into_make_service())
                .await?;
        }
    }
    Ok(())
}

/// Borne authentifiée (mode serveur).
#[derive(Clone)]
struct Borne(String);

/// `<jeton> <nom de la borne>` par ligne ; lignes vides et `#` ignorées.
fn load_tokens(p: &std::path::Path) -> anyhow::Result<Vec<(String, String)>> {
    let mut v = Vec::new();
    for l in std::fs::read_to_string(p)?.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (t, n) = l
            .split_once(char::is_whitespace)
            .ok_or_else(|| anyhow::anyhow!("ligne de jeton invalide"))?;
        if t.len() < 32 {
            anyhow::bail!("jeton trop court pour {n} (32 caractères minimum)");
        }
        v.push((t.to_string(), n.trim().to_string()));
    }
    Ok(v)
}

/// Comparaison en temps constant (longueurs publiques).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn auth(
    tokens: Arc<Vec<(String, String)>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let given = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or_default();
    let mut found = None;
    for (t, n) in tokens.iter() {
        if same(t.as_bytes(), given.as_bytes()) {
            found = Some(n.clone());
        }
    }
    match found {
        Some(n) => {
            log::debug!("{n} : {} {}", req.method(), req.uri().path());
            req.extensions_mut().insert(Borne(n));
            next.run(req).await
        }
        None => {
            log::warn!(
                "requête refusée (jeton absent ou inconnu) : {}",
                req.uri().path()
            );
            err(StatusCode::UNAUTHORIZED, "jeton absent ou inconnu")
        }
    }
}

/// Identifiant d'utilisateur propre à la borne (mode serveur : les bornes ne se voient pas).
fn scoped(b: &Option<Extension<Borne>>, uid: &str) -> String {
    match b {
        Some(Extension(Borne(n))) => format!("{n}/{uid}"),
        None => uid.to_string(),
    }
}

/// Analyse visible par l'appelant (en mode serveur, seulement celles de sa borne).
fn job_for(app: &App, id: &str, b: &Option<Extension<Borne>>) -> Option<Arc<Job>> {
    let j = job(app, id)?;
    match b {
        Some(Extension(Borne(n))) if !j.uid.starts_with(&format!("{n}/")) => None,
        _ => Some(j),
    }
}

fn err(code: StatusCode, msg: impl ToString) -> Response {
    (code, Json(json!({"error": msg.to_string()}))).into_response()
}

fn job(app: &App, id: &str) -> Option<Arc<Job>> {
    app.jobs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .cloned()
}

/// usbsas envoie le tar de session ; l'analyse démarre en tâche de fond.
async fn upload(
    State(app): State<Shared>,
    b: Option<Extension<Borne>>,
    Path(uid): Path<String>,
    body: Body,
) -> Response {
    let uid = scoped(&b, &uid);
    let work = match tempfile::Builder::new()
        .prefix("scan-")
        .tempdir_in(&app.conf.work_dir)
    {
        Ok(w) => w,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let tar_path = work.path().join("bundle.tar");
    let res: anyhow::Result<()> = async {
        let mut f = tokio::fs::File::create(&tar_path).await?;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            f.write_all(&chunk?).await?;
        }
        f.flush().await?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        return err(StatusCode::BAD_REQUEST, e);
    }
    let job = Arc::new(Job::new(&uid, work));
    {
        // Une seule analyse conservée par borne : la précédente (et ses fichiers) est libérée.
        let mut jobs = app.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.retain(|_, j| j.uid != uid);
        jobs.insert(job.id.clone(), job.clone());
    }
    log::info!("analyse {} pour {uid}", job.id);
    let import = app
        .import_armed
        .swap(false, std::sync::atomic::Ordering::SeqCst)
        .then(|| app.conf.import_dir.clone());
    let (app2, job2) = (app.clone(), job.clone());
    tokio::task::spawn_blocking(move || run_scan(&app2, &job2, &tar_path, import));
    Json(json!({"id": job.id, "status": "uploaded"})).into_response()
}

fn run_scan(app: &App, job: &Job, tar_path: &std::path::Path, import: Option<std::path::PathBuf>) {
    let av = app.engines.clamav_version();
    {
        let mut s = job.lock();
        s.yara = app.engines.yara_enabled();
        if let Ok((v, db, date)) = &av {
            s.antivirus = job::Antivirus {
                version: v.clone(),
                database_version: db.clone(),
                database_timestamp: date.clone(),
            };
        }
    }
    let scanner = scan::Scanner {
        engines: &app.engines,
        extractor: &app.extractor,
        limits: &app.conf.limits,
        job,
        work: job.work.path().to_path_buf(),
        import,
    };
    // Sans antivirus, rien ne passe : l'analyse échoue (usbsas avorte le transfert).
    let res = av.and_then(|_| scanner.run(tar_path));
    let _ = std::fs::remove_file(tar_path);
    let mut s = job.lock();
    s.finished = Some(jiff::Zoned::now());
    match res {
        Ok(entries) => {
            s.entries = entries;
            s.status = Status::Scanned;
        }
        Err(e) => {
            log::error!("analyse {} : {e:#}", job.id);
            s.status = Status::Error;
            s.error = Some(format!("{e:#}"));
        }
    }
    if let Some(dir) = &app.conf.reports_dir
        && let Err(e) = stage::save_report(dir, app.conf.max_reports, job, &s)
    {
        log::error!("rapport local : {e:#}");
    }
    log::info!("analyse {} terminée : {:?}", job.id, s.status);
}

async fn usbsas_result(
    State(app): State<Shared>,
    b: Option<Extension<Borne>>,
    Path((_uid, id)): Path<(String, String)>,
) -> Response {
    match job_for(&app, &id, &b) {
        Some(j) => Json(report::usbsas(&j, &j.lock())).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn progress_json(j: &Job) -> serde_json::Value {
    let s = j.lock();
    json!({
        "id": j.id,
        "status": s.status,
        "error": s.error,
        "done": s.done,
        "total": s.total,
        "scanned": s.scanned,
        "password_request": s.pending,
    })
}

async fn current(
    State(app): State<Shared>,
    b: Option<Extension<Borne>>,
    Path(uid): Path<String>,
) -> Response {
    let uid = scoped(&b, &uid);
    let j = app
        .jobs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .find(|j| j.uid == uid)
        .cloned();
    match j {
        Some(j) => Json(progress_json(&j)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn progress(
    State(app): State<Shared>,
    b: Option<Extension<Borne>>,
    Path(id): Path<String>,
) -> Response {
    match job_for(&app, &id, &b) {
        Some(j) => Json(progress_json(&j)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct PasswordBody {
    password: Option<String>,
    #[serde(default)]
    abandon: bool,
}

async fn password(
    State(app): State<Shared>,
    borne: Option<Extension<Borne>>,
    Path(id): Path<String>,
    Json(b): Json<PasswordBody>,
) -> Response {
    let Some(j) = job_for(&app, &id, &borne) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let a = match (b.abandon, b.password) {
        (false, Some(p)) => Answer::Password(zeroize::Zeroizing::new(p)),
        _ => Answer::Abandon,
    };
    if j.answer(a) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        err(StatusCode::CONFLICT, "aucun mot de passe attendu")
    }
}

async fn detailed(
    State(app): State<Shared>,
    b: Option<Extension<Borne>>,
    Path(id): Path<String>,
) -> Response {
    match job_for(&app, &id, &b) {
        Some(j) => Json(report::detailed(&j, &j.lock())).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct StageBody {
    /// Description de la clé source pour le rapport (n° de série…)
    device: String,
}

/// Prépare le dossier lu par usbsas pour l'écriture sur la clé de sortie.
async fn stage_req(
    State(app): State<Shared>,
    borne: Option<Extension<Borne>>,
    Path(id): Path<String>,
    Json(b): Json<StageBody>,
) -> Response {
    if borne.is_some() {
        return err(StatusCode::FORBIDDEN, "mode serveur : utiliser files.tar");
    }
    let Some(j) = job(&app, &id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let dir = app.conf.stage_dir.clone();
    let res = tokio::task::spawn_blocking(move || stage::stage(&dir, &j, &b.device)).await;
    match res {
        Ok(Ok(n)) => Json(json!({"files": n})).into_response(),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn arm_import(State(app): State<Shared>, b: Option<Extension<Borne>>) -> Response {
    if b.is_some() {
        return err(
            StatusCode::FORBIDDEN,
            "mode serveur : la clé admin est lue par le relais de la borne",
        );
    }
    app.import_armed
        .store(true, std::sync::atomic::Ordering::SeqCst);
    log::info!("prochaine lecture : clé administrateur (paquets de mise à jour)");
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct DeviceQuery {
    device: String,
}

/// Arbre des fichiers sains + rapports, en tar (relais d'une borne en mode serveur).
async fn files_tar(
    State(app): State<Shared>,
    b: Option<Extension<Borne>>,
    Path(id): Path<String>,
    Query(q): Query<DeviceQuery>,
) -> Response {
    let Some(j) = job_for(&app, &id, &b) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = j
        .work
        .path()
        .join(format!("files-{}.tar", uuid::Uuid::new_v4()));
    let p2 = path.clone();
    match tokio::task::spawn_blocking(move || stage::write_tar(&p2, &j, &q.device)).await {
        Ok(Ok(())) => (),
        Ok(Err(e)) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
    match tokio::fs::File::open(&path).await {
        Ok(f) => {
            // Le fichier reste lisible après suppression du nom (descripteur ouvert).
            let _ = tokio::fs::remove_file(&path).await;
            Response::builder()
                .header(axum::http::header::CONTENT_TYPE, "application/x-tar")
                .body(Body::from_stream(tokio_util::io::ReaderStream::new(f)))
                .unwrap_or_else(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn status(State(app): State<Shared>) -> Response {
    let av = app.engines.clamav_version();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "clamav": av.as_ref().ok().map(|(v, db, date)| json!({"version": v, "database_version": db, "database_timestamp": date})),
        "clamav_error": av.err().map(|e| format!("{e:#}")),
        "yara": app.engines.yara_enabled(),
        "limits": {
            "max_depth": app.conf.limits.max_depth,
            "max_files": app.conf.limits.max_files,
            "max_total_size": app.conf.limits.max_total_size,
        },
    }))
    .into_response()
}
