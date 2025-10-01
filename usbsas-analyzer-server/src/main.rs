//! Very basic remote analyse / upload / download server for `usbsas` using `clamav`.
//! Mainly used for example and integration tests.

use actix_files::NamedFile;
use actix_web::{get, head, http::header, post, web, App, HttpResponse, HttpServer, Responder};
use clap::{Arg, Command};
use futures::TryStreamExt;
use serde_json::json;
use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Seek, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process,
    sync::Mutex,
    thread,
};
use tar::{Archive, EntryType};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;

const TAR_DATA_DIR: &str = "data/";

struct AnalyzeStatus {
    status: String,
    files: HashMap<String, String>,
}

struct AppState {
    working_dir: Mutex<String>,
    current_scans: Mutex<HashMap<String, AnalyzeStatus>>,
    clamav: Mutex<Clamav>,
}

impl AppState {
    fn analyze(&self, bundle_id: &str, tar: &str) -> Result<(), actix_web::Error> {
        let tmpdir = tempfile::Builder::new()
            .prefix(&bundle_id)
            .tempdir_in(PathBuf::from(&*self.working_dir.lock().unwrap()))?;
        let mut archive = Archive::new(fs::File::open(tar)?);

        let mut entries_paths = Vec::new();

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry
                .path()?
                .to_path_buf()
                .into_os_string()
                .into_string()
                .unwrap();
            if !path.starts_with(TAR_DATA_DIR) {
                continue;
            }
            if matches!(
                entry.header().entry_type(),
                EntryType::Regular | EntryType::Directory | EntryType::Symlink
            ) {
                entry.unpack_in(tmpdir.path())?;
            }
            if matches!(
                entry.header().entry_type(),
                EntryType::Regular | EntryType::Symlink
            ) {
                entries_paths.push(path.strip_prefix(TAR_DATA_DIR).unwrap().to_string());
            }
        }

        if entries_paths.is_empty() {
            log::error!("no files to analyze");
            return Err(actix_web::error::ErrorNotFound(io::Error::new(
                io::ErrorKind::NotFound,
                "tar's \"data\" is empty or absent, no files to analyze",
            )));
        }

        let base_path = tmpdir
            .path()
            .join(TAR_DATA_DIR)
            .into_os_string()
            .into_string()
            .unwrap();
        let mut dirty_paths = self.clamav.lock().unwrap().analyze(&base_path)?;

        dirty_paths.iter_mut().for_each(|path| {
            *path = path.strip_prefix(base_path.as_str()).unwrap().to_string();
        });

        let mut current_scans = self.current_scans.lock().unwrap();
        let current_scan = current_scans.get_mut(bundle_id).unwrap();

        entries_paths.iter().for_each(|path| {
            if dirty_paths.contains(path) {
                current_scan
                    .files
                    .insert(path.to_owned(), "DIRTY".to_string());
            } else {
                current_scan
                    .files
                    .insert(path.to_owned(), "CLEAN".to_string());
            };
        });

        current_scan.status = "scanned".to_string();
        Ok(())
    }

    async fn recv_file(
        &self,
        mut body: web::Payload,
    ) -> Result<(String, String), actix_web::Error> {
        #[cfg(not(feature = "integration-tests"))]
        let bundle_id = uuid::Uuid::new_v4().simple().to_string();
        #[cfg(feature = "integration-tests")]
        let bundle_id = "bundle_test".into();
        let out_file_name = format!("{}/{}.tar", self.working_dir.lock().unwrap(), bundle_id);
        let mut out_file = tokio::fs::File::create(&out_file_name).await?;

        while let Some(bytes) = body.try_next().await? {
            out_file.write_all(&bytes).await?;
        }
        out_file.flush().await?;
        Ok((bundle_id, out_file_name))
    }
}

#[post("/api/scanbundle/{id}")]
async fn scan_bundle(
    body: web::Payload,
    _id: web::Path<String>,
    data: web::Data<AppState>,
) -> Result<impl Responder, actix_web::Error> {
    let (bundle_id, out_file_name) = data.recv_file(body).await?;

    data.current_scans.lock().unwrap().insert(
        bundle_id.clone(),
        AnalyzeStatus {
            status: "processing".to_string(),
            files: HashMap::new(),
        },
    );

    let bundle_id_clone = bundle_id.clone();
    thread::spawn(move || {
        if let Err(err) = data.analyze(&bundle_id_clone, &out_file_name) {
            log::error!("{err}");
            if let Some(status) = data.current_scans.lock().unwrap().get_mut(&bundle_id_clone) {
                status.status = "error".to_string();
            };
        }
    });

    Ok(HttpResponse::Ok().json(json!(
        {
            "id": bundle_id,
            "status": "uploaded"
        }
    )))
}

