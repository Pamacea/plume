//! Raccourci de lancement (menu Démarrer + Bureau) réécrit à chaque démarrage.
//!
//! Pas d'installateur : l'exé reste où l'utilisateur l'a dézippé ; écraser le
//! raccourci à chaque lancement le répare si le dossier a bougé.

use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, IPersistFile,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    ShellLink,
};
use windows::core::{Interface, PCWSTR};

pub fn install() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(work) = exe.parent() else {
        return;
    };
    unsafe {
        // RPC_E_CHANGED_MODE si COM est déjà monté autrement : on tente quand même
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        for folder in [FOLDERID_Desktop, FOLDERID_Programs] {
            let Some(dir) = known(folder) else {
                continue;
            };
            let lnk = dir.join("Plume.lnk");
            // échec silencieux (profil inaccessible…) : dlog, jamais de panique
            match write(&lnk, &exe, work) {
                Ok(()) => crate::dlog::dlog(&format!("raccourci {} écrit", lnk.display())),
                Err(e) => crate::dlog::dlog(&format!("raccourci {} : {e}", lnk.display())),
            }
        }
    }
}

/// Dossier connu du profil (Bureau : suit la redirection OneDrive si présente).
unsafe fn known(id: windows::core::GUID) -> Option<PathBuf> {
    let pw = unsafe { SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None) }.ok()?;
    let s = unsafe { pw.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(pw.as_ptr().cast())) };
    s.map(PathBuf::from)
}

unsafe fn write(lnk: &Path, exe: &Path, work: &Path) -> windows::core::Result<()> {
    let sl: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
    // large : %APPDATA%/%USERPROFILE% peut être non ASCII
    let wide =
        |p: &Path| -> Vec<u16> { p.to_string_lossy().encode_utf16().chain(Some(0)).collect() };
    let (t, w, l) = (wide(exe), wide(work), wide(lnk));
    unsafe {
        sl.SetPath(PCWSTR(t.as_ptr()))?;
        sl.SetWorkingDirectory(PCWSTR(w.as_ptr()))?;
        // le raccourci hérite de l'icône de l'exé (index 0)
        let pf: IPersistFile = sl.cast()?;
        pf.Save(PCWSTR(l.as_ptr()), true)
    }
}
