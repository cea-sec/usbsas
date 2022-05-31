#[cfg(feature = "log-json")]
use {
    env_logger::{Builder, Env},
    std::{
        io::Write,
        sync::{Arc, RwLock},
    },
};

#[cfg(feature = "log-json")]
pub fn init_logger(session_id: Arc<RwLock<String>>) {
    let mut builder = Builder::from_env(Env::default().filter_or("RUST_LOG", "info"));
    builder.format(move |buf, record| {
        write!(buf, "{{")?;
        write!(
            buf,
            "\"ts\":\"{}\",",
            time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        )?;
        write!(buf, " \"level\":\"{}\",", record.level())?;
        write!(buf, " \"target\":\"{}\",", record.target())?;
        write!(buf, " \"msg\":{},", serde_json::to_string(&record.args())?)?;
        #[cfg(feature = "log-json")]
        {
            write!(buf, " \"transfer_id\":\"{}\"", session_id.read().unwrap())?;
        }
        writeln!(buf, "}}")?;

        Ok(())
    });
    builder.init();
}

#[cfg(not(feature = "log-json"))]
pub fn init_logger() {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "info"));
}
