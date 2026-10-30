//! Log de diagnostic dans `plume.log` (cwd) — voir où la chaîne hook→bulle casse.
//! Écriture asynchrone : `dlog` tourne DANS le hook clavier bas niveau, où toute
//! I/O bloquerait la frappe de tout le système (freezes en frappe rapide).
use std::io::Write;
use std::sync::OnceLock;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::{SystemTime, UNIX_EPOCH};

static TX: OnceLock<SyncSender<String>> = OnceLock::new();

/// File bornée + thread d'écriture : le hook dépose une ligne et repart.
fn tx() -> &'static SyncSender<String> {
    TX.get_or_init(|| {
        let (tx, rx) = sync_channel::<String>(1024);
        std::thread::spawn(move || {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("plume.log")
                .ok();
            let mut batch = String::new();
            while let Ok(msg) = rx.recv() {
                batch.push_str(&msg);
                // vide la file d'un coup : une écriture par rafale, pas par ligne
                while let Ok(m) = rx.try_recv() {
                    batch.push_str(&m);
                }
                if let Some(f) = file.as_mut() {
                    let _ = f.write_all(batch.as_bytes());
                    let _ = f.flush();
                }
                batch.clear();
            }
        });
        tx
    })
}

pub fn dlog(msg: &str) {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // try_send : file pleine = ligne perdue, jamais la frappe
    let _ = tx().try_send(format!("{ms} {msg}\n"));
}
