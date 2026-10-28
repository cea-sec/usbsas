//! Asynchronous analysis of bundles.
//!
//! A bundle submitted with `POST /api/scanbundle/<user id>` is stored in
//! `WORKING-DIR/.scans/<bundle id>.tar` and registered in the [`Scanner`] as
//! `processing`. A background task then:
//!
//! 1. waits for a slot (at most `--max-concurrent-scans` analyses run at
//!    the same time, the others stay `processing`);
//! 2. extracts the regular files found under `data/` in the tar into a
//!    private directory. Each file is written under a generated name (its
//!    index in the archive): the original name is only kept in memory. This
//!    makes the extraction immune to path traversal, odd file names (newlines,
//!    non-printable characters, ...), restrictive permissions stored in the
//!    archive, and makes clamd's answer unambiguous to parse;
//! 3. scans that directory with clamd and maps the results back to the
//!    original paths;
//! 4. removes the tar and the extracted files, and stores the report.
//!
//! The report is returned (and forgotten) by the first
//! `GET /api/scanbundle/<user id>/<bundle id>` once the analysis is done.
//! Reports never fetched are dropped after `--result-ttl` seconds.
//!
//! Failure policy (fail closed): a file clamd reports as infected *or* that
//! it couldn't scan is `DIRTY`. If the archive can't be read, contains no
//! file, or clamd fails as a whole, the analysis status is `error`.

use crate::{
    clamav::Clamav,
    error::{Error, Result},
    storage::{self, RemoveOnDrop, Storage},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{self, BufReader},
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};
use tar::{Archive, EntryType};
use tokio::sync::Semaphore;

/// Only entries under this directory of the tar are analyzed.
const TAR_DATA_DIR: &str = "data/";
/// Version of the report format.
const REPORT_VERSION: u32 = 2;

/// Analysis status of one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileStatus {
    Clean,
    Dirty,
}

impl FileStatus {
    fn as_str(self) -> &'static str {
        match self {
            FileStatus::Clean => "CLEAN",
            FileStatus::Dirty => "DIRTY",
        }
    }
}

/// Final result of an analysis.
#[derive(Debug)]
struct Report {
    /// `true` if scanned successfully, `false` for `error`.
    success: bool,
    files: BTreeMap<String, FileStatus>,
    antivirus: Value,
}

#[derive(Debug)]
enum JobState {
    Processing,
    Done(Report),
}

#[derive(Debug)]
struct Job {
    /// User who submitted the bundle: only they can get the result.
    user_id: String,
    state: JobState,
    /// Last state change, used to expire unclaimed reports.
    updated: Instant,
}

/// Registry of analyses, shared by all the HTTP workers.
pub struct Scanner {
    storage: Arc<Storage>,
    clamav: Arc<Clamav>,
    jobs: Mutex<HashMap<String, Job>>,
    slots: Semaphore,
    result_ttl: Duration,
}

impl Scanner {
    pub fn new(
        storage: Arc<Storage>,
        clamav: Arc<Clamav>,
        max_concurrent_scans: usize,
        result_ttl: Duration,
    ) -> Self {
        Scanner {
            storage,
            clamav,
            jobs: Mutex::new(HashMap::new()),
            slots: Semaphore::new(max_concurrent_scans.max(1)),
            result_ttl,
        }
    }

    fn jobs(&self) -> MutexGuard<'_, HashMap<String, Job>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Receive a bundle and start its analysis in the background.
    ///
    /// Returns the bundle id once the bundle is completely received.
    pub async fn submit(
        self: &Arc<Self>,
        user_id: &str,
        body: actix_web::web::Payload,
        content_length: Option<u64>,
    ) -> Result<String> {
        let bundle_id = storage::new_bundle_id();
        let tar_path = self.storage.scan_tar_path(&bundle_id);
        let size = self
            .storage
            .receive(body, content_length, &tar_path)
            .await?;
        log::info!("[{user_id}] bundle {bundle_id} received for analysis ({size} bytes)");

        self.jobs().insert(
            bundle_id.clone(),
            Job {
                user_id: user_id.to_string(),
                state: JobState::Processing,
                updated: Instant::now(),
            },
        );

        let scanner = Arc::clone(self);
        let (user_id, id) = (user_id.to_string(), bundle_id.clone());
        actix_web::rt::spawn(async move { scanner.run(user_id, id).await });
        Ok(bundle_id)
    }

