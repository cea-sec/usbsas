#[cfg(feature = "log-json")]
use {
    env_logger::{Builder, Env},
    std::io::Write,
};

#[cfg(feature = "log-json")]
pub fn init_logger() {
    let session_id: String = std::env::var("USBSAS_SESSION_ID").unwrap_or_else(|_| "0".into());

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
        write!(buf, " \"transfer_id\":\"{}\"", session_id)?;
        writeln!(buf, "}}")?;

        Ok(())
    });
    builder.init();
}

#[cfg(not(feature = "log-json"))]
pub fn init_logger() {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "info,http=error"));
}