#[get("/api/scanbundle/{id}/{bundle_id}")]
async fn scan_result(
    params: web::Path<(String, String)>,
    data: web::Data<AppState>,
) -> Result<impl Responder, actix_web::Error> {
    let (_, bundle_id) = params.into_inner();
    let mut current_scans = data.current_scans.lock().unwrap();
    if current_scans.contains_key(&bundle_id) {
        let rep = if current_scans[&bundle_id].status == "scanned"
            || current_scans[&bundle_id].status == "error"
        {
            let entry = current_scans.remove(&bundle_id).unwrap();
            #[cfg(not(feature = "integration-tests"))]
            let av_infos = {
                let (clam_ver, db_ver, db_date) = data.clamav.lock().unwrap().version()?;
                json!({
                    "ClamAV": {
                    "version": clam_ver,
                    "database_version": db_ver,
                    "database_timestamp": db_date,
                    }
                })
            };
            // Fixed timestamp to keep a determistic filesystem hash
            #[cfg(feature = "integration-tests")]
            let av_infos = json!({
                "ClamAV": {
                    "version": "946767600",
                    "database_version": "946767600",
                    "database_timestamp": "946767600",
                }
            });
            fs::remove_file(format!(
                "{}/{}.tar",
                data.working_dir.lock().unwrap(),
                bundle_id
            ))?;
            #[cfg(feature = "integration-tests")]
            let bundle_id = "0";
            let files_status: HashMap<String, serde_json::Value> = entry
                .files
                .iter()
                .map(|(key, val)| (key.clone(), json!({ "status": val })))
                .collect();
            json!({
                "id": bundle_id,
                "status": entry.status,
                "version": 2,
                "files": files_status,
                "antivirus": av_infos
            })
        } else {
            json!({
                "id": bundle_id,
                "status": current_scans[&bundle_id].status,
            })
        };
        Ok(HttpResponse::Ok().json(rep))
    } else {
        Ok(HttpResponse::NotFound().finish())
    }
}

#[post("/api/uploadbundle/{id}")]
async fn upload_bundle(
    body: web::Payload,
    _id: web::Path<String>,
    data: web::Data<AppState>,
) -> Result<impl Responder, actix_web::Error> {
    let (_, _) = data.recv_file(body).await?;
    Ok(HttpResponse::Ok())
}

fn find_bundle(filename: &str) -> Result<(String, u64), actix_web::Error> {
    for ext in ["tar", "tar.gz", "gz"] {
        let bundle_path = format!("{filename}.{ext}");
        if let Ok(metadata) = fs::metadata(&bundle_path) {
            return Ok((bundle_path, metadata.len()));
        }
    }
    Err(actix_web::error::ErrorNotFound(io::Error::new(
        io::ErrorKind::NotFound,
        "Bundle not found",
    )))
}

#[head("/api/downloadbundle/{id}/{bundle_id}")]
async fn head_bundle_size(
    params: web::Path<(String, String)>,
    data: web::Data<AppState>,
) -> Result<impl Responder, actix_web::Error> {
    let (id, bundle_id) = params.into_inner();
    let (bundle_path, mut size) = find_bundle(&format!(
        "{}/{}/{}",
        data.working_dir.lock().unwrap(),
        id,
        bundle_id
    ))?;

    // usbsas expects uncompressed size of files with HEAD requests

    // XXX FIXME Dirty hack 1:
    // /!\ The following only work if the gzipped tar is < 4GB (as it's stored
    // %4GB), but good enough for this integration tests server
    if bundle_path.ends_with("gz") {
        let mut f = fs::File::open(&bundle_path)?;
        f.seek(io::SeekFrom::End(-4))?;
        let mut buf = vec![0; 4];
        f.read_exact(&mut buf)?;
        size = u32::from_ne_bytes(buf[0..4].try_into().unwrap()) as u64;
        log::debug!("filename: {bundle_path}, uncompressed size: {size}");
    }
    log::debug!("filename: {bundle_path}, size: {size}");

    Ok(HttpResponse::Ok()
        .insert_header(("X-Uncompressed-Content-Length", size))
        .finish())
}

#[get("/api/downloadbundle/{id}/{bundle_id}")]
async fn download_bundle(
    params: web::Path<(String, String)>,
    data: web::Data<AppState>,
) -> Result<impl Responder, actix_web::Error> {
    let (id, bundle_id) = params.into_inner();
    let (bundle_path, _) = find_bundle(&format!(
        "{}/{}/{}",
        data.working_dir.lock().unwrap(),
        id,
        bundle_id
    ))?;
    let named_file = if bundle_path.ends_with("gz") {
        NamedFile::open(bundle_path)?.set_content_encoding(header::ContentEncoding::Gzip)
    } else {
        NamedFile::open(bundle_path)?
    };
    Ok(named_file)
}

#[get("/shutdown")]
async fn shutdown(data: web::Data<AppState>) -> Result<impl Responder, actix_web::Error> {
    data.clamav.lock().unwrap().cmd("SHUTDOWN")?;
    Ok(HttpResponse::Ok())
}

struct Clamav {
    _process: process::Child,
    socket_path: String,
}

