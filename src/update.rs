//! Mise à jour automatique depuis la dernière release GitHub.
//!
//! Zéro dépendance Rust : un script PowerShell est posé dans `%TEMP%`,
//! détaché de l'exé (survitent à `exit`), qui télécharge la zip, attend la
//! fin de l'ancien process, recopie (configs locales conservées) et relance.

use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const SCRIPT: &str = r#"param([string]$Exe, [string]$ProcId)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try {
    $dir = Split-Path -Parent $Exe
    $zip = Join-Path $env:TEMP 'plume-update.zip'
    $tmp = Join-Path $env:TEMP 'plume-update'
    Invoke-WebRequest -Uri 'https://github.com/Pamacea/plume/releases/latest/download/plume-win64.zip' -OutFile $zip -UseBasicParsing
    if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force }
    Expand-Archive -Path $zip -DestinationPath $tmp -Force
    # la zip contient un dossier plume-win64/ (repli : racine)
    $src = Join-Path $tmp 'plume-win64'
    if (-not (Test-Path (Join-Path $src 'plume.exe'))) { $src = $tmp }
    if (-not (Test-Path (Join-Path $src 'plume.exe'))) { throw 'archive invalide : plume.exe absent' }
    # l'ancien process est déjà sorti (le script lui survit) ; sinon on attend
    while (Get-Process -Id $ProcId -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 400 }
    # ne jamais écraser la config locale (fichiers utilisateur)
    $keep = 'tics.txt','adjectifs.txt','verbes.txt','exclusions.txt'
    Get-ChildItem $src -Recurse -File | ForEach-Object {
        if ($keep -notcontains $_.Name) {
            $dest = Join-Path $dir $_.FullName.Substring($src.Length + 1)
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
            Copy-Item $_.FullName $dest -Force
        }
    }
    Start-Process -FilePath $Exe -WorkingDirectory $dir
    # zip + extraction : ~110 Mo dans %TEMP% sinon (silencieux = déjà parti)
    Remove-Item $zip, $tmp -Recurse -Force -ErrorAction SilentlyContinue
} catch {
    Add-Type -AssemblyName PresentationFramework
    [System.Windows.MessageBox]::Show("La mise à jour a échoué : $($_.Exception.Message)", 'Plume') | Out-Null
    # l'ancien exé est déjà mort : on le relance pour ne pas laisser l'utilisateur sans app
    while (Get-Process -Id $ProcId -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 400 }
    Start-Process -FilePath $Exe -WorkingDirectory (Split-Path -Parent $Exe)
}
"#;

/// Lance la mise à jour puis quitte l'application (succès ou erreur de spawn).
pub fn launch() {
    let script = std::env::temp_dir().join("plume-update.ps1");
    let result = std::env::current_exe().and_then(|exe| {
        // BOM UTF-8 : Windows PowerShell 5.1 lit les .ps1 sans BOM en ANSI
        std::fs::write(&script, format!("\u{FEFF}{SCRIPT}"))?;
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .arg("-Exe")
            .arg(&exe)
            .arg("-ProcId")
            .arg(std::process::id().to_string())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    });
    match result {
        // le script relancera l'exé après la copie : on peut sortir tout de suite
        Ok(_) => std::process::exit(0),
        Err(e) => {
            crate::dlog::dlog(&format!("mise à jour impossible : {e}"));
            let _ = std::fs::remove_file(&script);
        }
    }
}