    async fn run(self: Arc<Self>, user_id: String, bundle_id: String) {
        let tar_path = self.storage.scan_tar_path(&bundle_id);
        let _remove_tar = RemoveOnDrop(Some(tar_path.clone()));

        let report = match self.slots.acquire().await {
            Ok(_permit) => {
                let scanner = Arc::clone(&self);
                let bundle = bundle_id.clone();
                let start = Instant::now();
                let res = tokio::task::spawn_blocking(move || {
                    analyze(&scanner.storage, &scanner.clamav, &bundle, &tar_path)
                })
                .await;
                match res {
                    Ok(Ok(files)) => {
                        let dirty = files.values().filter(|s| **s == FileStatus::Dirty).count();
                        log::info!(
                            "[{user_id}] bundle {bundle_id} scanned in {:.1?}: {} file(s), {dirty} dirty",
                            start.elapsed(),
                            files.len()
                        );
                        Report {
                            success: true,
                            files,
                            antivirus: self.antivirus_infos().await,
                        }
                    }
                    Ok(Err(err)) => {
                        log::error!("[{user_id}] bundle {bundle_id} analysis failed: {err}");
                        self.error_report().await
                    }
                    Err(err) => {
                        log::error!("[{user_id}] bundle {bundle_id} analysis panicked: {err}");
                        self.error_report().await
                    }
                }
            }
            Err(_) => {
                log::error!("[{user_id}] bundle {bundle_id}: scanner closed");
                self.error_report().await
            }
        };

        if let Some(job) = self.jobs().get_mut(&bundle_id) {
            job.state = JobState::Done(report);
            job.updated = Instant::now();
        }
    }

    async fn error_report(&self) -> Report {
        Report {
            success: false,
            files: BTreeMap::new(),
            antivirus: self.antivirus_infos().await,
        }
    }

    /// Antivirus information included in the reports.
    async fn antivirus_infos(&self) -> Value {
        // Fixed values to keep a deterministic filesystem hash in tests
        #[cfg(feature = "integration-tests")]
        let (version, database_version, database_timestamp) = (
            "946767600".to_string(),
            "946767600".to_string(),
            "946767600".to_string(),
        );
        #[cfg(not(feature = "integration-tests"))]
        let (version, database_version, database_timestamp) = {
            let clamav = Arc::clone(&self.clamav);
            match tokio::task::spawn_blocking(move || clamav.version()).await {
                Ok(Ok(v)) => (v.clamav, v.database_version, v.database_timestamp),
                Ok(Err(err)) => {
                    log::warn!("couldn't get clamav version: {err}");
                    Default::default()
                }
                Err(err) => {
                    log::warn!("couldn't get clamav version: {err}");
                    Default::default()
                }
            }
        };
        json!({
            "ClamAV": {
                "version": version,
                "database_version": database_version,
                "database_timestamp": database_timestamp,
            }
        })
    }

    /// Status of an analysis.
    ///
    /// While processing: `{"id": ..., "status": "processing"}`. Once done,
    /// the full report is returned and the analysis is forgotten.
    pub fn poll(&self, user_id: &str, bundle_id: &str) -> Result<Value> {
        let mut jobs = self.jobs();
        let job = match jobs.get(bundle_id) {
            Some(job) if job.user_id == user_id => job,
            _ => return Err(Error::NotFound("analysis")),
        };
        if matches!(job.state, JobState::Processing) {
            return Ok(json!({ "id": bundle_id, "status": "processing" }));
        }
        let Some(Job {
            state: JobState::Done(report),
            ..
        }) = jobs.remove(bundle_id)
        else {
            unreachable!("checked above");
        };
        log::debug!("[{user_id}] report of bundle {bundle_id} delivered");

        #[cfg(feature = "integration-tests")]
        let bundle_id = "0";
        let files: BTreeMap<&String, Value> = report
            .files
            .iter()
            .map(|(path, status)| (path, json!({ "status": status.as_str() })))
            .collect();
        let status = if report.success { "scanned" } else { "error" };
        Ok(json!({
            "id": bundle_id,
            "status": status,
            "version": REPORT_VERSION,
            "files": files,
            "antivirus": report.antivirus,
        }))
    }

