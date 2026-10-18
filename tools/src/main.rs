//! Phase 4 de Plume — génère deux artefacts 100 % locaux :
//!
//!   data/conjugaison.json — table de conjugaison : {forme: [{inf, temps, personnes}]}
//!   data/mots.txt         — liste de mots (minuscules, alphabétiques, triés)
//!
//! Le script télécharge les sources (une fois en cache, réutilisable hors ligne),
//! les convertit, régénère les deux fichiers et vérifie les spot-checks.
//! Licences et URLs : voir tools/README.md.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const URL_VERBES: &str = "https://raw.githubusercontent.com/bretttolbert/verbecc/main/verbecc/data/xml/verbs/verbs-fr.xml";
const URL_CONJ: &str = "https://raw.githubusercontent.com/bretttolbert/verbecc/main/verbecc/data/xml/conjugations/conjugations-fr.xml";
const URL_DICO: &str =
    "https://raw.githubusercontent.com/hbenbel/French-Dictionary/master/dictionary/dictionary.csv";

const MOODS: [&str; 6] = [
    "Infinitif",
    "Indicatif",
    "Conditionnel",
    "Subjonctif",
    "Imperatif",
    "Participe",
];
const TENSE_ELEMS: [&str; 8] = [
    "infinitif-présent",
    "présent",
    "imparfait",
    "futur-simple",
    "passé-simple",
    "imperatif-présent",
    "participe-présent",
    "participe-passé",
];
const P6: &[&str] = &["1s", "2s", "3s", "1p", "2p", "3p"];
const PIMP: &[&str] = &["2s", "1p", "2p"];
const P1: &[&str] = &["-"];
const PPP: &[&str] = &["-", "-", "-", "-"];

/// (temps, personne, suffixe) : le radical est ajouté par verbe.
type Slots = Vec<(&'static str, &'static str, String)>;
type Formes = BTreeMap<String, BTreeMap<(String, String), BTreeSet<String>>>;

/// Télécharge `url` dans `dest` (curl). À défaut de réseau, réutilise le cache.
fn fetch(url: &str, dest: &Path) {
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
            if dest.exists() {
                eprintln!("avertissement : téléchargement impossible ({url}), cache réutilisé");
            } else {
                panic!("téléchargement impossible et aucun cache : {url}");
            }
        }
    }
}

/// `<v><i>infinitif</i><t>template</t>…</v>` (une ligne par verbe).
fn parse_verbes(xml: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for ligne in xml.lines() {
        if !ligne.starts_with("<v>") {
            continue;
        }
        let champ = |balise: &str| {
            let a = ligne.find(balise)?;
            let d = a + balise.len();
            let b = ligne[d..].find("</")?;
            Some(ligne[d..d + b].to_string())
        };
        if let (Some(i), Some(t)) = (champ("<i>"), champ("<t>")) {
            out.insert((i, t));
        }
    }
    out
}

