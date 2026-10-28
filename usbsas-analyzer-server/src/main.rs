//! Remote analyze / upload / download server for `usbsas`, using `clamav`.
//!
//! This is the reference implementation of the network side of usbsas. It
//! is used by the integration tests and as an example.

mod api;
mod clamav;
mod error;
mod scan;
mod storage;

use actix_web::{App, HttpServer, web};
use clap::{Arg, ArgAction, Command, value_parser};
use std::{io, path::PathBuf, sync::Arc, time::Duration};

/// Command line options.
#[derive(Debug)]
struct Args {
    working_dir: Option<PathBuf>,
    bind: String,
    workers: Option<usize>,
    max_concurrent_scans: Option<usize>,
    max_bundle_size: Option<u64>,
    result_ttl: u64,
    clamd: PathBuf,
    clamd_config: Option<PathBuf>,
    clamd_start_timeout: u64,
    scan_timeout: u64,
    disable_shutdown: bool,
}

impl Args {
    fn command() -> Command {
        Command::new("usbsas-analyzer-server")
            .about(
                "usbsas remote server: analyze bundles with clamav, store uploaded bundles \
                 and serve bundles to download",
            )
            .version(env!("CARGO_PKG_VERSION"))
            .arg(
                Arg::new("working-dir")
                    .short('d')
                    .long("working-dir")
                    .value_name("WORKING-DIR")
                    .value_parser(value_parser!(PathBuf))
                    .help(
                        "Working directory where bundles are stored (WORKING-DIR/USER/BUNDLE.tar). \
                         Defaults to a temporary directory removed at exit",
                    ),
            )
            .arg(
                Arg::new("bind")
                    .short('b')
                    .long("bind")
                    .value_name("ADDR:PORT")
                    .default_value("127.0.0.1:8042")
                    .help("Address to listen on"),
            )
            .arg(
                Arg::new("workers")
                    .long("workers")
                    .value_name("N")
                    .value_parser(value_parser!(usize))
                    .help("Number of HTTP worker threads [default: number of CPUs]"),
            )
            .arg(
                Arg::new("max-concurrent-scans")
                    .long("max-concurrent-scans")
                    .value_name("N")
                    .value_parser(value_parser!(usize))
                    .help(
                        "Maximum number of analyses running at the same time, the others are \
                         queued [default: number of CPUs]",
                    ),
            )
            .arg(
                Arg::new("max-bundle-size")
                    .long("max-bundle-size")
                    .value_name("BYTES")
                    .value_parser(value_parser!(u64))
                    .help("Maximum size of an uploaded bundle, in bytes [default: unlimited]"),
            )
            .arg(
                Arg::new("result-ttl")
                    .long("result-ttl")
                    .value_name("SECONDS")
                    .value_parser(value_parser!(u64))
                    .default_value("3600")
                    .help(
                        "Seconds after which an analysis report that was never fetched is dropped",
                    ),
            )
            .arg(
                Arg::new("clamd")
                    .long("clamd")
                    .value_name("PATH")
                    .value_parser(value_parser!(PathBuf))
                    .default_value("clamd")
                    .help("clamd binary"),
            )
            .arg(
                Arg::new("clamd-config")
                    .long("clamd-config")
                    .value_name("FILE")
                    .value_parser(value_parser!(PathBuf))
                    .help(
                        "Additional clamd configuration appended to the generated one \
                         (e.g. \"MaxThreads\", \"MaxFileSize\", \"AlertExceedsMax yes\")",
                    ),
            )
            .arg(
                Arg::new("clamd-start-timeout")
                    .long("clamd-start-timeout")
                    .value_name("SECONDS")
                    .value_parser(value_parser!(u64))
                    .default_value("300")
                    .help("Seconds to wait for clamd to load its database"),
            )
            .arg(
                Arg::new("scan-timeout")
                    .long("scan-timeout")
                    .value_name("SECONDS")
                    .value_parser(value_parser!(u64))
                    .default_value("0")
                    .help("Maximum duration of the scan of a bundle, in seconds (0: no limit)"),
            )
            .arg(
                Arg::new("disable-shutdown")
                    .long("disable-shutdown")
                    .action(ArgAction::SetTrue)
                    .help("Disable the GET /shutdown endpoint"),
            )
    }

    fn parse() -> Self {
        let m = Self::command().get_matches();
        // Options with a default value are always present.
        let required = |name: &str| *m.get_one::<u64>(name).expect("default value");
        Args {
            working_dir: m.get_one::<PathBuf>("working-dir").cloned(),
            bind: m.get_one::<String>("bind").expect("default value").clone(),
            workers: m.get_one::<usize>("workers").copied(),
            max_concurrent_scans: m.get_one::<usize>("max-concurrent-scans").copied(),
            max_bundle_size: m.get_one::<u64>("max-bundle-size").copied(),
            result_ttl: required("result-ttl"),
            clamd: m
                .get_one::<PathBuf>("clamd")
                .expect("default value")
                .clone(),
            clamd_config: m.get_one::<PathBuf>("clamd-config").cloned(),
            clamd_start_timeout: required("clamd-start-timeout"),
            scan_timeout: required("scan-timeout"),
            disable_shutdown: m.get_flag("disable-shutdown"),
        }
    }
}

#[actix_web::main]
async fn main() -> io::Result<()> {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "info"));
    let args = Args::parse();

    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());

    let (working_dir, _temp_dir) = match &args.working_dir {
        Some(dir) => (dir.clone(), None),
        None => {
            let tmp = tempfile::Builder::new()
                .prefix("usbsas-analyzer")
                .tempdir()?;
            log::warn!(
                "no working directory given, using {} (removed at exit)",
                tmp.path().display()
            );
            (tmp.path().to_path_buf(), Some(tmp))
        }
    };

    let storage = Arc::new(storage::Storage::new(&working_dir, args.max_bundle_size)?);
    log::info!("working directory: {}", storage.root().display());

    let clamav = {
        let opts = clamav::ClamdOptions {
            binary: args.clamd.clone(),
            dir: storage.root().join(storage::CLAMD_DIR),
            extra_config: args.clamd_config.clone(),
            start_timeout: Duration::from_secs(args.clamd_start_timeout),
            scan_timeout: (args.scan_timeout > 0).then(|| Duration::from_secs(args.scan_timeout)),
        };
        tokio::task::spawn_blocking(move || clamav::Clamav::start(opts))
            .await
            .map_err(io::Error::other)??
    };

    let scanner = Arc::new(scan::Scanner::new(
        Arc::clone(&storage),
        Arc::clone(&clamav),
        args.max_concurrent_scans.unwrap_or(cpus),
        Duration::from_secs(args.result_ttl),
    ));

    let state = web::Data::new(api::AppState {
        storage,
        scanner: Arc::clone(&scanner),
        clamav: Arc::clone(&clamav),
        shutdown_enabled: !args.disable_shutdown,
        server: Default::default(),
    });

    let app_state = state.clone();
    let server = HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .configure(api::configure)
    })
    .workers(args.workers.unwrap_or(cpus))
    .bind(&args.bind)
    .inspect_err(|err| {
        log::error!("couldn't listen on {}: {err}", args.bind);
        clamav.shutdown();
    })?
    .run();
    let _ = state.server.set(server.handle());
    log::info!("listening on {}", args.bind);

    // Periodically drop the reports nobody fetched.
    actix_web::rt::spawn(async move {
        let mut interval = actix_web::rt::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            scanner.reap();
        }
    });

    let res = server.await;
    tokio::task::spawn_blocking(move || clamav.shutdown())
        .await
        .map_err(io::Error::other)?;
    log::info!("stopped");
    res
}
