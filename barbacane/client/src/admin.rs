//! Client du service d'administration `barbacane-admind` (socket Unix,
//! une requête JSON par ligne, une réponse JSON par ligne). Le PIN est
//! vérifié par le service, jamais par l'interface.

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub fn request(socket: &str, req: Value) -> Result<Value, String> {
    let mut s = UnixStream::connect(socket)
        .map_err(|e| format!("service d'administration injoignable ({e})"))?;
    s.set_read_timeout(Some(Duration::from_secs(900))).ok();
    let mut line = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    s.write_all(&line).map_err(|e| e.to_string())?;
    let mut rep = String::new();
    BufReader::new(s)
        .read_line(&mut rep)
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&rep).map_err(|e| format!("réponse invalide : {e}"))?;
    match v.get("error").and_then(Value::as_str) {
        Some(e) => Err(e.to_string()),
        None => Ok(v),
    }
}

/// Version asynchrone pour l'interface.
pub async fn call(socket: String, req: Value) -> Result<Value, String> {
    smol::unblock(move || request(&socket, req)).await
}