    /// Drop reports that were not fetched in time.
    pub fn reap(&self) {
        let ttl = self.result_ttl;
        self.jobs().retain(|bundle_id, job| {
            let expired = matches!(job.state, JobState::Done(_)) && job.updated.elapsed() > ttl;
            if expired {
                log::warn!(
                    "[{}] report of bundle {bundle_id} never fetched, dropping it",
                    job.user_id
                );
            }
            !expired
        });
    }
}

/// Extract and scan a bundle (blocking).
fn analyze(
    storage: &Storage,
    clamav: &Clamav,
    bundle_id: &str,
    tar_path: &Path,
) -> io::Result<BTreeMap<String, FileStatus>> {
    let workdir = tempfile::Builder::new()
        .prefix(&format!("{bundle_id}."))
        .tempdir_in(storage.scans_dir())?;
    let files_dir = workdir.path().join("files");
    fs::create_dir(&files_dir)?;

    // (original path, name of the extracted file if any)
    let mut entries: Vec<(String, Option<String>)> = Vec::new();
    let mut archive = Archive::new(BufReader::with_capacity(
        1024 * 1024,
        fs::File::open(tar_path)?,
    ));
    for (index, entry) in archive.entries()?.enumerate() {
        let mut entry = entry?;
        let path = {
            let raw_path = entry.path()?;
            raw_path
                .to_str()
                .ok_or_else(|| io::Error::other(format!("non UTF-8 path in tar: {raw_path:?}")))?
                .to_string()
        };
        let Some(rel_path) = path.strip_prefix(TAR_DATA_DIR) else {
            continue;
        };
        if rel_path.is_empty() {
            continue;
        }
        let rel_path = rel_path.to_string();
        let entry_type = entry.header().entry_type();
        match entry_type {
            EntryType::Regular => {
                let name = index.to_string();
                let mut out = fs::File::create(files_dir.join(&name))?;
                io::copy(&mut entry, &mut out)?;
                entries.push((rel_path, Some(name)));
            }
            // Nothing to scan in a symlink (and clamd doesn't follow them),
            // but report it like the other files.
            EntryType::Symlink => entries.push((rel_path, None)),
            EntryType::Directory => (),
            other => log::warn!("bundle {bundle_id}: ignoring {rel_path:?} (type {other:?})"),
        }
    }

    if entries.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "tar's \"data\" is empty or absent, no files to analyze",
        ));
    }

    let outcome = if entries.iter().any(|(_, name)| name.is_some()) {
        clamav.contscan(&files_dir)?
    } else {
        Default::default()
    };

    let mut unscannable: HashMap<&str, &str> = HashMap::new();
    for (name, err) in &outcome.errors {
        match name {
            Some(name) => {
                unscannable.insert(name, err);
            }
            None => return Err(io::Error::other(format!("clamd error: {err}"))),
        }
    }

    let mut files = BTreeMap::new();
    for (path, name) in entries {
        let status = match name.as_deref() {
            Some(name) if outcome.infected.contains_key(name) => {
                log::warn!(
                    "bundle {bundle_id}: {path:?} is infected ({})",
                    outcome.infected[name]
                );
                FileStatus::Dirty
            }
            Some(name) if unscannable.contains_key(name) => {
                log::warn!(
                    "bundle {bundle_id}: {path:?} couldn't be scanned ({}), marking it dirty",
                    unscannable[name]
                );
                FileStatus::Dirty
            }
            _ => FileStatus::Clean,
        };
        // If a path appears several times in the archive, dirty wins.
        let current = files.entry(path).or_insert(status);
        if status == FileStatus::Dirty {
            *current = FileStatus::Dirty;
        }
    }
    Ok(files)
}