fn decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn tense_def(mood: &str, name: &str) -> Option<(&'static str, &'static [&'static str])> {
    Some(match (mood, name) {
        ("Infinitif", "infinitif-présent") => ("infinitif", P1),
        ("Indicatif", "présent") => ("présent", P6),
        ("Indicatif", "imparfait") => ("imparfait", P6),
        ("Indicatif", "futur-simple") => ("futur", P6),
        ("Indicatif", "passé-simple") => ("passé simple", P6),
        ("Conditionnel", "présent") => ("conditionnel présent", P6),
        ("Subjonctif", "présent") => ("subjonctif présent", P6),
        ("Subjonctif", "imparfait") => ("subjonctif imparfait", P6),
        ("Imperatif", "imperatif-présent") => ("impératif", PIMP),
        ("Participe", "participe-présent") => ("participe présent", P1),
        ("Participe", "participe-passé") => ("participe passé", PPP),
        _ => return None,
    })
}

/// Analyse les templates de conjugaison (XML très régulier, sans dépendance).
/// Un `<p>` vide = forme inexistante (verbe impersonnel) ; plusieurs `<i>`
/// dans un `<p>` = graphies alternatives (« ece/èce », « iérai/eyerai »).
fn parse_templates(xml: &str) -> (BTreeMap<String, Slots>, usize) {
    let mut out: BTreeMap<String, Slots> = BTreeMap::new();
    let mut avertissements = 0usize;
    let mut courant: Option<String> = None;
    let mut mood = String::new();
    let mut actif: Option<(&'static str, &'static [&'static str])> = None;
    let mut slot = 0usize;
    let mut alts: Option<Vec<String>> = None;
    let mut dans_i = false;
    let mut i = 0usize;

    while i < xml.len() {
        let Some(lt) = xml[i..].find('<') else { break };
        let abs = i + lt;
        if xml[abs..].starts_with("<!--") {
            i = match xml[abs..].find("-->") {
                Some(p) => abs + p + 3,
                None => xml.len(),
            };
            continue;
        }
        if dans_i {
            if let Some(a) = alts.as_mut().and_then(|v| v.last_mut()) {
                a.push_str(&xml[i..abs]);
            }
        }
        let Some(gt_rel) = xml[abs..].find('>') else {
            break;
        };
        let gt = abs + gt_rel;
        let tag = &xml[abs + 1..gt];
        i = gt + 1;

        if tag.starts_with('!') || tag.starts_with('?') {
            continue;
        }
        if let Some(nom) = tag.strip_prefix('/') {
            match nom.trim() {
                "i" => dans_i = false,
                "p" => {
                    if let (Some((temps, personnes)), Some(nom_tpl)) = (actif, courant.as_ref()) {
                        let slots = out.entry(nom_tpl.clone()).or_default();
                        if slot < personnes.len() {
                            if let Some(alts_pris) = alts.take() {
                                for a in alts_pris {
                                    if !a.is_empty() {
                                        slots.push((temps, personnes[slot], decode(&a)));
                                    }
                                }
                            }
                        } else {
                            avertissements += 1;
                        }
                        slot += 1;
                    }
                    alts = None;
                }
                "template" => {
                    courant = None;
                    actif = None;
                }
                nom => {
                    if TENSE_ELEMS.contains(&nom) {
                        actif = None;
                    } else if MOODS.contains(&nom) {
                        mood.clear();
                    }
                }
            }
            continue;
        }

        let corps = tag.trim_end_matches('/');
        let nom = corps.split_whitespace().next().unwrap_or("");
        match nom {
            "template" => {
                let nom_tpl = corps
                    .find("name=\"")
                    .and_then(|p| corps[p + 6..].find('"').map(|f| p + 6..p + 6 + f))
                    .map(|r| corps[r].to_string());
                courant = nom_tpl;
                if courant.is_some() {
                    out.entry(courant.clone().unwrap()).or_default();
                }
                actif = None;
            }
            "p" => {
                alts = Some(Vec::new());
                dans_i = false;
            }
            "i" => {
                if let Some(a) = alts.as_mut() {
                    a.push(String::new());
                    dans_i = true;
                }
            }
            nom => {
                if MOODS.contains(&nom) {
                    mood.clear();
                    mood.push_str(nom);
                    actif = None;
                } else if let Some(d) = tense_def(&mood, nom) {
                    actif = Some(d);
                    slot = 0;
                }
            }
        }
    }
    (out, avertissements)
}

/// Radicals : le suffixe après «:» du nom du template est retiré du verbe.
/// Ex. manger + template «man:ger» → radical «man» + «ge» = «mange».
fn deriver(
    formes: &mut Formes,
    verbes: &[(String, String)],
    tpl: &BTreeMap<String, Slots>,
) -> (usize, usize, usize) {
    let (mut nb_verbes, mut sauts, mut decalages) = (0usize, 0usize, 0usize);
    for (verbe, nom_tpl) in verbes {
        let Some(slots) = tpl.get(nom_tpl) else {
            sauts += 1;
            eprintln!("avertissement : template introuvable {nom_tpl} ({verbe})");
            continue;
        };
        let droite = match nom_tpl.split_once(':') {
            Some((_, d)) => d,
            None => {
                sauts += 1;
                continue;
            }
        };
        if !verbe.ends_with(droite) {
            decalages += 1;
            eprintln!("avertissement : radical inattendu {verbe} ({nom_tpl})");
            continue;
        }
        let radical = &verbe[..verbe.len() - droite.len()];
        for (temps, personne, alt) in slots {
            let forme = format!("{radical}{alt}").to_lowercase();
            if forme.is_empty() {
                continue;
            }
            formes
                .entry(forme)
                .or_default()
                .entry((verbe.clone(), (*temps).to_string()))
                .or_default()
                .insert((*personne).to_string());
        }
        nb_verbes += 1;
    }
    (nb_verbes, sauts, decalages)
}

fn json_str(s: &mut String, v: &str) {
    s.push('"');
    for c in v.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
}

fn rang(p: &str) -> u8 {
    match p {
        "1s" => 0,
        "2s" => 1,
        "3s" => 2,
        "1p" => 3,
        "2p" => 4,
        "3p" => 5,
        _ => 6,
    }
}

fn ecrire_json(path: &Path, formes: &Formes) -> (usize, usize) {
    let mut s = String::with_capacity(32 * 1024 * 1024);
    let (mut lectures, mut premier_f) = (0usize, true);
    s.push('{');
    for (forme, lectures_forme) in formes {
        if !premier_f {
            s.push(',');
        }
        premier_f = false;
        json_str(&mut s, forme);
        s.push(':');
        s.push('[');
        let mut premier_r = true;
        for ((inf, temps), personnes) in lectures_forme {
            if !premier_r {
                s.push(',');
            }
            premier_r = false;
            s.push_str("{\"inf\":");
            json_str(&mut s, inf);
            s.push_str(",\"temps\":");
            json_str(&mut s, temps);
            s.push_str(",\"personnes\":[");
            // «-» est toujours seul ; sinon ordre canonique 1s..3p.
            let mut ps: Vec<&String> = personnes.iter().collect();
            if !personnes.contains("-") {
                ps.sort_by_key(|p| rang(p.as_str()));
            }
            for (k, p) in ps.iter().enumerate() {
                if k > 0 {
                    s.push(',');
                }
                json_str(&mut s, p.as_str());
            }
            s.push_str("]}");
            lectures += 1;
        }
        s.push(']');
    }
    s.push('}');
    if let Err(e) = fs::write(path, &s) {
        panic!("écriture impossible {}: {e}", path.display());
    }
    (lectures, s.len())
}

/// Mot accepté : non vide, que des lettres minuscules (pas de ponctuation,
/// de chiffres, de majuscules — spec de data/mots.txt).
fn est_mot(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphabetic() && c.is_lowercase())
}

fn spot_checks(formes: &Formes, mots: &BTreeSet<String>) -> Vec<String> {
    let requis: &[(&str, &str, &str, &str)] = &[
        ("mange", "manger", "présent", "1s"),
        ("mange", "manger", "présent", "3s"),
        ("suis", "être", "présent", "1s"),
        ("irai", "aller", "futur", "1s"),
        ("allé", "aller", "participe passé", "-"),
        ("fûmes", "être", "passé simple", "1p"),
        ("ont", "avoir", "présent", "3p"),
    ];
    let mut erreurs = Vec::new();
    for (forme, inf, temps, personne) in requis {
        let ok = formes.get(*forme).is_some_and(|lectures| {
            lectures.iter().any(|((i, t), p)| {
                i.as_str() == *inf && t.as_str() == *temps && p.contains(*personne)
            })
        });
        if !ok {
            erreurs.push(format!(
                "conjugaison.json : lecture absente {forme} → {inf}/{temps}/{personne}"
            ));
        }
    }
    for mot in ["bonjour", "grandes", "écrire"] {
        if !mots.contains(mot) {
            erreurs.push(format!("mots.txt : mot absent {mot}"));
        }
    }
    erreurs
}

fn main() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cache = base.join("cache");
    let data = base.join("..").join("data");
    fs::create_dir_all(&cache).expect("création de tools/cache");
    fs::create_dir_all(&data).expect("création de data");

    println!("Téléchargement des sources…");
    let p_verbes = cache.join("verbs-fr.xml");
    let p_conj = cache.join("conjugations-fr.xml");
    let p_dico = cache.join("dictionary.csv");
    fetch(URL_VERBES, &p_verbes);
    fetch(URL_CONJ, &p_conj);
    fetch(URL_DICO, &p_dico);

    let xml_verbes = fs::read_to_string(&p_verbes).expect("lecture verbs-fr.xml");
    let xml_conj = fs::read_to_string(&p_conj).expect("lecture conjugations-fr.xml");
    let csv = fs::read_to_string(&p_dico).expect("lecture dictionary.csv");

    let verbes: Vec<(String, String)> = parse_verbes(&xml_verbes).into_iter().collect();
    let (templates, avertissements_tpl) = parse_templates(&xml_conj);
    let vides = templates.values().filter(|s| s.is_empty()).count();
    println!(
        "Sources : {} verbes, {} templates ({} sans forme, {} avertissements d'analyse)",
        verbes.len(),
        templates.len(),
        vides,
        avertissements_tpl
    );

    let mut formes: Formes = BTreeMap::new();
    let (nb_verbes, sauts, decalages) = deriver(&mut formes, &verbes, &templates);
    println!(
        "Expansion : {nb_verbes} verbes conjugués, {sauts} sautés, {decalages} radicaux inattendus"
    );

    let path_json = data.join("conjugaison.json");
    let (lectures, octets) = ecrire_json(&path_json, &formes);
    println!(
        "data/conjugaison.json : {} formes, {lectures} lectures, {:.1} Mo",
        formes.len(),
        octets as f64 / 1_048_576.0
    );

    let mut mots: BTreeSet<String> = BTreeSet::new();
    for ligne in csv.lines() {
        let l = ligne.trim();
        if est_mot(l) {
            mots.insert(l.to_string());
        }
    }
    // Cohérence entre les deux artefacts : toute forme conjuguée est un mot.
    for forme in formes.keys() {
        if est_mot(forme) {
            mots.insert(forme.clone());
        }
    }
    let path_mots = data.join("mots.txt");
    let mut texte = String::with_capacity(mots.len() * 12);
    for m in &mots {
        texte.push_str(m);
        texte.push('\n');
    }
    if let Err(e) = fs::write(&path_mots, &texte) {
        panic!("écriture impossible {}: {e}", path_mots.display());
    }
    println!(
        "data/mots.txt : {} mots, {:.1} Mo",
        mots.len(),
        texte.len() as f64 / 1_048_576.0
    );

    let erreurs = spot_checks(&formes, &mots);
    if erreurs.is_empty() {
        println!("Spot-checks : OK (7 lectures conjuguées + bonjour/grandes/écrire)");
    } else {
        for e in &erreurs {
            eprintln!("ÉCHEC : {e}");
        }
        std::process::exit(1);
    }
}
