//! Management of the `clamd` daemon and client side of its protocol.
//!
//! The server starts its own `clamd` instance (in the foreground, with a
//! configuration file generated in `WORKING-DIR/.clamd/`) and talks to it
//! through a local Unix socket.
//!
//! A supervisor thread owns the `clamd` process: it starts it, waits until
//! its database is loaded, and restarts it if it dies unexpectedly. `clamd` is
//! started with `PR_SET_PDEATHSIG` so it doesn't outlive the server even if
//! the server is killed with `SIGKILL` (the supervisor thread lives as long as
//! the process, which matters because the death signal is tied to the thread
//! that forked).
//!
//! Every command opens a new connection to `clamd`, which serves them
//! concurrently with its own thread pool (see `MaxThreads` in clamd.conf), so
//! no lock is held while a scan runs and several bundles can be analyzed in
//! parallel.

use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Write},
    os::unix::{net::UnixStream, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// `PATH` given to clamd (and used to look it up when `--clamd` is not an
/// absolute path).
const CLAMD_PATH_ENV: &str = "/bin:/sbin:/usr/bin:/usr/sbin:/usr/local/bin:/usr/local/sbin";
/// Timeout of short commands (PING, VERSION, SHUTDOWN).
const CMD_TIMEOUT: Duration = Duration::from_secs(30);
/// Interval between two health checks of clamd.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(2);
/// Max length of a Unix socket path (`sun_path`).
const SUN_PATH_MAX: usize = 107;

/// Options used to start clamd.
#[derive(Debug, Clone)]
pub struct ClamdOptions {
    /// clamd binary.
    pub binary: PathBuf,
    /// Directory for the configuration file, socket and temporary files.
    pub dir: PathBuf,
    /// Additional configuration file appended to the generated one.
    pub extra_config: Option<PathBuf>,
    /// How long to wait for clamd to be ready (database loading).
    pub start_timeout: Duration,
    /// Timeout of a scan command.
    pub scan_timeout: Option<Duration>,
}

/// Result of a `CONTSCAN` command on a directory.
///
/// Files are identified by their path relative to the scanned directory.
#[derive(Debug, Default)]
pub struct ScanOutcome {
    /// Infected files and the name of the detected signature.
    pub infected: HashMap<String, String>,
    /// Files clamd couldn't scan (`None` for the scanned directory itself),
    /// with its error message.
    pub errors: Vec<(Option<String>, String)>,
}

/// Version information returned by the `VERSION` command.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "integration-tests", allow(dead_code))]
pub struct Version {
    pub clamav: String,
    pub database_version: String,
    pub database_timestamp: String,
}

/// Handle on the clamd instance.
pub struct Clamav {
    opts: ClamdOptions,
    config_path: PathBuf,
    socket_path: PathBuf,
    child: Mutex<Option<Child>>,
    /// Set when clamd is (re)starting or dead, cleared once it answers PING.
    ready: Mutex<bool>,
    ready_cond: Condvar,
    stopping: AtomicBool,
}

impl Clamav {
    /// Write clamd's configuration, start it and wait until it is ready.
    ///
    /// A supervisor thread keeps clamd running until [`Clamav::shutdown`] is
    /// called.
    pub fn start(opts: ClamdOptions) -> io::Result<Arc<Self>> {
        let dir = &opts.dir;
        fs::create_dir_all(dir.join("tmp"))?;
        let config_path = dir.join("clamd.conf");
        let socket_path = dir.join("clamd.sock");
        if socket_path.as_os_str().len() > SUN_PATH_MAX {
            return Err(io::Error::other(format!(
                "clamd socket path {} is too long, use a shorter working directory",
                socket_path.display()
            )));
        }
        write_config(&opts, &config_path, &socket_path)?;

        let clamav = Arc::new(Clamav {
            opts,
            config_path,
            socket_path,
            child: Mutex::new(None),
            ready: Mutex::new(false),
            ready_cond: Condvar::new(),
            stopping: AtomicBool::new(false),
        });

        // The supervisor spawns clamd (so its death signal is tied to a
        // thread that lives as long as the process) and reports the result
        // of the first start through this channel.
        let (tx, rx) = std::sync::mpsc::channel();
        let supervisor = Arc::clone(&clamav);
        thread::Builder::new()
            .name("clamd-supervisor".into())
            .spawn(move || supervisor.supervise(tx))?;
        rx.recv()
            .map_err(|_| io::Error::other("clamd supervisor died"))??;
        Ok(clamav)
    }

    fn supervise(&self, first_start: std::sync::mpsc::Sender<io::Result<()>>) {
        let res = self.spawn_and_wait();
        let ok = res.is_ok();
        let _ = first_start.send(res);
        if !ok {
            return;
        }
        let mut backoff = WATCHDOG_INTERVAL;
        loop {
            thread::sleep(WATCHDOG_INTERVAL);
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            let exited = match self.child().as_mut().map(Child::try_wait) {
                Some(Ok(None)) => None,
                Some(Ok(Some(status))) => Some(status.to_string()),
                Some(Err(err)) => Some(err.to_string()),
                None => Some("not running".into()),
            };
            let Some(reason) = exited else {
                backoff = WATCHDOG_INTERVAL;
                // A client may have flagged clamd as unreachable while it is
                // in fact running (transient error): check it again.
                if !self.is_ready()
                    && self
                        .raw_command("PING", Some(CMD_TIMEOUT))
                        .is_ok_and(|resp| resp.trim() == "PONG")
                {
                    self.set_ready(true);
                }
                continue;
            };
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            log::error!("clamd exited unexpectedly ({reason}), restarting it");
            self.set_ready(false);
            if let Err(err) = self.spawn_and_wait() {
                log::error!("couldn't restart clamd: {err}");
                thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }

    fn spawn_and_wait(&self) -> io::Result<()> {
        // A stale socket from a previous (killed) instance would make the
        // readiness check below connect to nothing.
        let _ = fs::remove_file(&self.socket_path);

        log::info!("starting clamd ({})", self.opts.binary.display());
        let mut cmd = Command::new(&self.opts.binary);
        cmd.env("PATH", CLAMD_PATH_ENV)
            .arg(format!("--config-file={}", self.config_path.display()))
            .stdin(Stdio::null());
        // SAFETY: prctl is async-signal-safe, nothing is allocated here.
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("couldn't run {}: {err}", self.opts.binary.display()),
            )
        })?;
        *self.child() = Some(child);

        // The socket only appears once the database is loaded, which can
        // take a while.
        let deadline = Instant::now() + self.opts.start_timeout;
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                return Err(io::Error::other("stopping"));
            }
            if let Some(status) = self.child().as_mut().and_then(|c| c.try_wait().ok()?) {
                return Err(io::Error::other(format!(
                    "clamd exited during startup: {status}"
                )));
            }
            match self.raw_command("PING", Some(CMD_TIMEOUT)) {
                Ok(resp) if resp.trim() == "PONG" => break,
                Ok(resp) => {
                    return Err(io::Error::other(format!(
                        "unexpected answer to PING: {resp:?}"
                    )));
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    if Instant::now() > deadline {
                        self.kill();
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timeout waiting for clamd to be ready",
                        ));
                    }
                    thread::sleep(Duration::from_millis(500));
                }
                Err(err) => return Err(err),
            }
        }
        log::info!("clamd is ready");
        self.set_ready(true);
        Ok(())
    }

    fn child(&self) -> std::sync::MutexGuard<'_, Option<Child>> {
        self.child.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_ready(&self) -> bool {
        *self.ready.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_ready(&self, ready: bool) {
        *self.ready.lock().unwrap_or_else(PoisonError::into_inner) = ready;
        self.ready_cond.notify_all();
    }

    /// Block until clamd is ready (e.g. while it is being restarted), for at
    /// most the startup timeout.
    fn wait_ready(&self) -> io::Result<()> {
        let guard = self.ready.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = self
            .ready_cond
            .wait_timeout_while(guard, self.opts.start_timeout, |ready| {
                !*ready && !self.stopping.load(Ordering::SeqCst)
            })
            .unwrap_or_else(PoisonError::into_inner);
        if *guard {
            Ok(())
        } else {
            Err(io::Error::other("clamd is not available"))
        }
    }

    /// Send a command (without the `n` prefix) and read the whole answer.
    fn raw_command(&self, cmd: &str, timeout: Option<Duration>) -> io::Result<String> {
        let mut socket = UnixStream::connect(&self.socket_path)?;
        socket.set_write_timeout(Some(CMD_TIMEOUT))?;
        socket.set_read_timeout(timeout)?;
        // "n" prefix: newline delimited command and answer.
        socket.write_all(format!("n{cmd}\n").as_bytes())?;
        let mut response = String::new();
        socket.read_to_string(&mut response)?;
        Ok(response)
    }

    /// Send a command to clamd once it is ready.
    ///
    /// If clamd can't be reached (it died and the supervisor didn't notice
    /// yet), wait for it to be restarted and retry once.
    pub fn command(&self, cmd: &str, timeout: Option<Duration>) -> io::Result<String> {
        if self.is_stopping() {
            return Err(io::Error::other("clamd is shutting down"));
        }
        self.wait_ready()?;
        match self.raw_command(cmd, timeout) {
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) && !self.is_stopping() =>
            {
                log::warn!("clamd unreachable ({err}), waiting for it");
                self.set_ready(false);
                self.wait_ready()?;
                self.raw_command(cmd, timeout)
            }
            res => res,
        }
    }

    /// Versions of clamd and of its database.
    ///
    /// The answer is `ClamAV <version>/<db version>/<db date>`, or only
    /// `ClamAV <version>` with some databases: missing parts are empty.
    #[cfg_attr(feature = "integration-tests", allow(dead_code))]
    pub fn version(&self) -> io::Result<Version> {
        let response = self.command("VERSION", Some(CMD_TIMEOUT))?;
        let mut parts = response.trim().splitn(3, '/');
        let mut next = || parts.next().unwrap_or_default().to_string();
        Ok(Version {
            clamav: next(),
            database_version: next(),
            database_timestamp: next(),
        })
    }

    /// Recursively scan a directory with `CONTSCAN` (continue after the
    /// first infected file to report all of them).
    ///
    /// The path must not contain a newline.
    pub fn contscan(&self, dir: &Path) -> io::Result<ScanOutcome> {
        let dir_str = dir
            .to_str()
            .filter(|s| !s.contains('\n'))
            .ok_or_else(|| io::Error::other("invalid scan path"))?;
        let response = self.command(&format!("CONTSCAN {dir_str}"), self.opts.scan_timeout)?;
        log::debug!("CONTSCAN {dir_str}: {response:?}");
        parse_scan_response(dir_str, &response)
    }

    /// Whether [`Clamav::shutdown`] was called.
    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Ask clamd to stop and wait for it (killing it after a few seconds).
    /// Idempotent.
    pub fn shutdown(&self) {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        self.set_ready(false);
        log::info!("stopping clamd");
        if let Err(err) = self.raw_command("SHUTDOWN", Some(CMD_TIMEOUT)) {
            log::debug!("SHUTDOWN command failed: {err}");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.child().as_mut().map(Child::try_wait) {
                None | Some(Ok(Some(_))) | Some(Err(_)) => break,
                Some(Ok(None)) if Instant::now() > deadline => {
                    log::warn!("clamd didn't stop, killing it");
                    self.kill();
                    break;
                }
                Some(Ok(None)) => thread::sleep(Duration::from_millis(100)),
            }
        }
        *self.child() = None;
        let _ = fs::remove_file(&self.socket_path);
    }

    fn kill(&self) {
        if let Some(child) = self.child().as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn write_config(opts: &ClamdOptions, config_path: &Path, socket_path: &Path) -> io::Result<()> {
    let dir = &opts.dir;
    let mut config = format!(
        "# Generated by usbsas-analyzer-server, do not edit\n\
         LocalSocket {}\n\
         TemporaryDirectory {}\n\
         Foreground true\n",
        socket_path.display(),
        dir.join("tmp").display(),
    );

    // Use a dummy database for tests (faster loading, only detects eicar)
    #[cfg(feature = "integration-tests")]
    {
        let db_dir = dir.join("db");
        fs::create_dir_all(&db_dir)?;
        fs::write(
            db_dir.join("test.ndb"),
            "Eicar-Test-Signature:0:0:58354f2150254041505b345c505a58353428505e2937434329377d2445494341522d5354414e444152442d414e544956495255532d544553542d46494c452124482b482a\nEicar-Test-Signature-1:0:*:574456504956416c51454651577a5263554670594e54516f554634704e304e444b5464394a45564a513046534c564e555155354551564a454c55464f56456c5753564a565579315552564e550a4c555a4a544555684a45677253436f3d0a",
        )?;
        config.push_str(&format!("DatabaseDirectory {}\n", db_dir.display()));
    }

    if let Some(extra) = &opts.extra_config {
        let extra_content = fs::read_to_string(extra)
            .map_err(|err| io::Error::new(err.kind(), format!("{}: {err}", extra.display())))?;
        config.push_str("# From ");
        config.push_str(&extra.display().to_string());
        config.push('\n');
        config.push_str(&extra_content);
        config.push('\n');
    }
    fs::write(config_path, config)
}

/// Parse the answer to a `CONTSCAN` command on `base`.
///
/// Error messages may contain `": "`, so the path is found by stripping the
/// (known) base directory; the relative file name is then everything up to
/// the first `": "`. The caller must make sure file names don't contain
/// `": "` nor newlines.
fn parse_scan_response(base: &str, response: &str) -> io::Result<ScanOutcome> {
    let unexpected = |line: &str| io::Error::other(format!("unexpected clamd answer: {line:?}"));
    let mut outcome = ScanOutcome::default();
    let mut lines = 0;
    for line in response.lines().filter(|l| !l.trim().is_empty()) {
        lines += 1;
        let rest = line.strip_prefix(base).ok_or_else(|| unexpected(line))?;
        let (name, result) = if let Some(result) = rest.strip_prefix(": ") {
            (None, result)
        } else if let Some(rest) = rest.strip_prefix('/') {
            let (name, result) = rest.split_once(": ").ok_or_else(|| unexpected(line))?;
            (Some(name.to_string()), result)
        } else {
            return Err(unexpected(line));
        };
        if result == "OK" {
            continue;
        } else if let Some(signature) = result.strip_suffix(" FOUND") {
            let name = name.ok_or_else(|| unexpected(line))?;
            outcome.infected.insert(name, signature.to_string());
        } else if let Some(err) = result.strip_suffix(" ERROR") {
            outcome.errors.push((name, err.to_string()));
        } else {
            return Err(unexpected(line));
        }
    }
    if lines == 0 {
        return Err(io::Error::other("empty answer from clamd"));
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok() {
        let o = parse_scan_response("/a/b", "/a/b: OK\n").unwrap();
        assert!(o.infected.is_empty() && o.errors.is_empty());
    }

    #[test]
    fn parse_found_and_errors() {
        let o = parse_scan_response(
            "/a: b",
            "/a: b/1: Eicar-Test-Signature FOUND\n/a: b/2: Win.Test FOUND\n\
             /a: b/3: lstat() failed: No such file. ERROR\n/a: b: Can't open: x ERROR\n",
        )
        .unwrap();
        assert_eq!(o.infected["1"], "Eicar-Test-Signature");
        assert_eq!(o.infected["2"], "Win.Test");
        assert_eq!(
            o.errors[0],
            (Some("3".into()), "lstat() failed: No such file.".into())
        );
        assert_eq!(o.errors[1], (None, "Can't open: x".into()));
    }

    #[test]
    fn parse_garbage() {
        assert!(parse_scan_response("/a", "").is_err());
        assert!(parse_scan_response("/a", "UNKNOWN COMMAND\n").is_err());
        assert!(parse_scan_response("/a", "/a: something weird\n").is_err());
        assert!(parse_scan_response("/a", "/other/1: X FOUND\n").is_err());
        assert!(parse_scan_response("/a", "/a: X FOUND\n").is_err());
    }
}