impl Clamav {
    fn new(working_path: &str) -> io::Result<Self> {
        let clamav_config_path = format!("{working_path}/clamd.conf");
        let clamav_socket_path = format!("{working_path}/clamd.socket");

        #[cfg(not(feature = "integration-tests"))]
        std::fs::write(
            &clamav_config_path,
            format!(
                "TemporaryDirectory {working_path}\nLocalSocket {clamav_socket_path}\nForeground true\n",
            ),
        )?;

        // Use dummy db for tests (faster loading, only detects eicar)
        #[cfg(feature = "integration-tests")]
        {
            std::fs::write(
                &clamav_config_path,
                format!(
                    "TemporaryDirectory {working_path}\nLocalSocket {clamav_socket_path}\nForeground true\nDatabaseDirectory {working_path}/db\n",
                ),
            )?;
            let _ = fs::create_dir(&format!("{working_path}/db"));
            std::fs::write(
                &format!("{working_path}/db/test.ndb"),
                "Eicar-Test-Signature:0:0:58354f2150254041505b345c505a58353428505e2937434329377d2445494341522d5354414e444152442d414e544956495255532d544553542d46494c452124482b482a\nEicar-Test-Signature-1:0:*:574456504956416c51454651577a5263554670594e54516f554634704e304e444b5464394a45564a513046534c564e555155354551564a454c55464f56456c5753564a565579315552564e550a4c555a4a544555684a45677253436f3d0a"
            )?;
        }

        log::debug!("start clamd");
        let clamav_child = process::Command::new("clamd")
            .args(["--config", &clamav_config_path])
            .spawn()?;

        // Try opening the socket, this may take some times as the socket will
        // only be created by clamd when the database is loaded.
        let mut timeout_retry = 60;
        let mut clamav_socket = loop {
            log::trace!("attempt to connect to clamd socket");
            match UnixStream::connect(&clamav_socket_path) {
                Ok(socket) => break socket,
                Err(err) => match err.kind() {
                    std::io::ErrorKind::NotFound => {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        timeout_retry -= 1;
                        if timeout_retry == 0 {
                            return Err(err);
                        } else {
                            continue;
                        }
                    }
                    _ => return Err(err),
                },
            }
        };

        let mut response = String::new();
        clamav_socket.write_all(b"PING")?;
        clamav_socket.read_to_string(&mut response)?;

        if response.trim() != "PONG" {
            log::error!("{response:#?}");
            return Err(io::Error::other("Couldn't connect to clamd socket"));
        }

        log::debug!("clamd ping pong ok");

        Ok(Clamav {
            _process: clamav_child,
            socket_path: clamav_socket_path,
        })
    }

    fn cmd(&mut self, cmd: &str) -> io::Result<String> {
        let mut socket = UnixStream::connect(&self.socket_path)?;
        let mut response = String::new();
        socket.write_all(cmd.as_bytes())?;
        socket.read_to_string(&mut response)?;
        Ok(response)
    }

    #[cfg(not(feature = "integration-tests"))]
    fn version(&mut self) -> io::Result<(String, String, String)> {
        let version = self.cmd("VERSION")?;
        let versions: Vec<&str> = version.trim().split('/').collect();
        let (clam_ver, db_ver, db_date) = (
            versions[0].to_string(),
            versions[1].to_string(),
            versions[2].to_string(),
        );
        Ok((clam_ver, db_ver, db_date))
    }

    fn analyze(&mut self, path: &str) -> io::Result<Vec<String>> {
        let response = self.cmd(&format!("CONTSCAN {path}"))?;
        log::debug!("{response:#?}");
        let mut dirty = Vec::new();
        if response.ends_with("OK\n") {
            return Ok(dirty);
        }
        for line in response.lines() {
            match line.rfind(": ") {
                Some(index) => dirty.push(line.split_at(index).0.to_string()),
                None => continue,
            }
        }
        Ok(dirty)
    }
}

#[actix_web::main]
async fn main() -> io::Result<()> {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "info"));

    let command = Command::new("usbsas-analyzer-server")
        .about("simple usbsas remote server for integrations tests")
        .version("0.2")
        .arg(
            Arg::new("working-dir")
                .value_name("WORKING-DIR")
                .short('d')
                .long("working-dir")
                .num_args(1)
                .required(false),
        );

    let matches = command.get_matches();

    let (working_path, _temp_dir): (String, Option<TempDir>) =
        if let Some(wp) = matches.get_one::<String>("working-dir") {
            (wp.to_owned(), None)
        } else {
            let tmpdir = tempfile::Builder::new()
                .prefix("usbsas-analyzer")
                .tempdir_in("/tmp")
                .unwrap();
            let working_path = tmpdir.path().to_string_lossy().to_string();
            (working_path, Some(tmpdir))
        };

    let clamav = Clamav::new(&working_path)?;
    let app_data = web::Data::new(AppState {
        working_dir: Mutex::new(working_path),
        current_scans: Mutex::new(HashMap::new()),
        clamav: Mutex::new(clamav),
    });
    HttpServer::new(move || {
        App::new()
            .app_data(app_data.clone())
            .service(scan_bundle)
            .service(scan_result)
            .service(upload_bundle)
            .service(head_bundle_size)
            .service(download_bundle)
            .service(shutdown)
    })
    .bind("127.0.0.1:8042")?
    .run()
    .await
}
