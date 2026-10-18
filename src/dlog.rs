//! Log de diagnostic dans `plume.log` (cwd) — voir où la chaîne hook→bulle casse.
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn dlog(msg: &str) {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("plume.log")
    {
        let _ = writeln!(f, "{ms} {msg}");
    }
}
