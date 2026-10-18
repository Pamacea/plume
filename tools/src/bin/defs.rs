//! Génère `data/defs.txt` — définitions courtes pour l'affichage dans la bulle.
//!
//! Source : dump JSONL.gz de kaikki.org (wiktextract), édition **fr.wiktionary**
//! (`lang_code = fr`, glosses en français — pas l'édition anglaise), première
//! définition de chaque entrée. Licences et URL : voir tools/README.md.
//! Ré-exécutable hors ligne (cache).

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

const URL_DEFS: &str =
    "https://kaikki.org/frwiktionary/Fran%C3%A7ais/kaikki.org-dictionary-Fran%C3%A7ais.jsonl.gz";
/// Longueur max d'une ligne `mot|définition` (en caractères, tronquée sur un mot).
const MAX_LIGNE: usize = 140;
const MIN_ENTREES: usize = 80_000;
const MAX_OCTETS: usize = 30 * 1024 * 1024;

/// Télécharge `url` dans `dest` (curl). Déjà en cache : aucun appel réseau.
fn fetch(url: &str, dest: &Path) {
    if dest.exists() {
        return;
    }
    let tmp = dest.with_extension("part");
    let statut = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&tmp)
        .arg(url)
        .status();
    match statut {
        Ok(s) if s.success() => {
            if let Err(e) = fs::rename(&tmp, dest) {
                panic!("impossible d'enregistrer {}: {e}", dest.display());
            }
        }
        _ => {
            let _ = fs::remove_file(&tmp);
            panic!("téléchargement impossible : {url}");
        }
    }
}

/// Une ligne JSON du dump → `(mot minuscule, définition)` si définition il y a.
/// Écarte les pages de formes (`pos_title` « Forme … » : conjugaisons et fléchis,
/// 1,4 M d'entrées sans contenu sémantique) et les noms propres (Prénom,
/// Nom de famille… : étiquettes, pas des définitions) — sinon > 30 Mo.
fn entree(ligne: &str) -> Option<(String, String)> {
    let v: Value = serde_json::from_str(ligne).ok()?;
    if v.get("lang_code").and_then(Value::as_str) != Some("fr") {
        return None;
    }
    let mot = v.get("word")?.as_str()?.to_lowercase();
    if mot.is_empty() || mot.contains('|') || mot.contains('#') {
        return None;
    }
    let titre = v.get("pos_title").and_then(Value::as_str).unwrap_or("");
    if titre.starts_with("Forme")
        || titre.starts_with("Nom propre")
        || titre.starts_with("Nom de famille")
        || titre.starts_with("Prénom")
        || titre.starts_with("Patronyme")
    {
        return None;
    }
    let mut def: Option<&str> = None;
    for sense in v.get("senses")?.as_array()? {
        if let Some(g) = sense.get("glosses").and_then(Value::as_array) {
            if let Some(t) = g.first().and_then(Value::as_str) {
                def = Some(t);
                break;
            }
        }
    }
    // La définition tient dans le reste de la ligne après `mot|`.
    let budget = MAX_LIGNE.saturating_sub(mot.chars().count() + 1).max(1);
    let def = tronque(&nettoie(def?), budget)?;
    Some((mot, def))
}

/// Sauts de ligne et `|` (séparateur du format) en espaces, espaces répétés fusionnés.
fn nettoie(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut espace = false;
    for c in s.chars() {
        let c = if c == '|' || c.is_whitespace() { ' ' } else { c };
        if c == ' ' {
            espace = true; // on n'écrit l'espace qu'au prochain caractère utile
        } else {
            if espace && !out.is_empty() {
                out.push(' ');
            }
            out.push(c);
            espace = false;
        }
    }
    out.trim().to_string()
}

/// Définition ≤ `max` caractères, tronquée sur un espace + `…`. Vide si pas de texte.
fn tronque(s: &str, max: usize) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    if s.chars().count() <= max {
        return Some(s.to_string());
    }
    let coup: String = s.chars().take(max - 1).collect();
    let fin = match coup.rfind(' ') {
        Some(i) if i > 4 => &coup[..i],
        _ => coup.trim_end(),
    };
    Some(format!("{fin}…"))
}

fn main() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cache = base.join("cache");
    let data = base.join("..").join("data");
    fs::create_dir_all(&cache).expect("création de tools/cache");

    println!("Téléchargement de la source…");
    let p_gz = cache.join("kaikki-frwiktionary.jsonl.gz");
    fetch(URL_DEFS, &p_gz);

    let fichier = fs::File::open(&p_gz).expect("ouverture du cache kaikki-frwiktionary.jsonl.gz");
    let lecture = BufReader::new(flate2::read::GzDecoder::new(fichier));
    let mut defs: BTreeMap<String, String> = BTreeMap::new();
    let (mut lignes, mut fr, mut sans_def) = (0usize, 0usize, 0usize);
    for ligne in lecture.lines() {
        let ligne = ligne.expect("lecture JSONL (décompression)");
        lignes += 1;
        if ligne.contains("\"lang_code\":\"fr\"") || ligne.contains("\"lang_code\": \"fr\"") {
            fr += 1;
        }
        match entree(&ligne) {
            Some((mot, def)) => {
                defs.entry(mot).or_insert(def);
            }
            None => sans_def += 1,
        }
        if lignes % 100_000 == 0 {
            println!("  {lignes} lignes traitées…");
        }
    }

    // Écriture + contrôles : exactement `mot|définition`, aucun champ vide.
    let mut texte = String::with_capacity(defs.len() * 70);
    for (mot, def) in &defs {
        if mot.is_empty() || def.is_empty() || def.contains('|') {
            eprintln!("ÉCHEC : entrée invalide {mot} | {def}");
            std::process::exit(1);
        }
        texte.push_str(mot);
        texte.push('|');
        texte.push_str(def);
        texte.push('\n');
    }
    let path_out = data.join("defs.txt");
    if let Err(e) = fs::write(&path_out, &texte) {
        panic!("écriture impossible {}: {e}", path_out.display());
    }
    println!(
        "data/defs.txt : {} entrées, {:.1} Mo ({lignes} lignes source, {fr} fr, {sans_def} écartées ou sans définition)",
        defs.len(),
        texte.len() as f64 / 1_048_576.0
    );

    // Spot-checks : entrées obligatoires + contrôle du format ligne à ligne.
    let mut erreurs: Vec<String> = Vec::new();
    for mot in ["bonjour", "être", "grand"] {
        match defs.get(mot) {
            Some(d) => println!("  {mot} → {d}"),
            None => erreurs.push(format!("spot-check absent : {mot}")),
        }
    }
    match defs.get("c'est") {
        Some(d) => println!("  c'est → {d} (présent)"),
        None => println!("  c'est → absent du dump (non bloquant)"),
    }
    let mut lues = 0usize;
    for ligne in texte.lines() {
        lues += 1;
        let c = ligne.matches('|').count();
        if c != 1 {
            erreurs.push(format!("{c} « | » sur : {ligne}"));
        }
    }
    if defs.len() < MIN_ENTREES {
        erreurs.push(format!("{} entrées < {MIN_ENTREES}", defs.len()));
    }
    if texte.len() > MAX_OCTETS {
        erreurs.push(format!("{} octets > 30 Mo", texte.len()));
    }

    if erreurs.is_empty() {
        println!(
            "Spot-checks : OK ({lues} lignes, 1 « | » par ligne, 0 champ vide, ≥ {MIN_ENTREES} entrées, ≤ 30 Mo)"
        );
    } else {
        for e in &erreurs {
            eprintln!("ÉCHEC : {e}");
        }
        std::process::exit(1);
    }
}
