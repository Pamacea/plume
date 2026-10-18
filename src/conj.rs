//! Cohérence grammaticale (phase 6) : trois règles, haute précision —
//! mieux vaut se taire que proposer à côté. Popup en 3e position dans app.rs :
//! tics/adjectifs > orthographe > conjugaison, une seule popup par délimiteur.
//!
//! A — désaccord sujet/nombre : toutes les lectures du mot excluent la personne
//!     du sujet → proposer la forme du même (infinitif, temps) pour cette personne.
//! B — « si » + conditionnel présent → imparfait du même infinitif.
//! C — participe après verbe d'être, verbes à être codés en dur :
//!     elle → -e, elles → -es.
//!
//! Mémoire : formes internées dans un seul blob trié (recherche binaire),
//! lectures compactes {inf u32, temps u8, masque personnes u8} → ≈ 16 Mo
//! au lieu de ~123 Mo (avant : `String` dupliquées partout).

use std::collections::HashMap;
use std::time::Instant;

/// Verbes intransitifs à l'auxiliaire être (leur participe s'accorde avec le sujet).
// shortcut: liste codée en dur (standards, prefixes compris) ; upgrade: déduire de la table
const ETRE_AUX: &[&str] = &[
    "aller",
    "arriver",
    "devenir",
    "redevenir",
    "descendre",
    "entrer",
    "monter",
    "mourir",
    "naître",
    "partir",
    "repartir",
    "rentrer",
    "rester",
    "retourner",
    "ressortir",
    "sortir",
    "survenir",
    "tomber",
    "venir",
    "revenir",
];

/// Particules qui peuvent s'insérer entre le verbe d'être et le participe.
const PARTICULES: &[&str] = &["ne", "n'", "pas", "plus", "jamais"];

/// Lecture compacte d'une forme (8 B alignés) : infinitif et temps internés,
/// personnes en masque binaire — bit 0 = 1s … bit 5 = 3p, bit 6 = `-` non conjugué.
#[derive(Clone, Copy)]
struct Reading {
    inf: u32,
    temps: u8,
    pers: u8,
}

/// Bit d'une personne ; 0 = libellé inconnu des pronoms (jamais demandé par les règles).
fn person_bit(p: &str) -> u8 {
    match p {
        "1s" => 1,
        "2s" => 1 << 1,
        "3s" => 1 << 2,
        "1p" => 1 << 3,
        "2p" => 1 << 4,
        "3p" => 1 << 5,
        "-" => 1 << 6,
        _ => 0,
    }
}

/// Plage d'une forme dans `blob` + plage de ses lectures dans `lectures`.
#[derive(Clone, Copy)]
struct Forme {
    off: u32,
    len: u32,
    lect: u32,
    nlect: u32,
}

fn str_at(blob: &str, off: u32, len: u32) -> &str {
    &blob[off as usize..off as usize + len as usize]
}

pub struct Conj {
    /// toutes les formes du dico dans une seule String, séparées par '\n'
    blob: String,
    /// plages de `blob`, triées par forme → recherche binaire (homonymie : « mange » ×3)
    formes: Vec<Forme>,
    /// lectures compactes, découpées par `Forme::lect`/`nlect`
    lectures: Vec<Reading>,
    /// index inverse (inf, temps, personne) → plage de la forme corrective
    inv: HashMap<(u32, u8, u8), (u32, u32)>,
    /// noms internés référencés par les ids des lectures (7 011 infinitifs, 11 temps)
    infs: Vec<String>,
    temps: Vec<String>,
}

impl Conj {
    /// Charge `data/conjugaison.json`. Synchrone au démarrage : temps loggé dans
    /// plume.log (cible < 1,5 s et < 300 Mo, mesuré — voir rapport de phase).
    // shortcut: lu depuis le répertoire courant, upgrade: chemin figé alongside l'exécutable
    // fichier absent → None = règles désactivées (aucun panic)
    pub fn load() -> Option<Conj> {
        let t = Instant::now();
        let content = std::fs::read_to_string("data/conjugaison.json").ok()?;
        let conj = Conj::from_json(&content)?;
        crate::dlog::dlog(&format!(
            "conjugaison: {} formes, {} index inverses, {} ms",
            conj.formes.len(),
            conj.inv.len(),
            t.elapsed().as_millis()
        ));
        Some(conj)
    }

    /// Parse sans dérive serde (serde_json seul) : un passage `Value`, conversion
    /// immédiate dans les structures compactes (internement en une passe), puis
    /// drop de l'arbre JSON (le pic mémoire = arbre seul, ~24 Mo transitoires).
    fn from_json(content: &str) -> Option<Conj> {
        let serde_json::Value::Object(map) = serde_json::from_str(content).ok()? else {
            return None;
        };
        let mut blob = String::new();
        let mut formes: Vec<Forme> = Vec::with_capacity(map.len());
        let mut lectures: Vec<Reading> = Vec::new();
        let mut inv: HashMap<(u32, u8, u8), (u32, u32)> = HashMap::new();
        // internement local : chaque nom unique n'est copié qu'une seule fois
        let mut infs: Vec<String> = Vec::new();
        let mut inf_id: HashMap<String, u32> = HashMap::new();
        let mut temps: Vec<String> = Vec::new();
        let mut temps_id: HashMap<String, u8> = HashMap::new();

        for (forme, rs) in map {
            let serde_json::Value::Array(list) = rs else {
                continue;
            };
            let off = blob.len() as u32;
            let rstart = lectures.len() as u32;
            for r in &list {
                let (Some(inf), Some(tm)) = (
                    r.get("inf").and_then(|v| v.as_str()),
                    r.get("temps").and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                let Some(ps) = r.get("personnes").and_then(|v| v.as_array()) else {
                    continue;
                };
                let iid = if let Some(&id) = inf_id.get(inf) {
                    id
                } else {
                    infs.push(inf.to_string());
                    let id = infs.len() as u32 - 1;
                    inf_id.insert(inf.to_string(), id);
                    id
                };
                let tid = if let Some(&id) = temps_id.get(tm) {
                    id
                } else if temps.len() < usize::from(u8::MAX) {
                    temps.push(tm.to_string());
                    let id = u8::try_from(temps.len() - 1).ok()?;
                    temps_id.insert(tm.to_string(), id);
                    id
                } else {
                    continue; // > 255 temps : u8 saturé, lecture inexploitable
                };
                let mut pers = 0u8;
                for p in ps.iter().filter_map(|v| v.as_str()) {
                    let bit = person_bit(p);
                    pers |= bit;
                    // `-` (infinitif/participes) : personne non conjuguée, pas d'index
                    if p != "-" {
                        // premier gagnant dans l'ordre d'itération JSON (comme avant)
                        inv.entry((iid, tid, bit))
                            .or_insert((off, forme.len() as u32));
                    }
                }
                lectures.push(Reading {
                    inf: iid,
                    temps: tid,
                    pers,
                });
            }
            if lectures.len() as u32 == rstart {
                continue; // aucune lecture valide : forme ignorée (comme avant)
            }
            blob.push_str(&forme);
            blob.push('\n');
            formes.push(Forme {
                off,
                len: forme.len() as u32,
                lect: rstart,
                nlect: lectures.len() as u32 - rstart,
            });
        }
        // recherche binaire exigée : les formes sont rangées dans l'ordre octet
        // (le blob est construit dans l'ordre d'itération du map serde, non trié)
        formes
            .sort_unstable_by(|a, b| str_at(&blob, a.off, a.len).cmp(str_at(&blob, b.off, b.len)));
        Some(Conj {
            blob,
            formes,
            lectures,
            inv,
            infs,
            temps,
        })
    }

    /// Suggestion de cohérence pour `mot` tapé juste avant le délimiteur final ;
    /// `prefix` = buffer avant le mot. `None` = rien à proposer (ou fichier absent).
    pub fn suggest(&self, prefix: &str, mot: &str) -> Option<String> {
        if self.formes.is_empty() {
            return None; // fichier absent : conjugaison désactivée
        }
        // pronom élidé dans le mot (« j'aurais ») : lookup sur la radicale seul,
        // le clitic est ré-attaché à la correction (sinon on écraserait le « j' »)
        let (clitic, base) = split_clitic(mot);
        let base = base.to_lowercase();
        let readings = self.find(&base)?;
        let person = person_of_clitic(clitic).or_else(|| subject_person(prefix));
        // priorité A > B > C : chaîne else-if, une seule popup
        self.rule_a(prefix, readings, &base, person)
            .or_else(|| self.rule_b(prefix, readings, &base, person))
            .or_else(|| self.rule_c(prefix, readings, &base))
            .map(|f| format!("{clitic}{f}"))
    }

    /// Infinitif représenté par `mot` (« mange » → « manger »), ou `None` si le
    /// mot est inconnu ou partagé entre plusieurs verbes (homonymie → silence).
    /// Pas de personne requise ici : `variantes` est le garde-fou.
    pub fn infinitif_de(&self, mot: &str) -> Option<String> {
        if self.formes.is_empty() {
            return None;
        }
        let (_, base) = split_clitic(mot);
        let readings = self.find(&base.to_lowercase())?;
        let mut iid: Option<u32> = None;
        for r in readings {
            match iid {
                None => iid = Some(r.inf),
                Some(i) if i == r.inf => {}
                _ => return None, // deux verbes différents : se taire
            }
        }
        Some(self.inf_str(iid?).to_string())
    }

    /// Synonymes conjugués de `mot` : même temps et personne que la lecture
    /// retenue via `prefix`, clitic ré-attaché. Vide = personne indéductible,
    /// mot inconnu, ou rien de sûr (« mieux vaut se taire qu'à côté »).
    pub fn variantes(&self, prefix: &str, mot: &str, synonymes: &[String]) -> Vec<String> {
        if self.formes.is_empty() {
            return Vec::new();
        }
        let (clitic, base) = split_clitic(mot);
        let base = base.to_lowercase();
        let Some(readings) = self.find(&base) else {
            return Vec::new();
        };
        let Some(person) = person_of_clitic(clitic).or_else(|| subject_person(prefix)) else {
            return Vec::new(); // personne non déductible → aucune cible sûre
        };
        let bit = person_bit(person);
        // infinitif après modal (« je peux regarder ») : formes non conjuguées
        // absentes de l'index inverse → synonymes renvoyés tels quels
        if readings
            .iter()
            .all(|r| self.temps_str(r.temps) == "infinitif")
        {
            return synonymes
                .iter()
                .filter(|s| s.as_str() != base)
                .map(|s| format!("{clitic}{s}"))
                .collect();
        }
        let temps_cibles: Vec<u8> = readings
            .iter()
            .filter(|r| r.pers & bit != 0)
            .map(|r| r.temps)
            .collect();
        if temps_cibles.is_empty() {
            return Vec::new(); // couvert par la règle A si faute de personne
        }
        let mut out = Vec::new();
        'syn: for syn in synonymes {
            let Some(syn_id) = self.infs.iter().position(|i| i == syn) else {
                continue; // synonyme hors table (7 011 verbes) : ignoré
            };
            let mut form: Option<&str> = None;
            for &temps in &temps_cibles {
                let Some(&(off, len)) = self.inv.get(&(syn_id as u32, temps, bit)) else {
                    continue 'syn; // temps/personne inexistant pour ce synonyme
                };
                let f = str_at(&self.blob, off, len);
                match form {
                    None => form = Some(f),
                    Some(x) if x == f => {}
                    Some(_) => continue 'syn, // homonymie : deux formes → silence
                }
            }
            if let Some(f) = form
                && f != base
            {
                out.push(format!("{clitic}{f}"));
            }
        }
        out
    }

    /// Règle A — désaccord sujet/nombre (« ils a » → « ont »).
    fn rule_a(
        &self,
        prefix: &str,
        readings: &[Reading],
        base: &str,
        person: Option<&str>,
    ) -> Option<String> {
        // shortcut: un verbe juste avant (« tu as dit », « j'ai fait ») → le dernier
        // mot n'est peut-être que le participe/infinitif d'un auxiliaire → se taire
        if let Some(prev) = tokens(sentence(prefix)).last()
            && let (_, prev_base) = split_clitic(prev)
            && self.find(prev_base).is_some()
        {
            return None;
        }
        let person = person?; // pas de pronom sujet déductible → se taire
        // mismatch = TOUTES les lectures excluent la personne attendue ;
        // homonymie avec au moins une lecture couvrant le sujet → se taire
        if readings.iter().any(|r| r.pers & person_bit(person) != 0) {
            return None;
        }
        let cibles: Vec<(u32, u8)> = readings.iter().map(|r| (r.inf, r.temps)).collect();
        self.unique_fix(&cibles, person, base)
    }

    /// Règle B — « si » + conditionnel → imparfait (« si j'aurais » → « si j'avais »).
    // shortcut: subjonctif + « si » non concerné (rare et discutable), garder simple
    fn rule_b(
        &self,
        prefix: &str,
        readings: &[Reading],
        base: &str,
        person: Option<&str>,
    ) -> Option<String> {
        let imparfait = self.temps_id("imparfait")?; // libellé hors table → silence (index inexistant avant)
        let cibles: Vec<(u32, u8)> = readings
            .iter()
            .filter(|r| self.temps_str(r.temps) == "conditionnel présent")
            .map(|r| (r.inf, imparfait))
            .collect();
        if cibles.is_empty() {
            return None;
        }
        // « si » (ou « s'il ») dans la phrase en cours, avant le verbe
        // shortcut: « si » n'importe où dans la phrase, pas forcément sujet direct
        if !tokens(sentence(prefix))
            .iter()
            .any(|t| t == "si" || t == "s'il")
        {
            return None;
        }
        let person = person?; // shortcut: pas de pronom → personne indéductible → se taire
        self.unique_fix(&cibles, person, base)
    }

    /// Règle C — accord du participe après verbe d'être (« elle est allé » → « allée »).
    fn rule_c(&self, prefix: &str, readings: &[Reading], base: &str) -> Option<String> {
        // le mot doit être un participe passé d'un verbe à être
        if !readings.iter().any(|r| {
            self.temps_str(r.temps) == "participe passé" && ETRE_AUX.contains(&self.inf_str(r.inf))
        }) {
            return None;
        }
        let mut toks = tokens(sentence(prefix));
        // particules entre le verbe et le participe (« n'est pas allé »)
        while toks
            .last()
            .is_some_and(|t| PARTICULES.contains(&t.as_str()))
        {
            toks.pop();
        }
        // verbe d'être conjugué juste avant : est/était/sera/sont… (lookup, pas de liste)
        let verb = toks.last()?;
        let (_, verb_base) = split_clitic(verb); // « s'est » → « est »
        if !self
            .find(verb_base)?
            .iter()
            .any(|r| self.inf_str(r.inf) == "être")
        {
            return None;
        }
        // sujet pronominal avant le verbe
        // shortcut: sujets nominaux ignorés (« la voiture est allé »), upgrade: GM
        let subj = toks[..toks.len() - 1]
            .iter()
            .rev()
            .take(3)
            .find(|t| *t == "elle" || *t == "elles")?;
        // accord 3 lignes (cf. config::agree) : elle → base+e, elles → base+es
        let stem = base
            .strip_suffix("es")
            .or_else(|| base.strip_suffix('s'))
            .or_else(|| base.strip_suffix('e'))
            .unwrap_or(base);
        let attendu = if subj == "elles" {
            format!("{stem}es")
        } else {
            format!("{stem}e")
        };
        (attendu != base).then_some(attendu)
    }

    /// Forme corrective unique pour (infinitif, temps, personne) parmi `cibles` :
    /// `None` si aucune, ou si l'homonymie donne deux correctifs différents.
    fn unique_fix(&self, cibles: &[(u32, u8)], person: &str, base: &str) -> Option<String> {
        let bit = person_bit(person);
        let mut fix: Option<&str> = None;
        for &(inf, temps) in cibles {
            let Some(&(off, len)) = self.inv.get(&(inf, temps, bit)) else {
                continue; // ex. impératif du 3p n'existe pas → lecture muette
            };
            let f = str_at(&self.blob, off, len);
            match fix {
                None => fix = Some(f),
                Some(x) if x == f => {}
                // homonymie : deux correctifs différents → haute précision, se taire
                Some(_) => return None,
            }
        }
        fix.filter(|&f| f != base).map(str::to_string)
    }

    /// Recherche binaire : plage de lectures de la forme `base`, ou `None`.
    fn find(&self, base: &str) -> Option<&[Reading]> {
        let i = self
            .formes
            .binary_search_by(|f| str_at(&self.blob, f.off, f.len).cmp(base))
            .ok()?;
        let f = self.formes[i];
        let start = f.lect as usize;
        Some(&self.lectures[start..start + f.nlect as usize])
    }

    /// Nom d'un id d'infinitif interné.
    fn inf_str(&self, id: u32) -> &str {
        &self.infs[id as usize]
    }

    /// Libellé d'un id de temps interné.
    fn temps_str(&self, id: u8) -> &str {
        &self.temps[id as usize]
    }

    /// Id d'un libellé de temps (11 entrées : comparaison linéaire négligeable).
    fn temps_id(&self, name: &str) -> Option<u8> {
        self.temps.iter().position(|t| t == name).map(|i| i as u8)
    }
}

/// Sépare le pronom élidé de la radicale (« j'aurais » → « j' » + « aurais »).
fn split_clitic(mot: &str) -> (&str, &str) {
    for (i, ch) in mot.char_indices() {
        if (ch == '\'' || ch == '’') && i > 0 {
            return mot.split_at(i + ch.len_utf8());
        }
    }
    ("", mot)
}

/// Personne portée par le clitic du mot lui-même (« j' » → 1s, « t' » → 2s ;
/// n'/l'/s'… ne portent pas le sujet).
fn person_of_clitic(clitic: &str) -> Option<&'static str> {
    match clitic.trim_end_matches(['\'', '’']) {
        "j" | "je" => Some("1s"),
        "t" | "tu" => Some("2s"),
        _ => None,
    }
}

/// Pronom sujet : dernier pronom parmi les ~4 tokens précédents, dans la phrase
/// en cours (après le dernier `.` `!` `?`). « ne », « y », « lui »… sont ignorés ;
/// le pronom peut être élidé dans un token (« qu'il » → il).
fn subject_person(prefix: &str) -> Option<&'static str> {
    tokens(sentence(prefix))
        .iter()
        .rev()
        .take(4)
        .find_map(|t| pronoun_person(t))
}

fn pronoun_person(tok: &str) -> Option<&'static str> {
    fn find(t: &str) -> Option<&'static str> {
        match t {
            "je" | "j" => Some("1s"),
            "tu" | "t" => Some("2s"),
            "il" | "elle" | "on" => Some("3s"),
            "nous" => Some("1p"),
            "vous" => Some("2p"),
            "ils" | "elles" => Some("3p"),
            _ => None,
        }
    }
    find(tok).or_else(|| {
        let (a, b) = tok.split_once(['\'', '’'])?;
        find(b).or_else(|| find(a))
    })
}

/// Phrase en cours : après le dernier `.` `!` `?` du prefixe.
fn sentence(prefix: &str) -> &str {
    prefix.rsplit(['.', '!', '?']).next().unwrap_or(prefix)
}

/// Tokens d'une phrase : lettres + apostrophes (« qu'il » = 1 token),
/// tout le reste (espaces, ponctuation, tirets) sépare. Minuscules.
fn tokens(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_alphabetic() || ch == '\'' || ch == '’' {
            cur.push(ch);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.iter().map(|t| t.to_lowercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Diagnostic sur vraies données : `cargo test --release diag_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn diag_real() {
        let c = Conj::load().expect("data/conjugaison.json présent");
        println!(
            "find(allé) = {} lectures",
            c.find("allé").map_or(0, |l| l.len())
        );
        println!(
            "suggest(\"elle est \", \"allé\") = {:?}",
            c.suggest("elle est ", "allé")
        );
        println!(
            "suggest(\"elle est \", \"allée\") = {:?}",
            c.suggest("elle est ", "allée")
        );
    }

    /// Mesure manuelle (données réelles) : `cargo test --release mesure_conj -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mesure_conj() {
        let t = Instant::now();
        let c = Conj::load().expect("data/conjugaison.json présent");
        let ms = t.elapsed().as_millis();
        // approximation mémoire vive = somme des tailles de structures (heap inclus)
        let octets = c.blob.len()
            + c.formes.len() * std::mem::size_of::<Forme>()
            + c.lectures.len() * std::mem::size_of::<Reading>()
            + c.inv.capacity() * (std::mem::size_of::<(u32, u8, u8, (u32, u32))>() + 1)
            + c.infs.iter().map(|s| s.len() + 40).sum::<usize>()
            + c.temps.iter().map(|s| s.len() + 40).sum::<usize>();
        println!(
            "APRÈS — chargement: {ms} ms | formes: {} | lectures: {} | index inv: {} | blob: {} o | size_of Reading/Forme: {}/{} B | mémoire ≈ {:.1} Mo",
            c.formes.len(),
            c.lectures.len(),
            c.inv.len(),
            c.blob.len(),
            std::mem::size_of::<Reading>(),
            std::mem::size_of::<Forme>(),
            octets as f64 / (1024.0 * 1024.0)
        );
    }

    /// Indexation ID ↔ chaînes sur la structure compacte (blob trié + internement).
    #[test]
    fn internage_ids_strings() {
        let c = c();
        let lects = c.find("mange").expect("forme présente");
        assert_eq!(lects.len(), 2);
        // ordre du JSON : impératif 2s puis présent 1s/3s, ids décodés à la volée
        assert_eq!(c.inf_str(lects[0].inf), "manger");
        assert_eq!(c.temps_str(lects[0].temps), "impératif");
        assert_eq!(lects[0].pers, person_bit("2s"));
        assert_eq!(c.inf_str(lects[1].inf), "manger");
        assert_eq!(c.temps_str(lects[1].temps), "présent");
        assert_eq!(lects[1].pers, person_bit("1s") | person_bit("3s"));
        // participe non conjugué : bit « - » seul
        let fait = c.find("fait").expect("forme présente");
        assert_eq!(fait[0].pers, person_bit("-"));
        // forme inconnue
        assert!(c.find("zzzz").is_none());
    }

    /// Fixture mini embarquée (~15 formes, avoir/aller/manger + être pour la
    /// règle C) : le vrai `data/conjugaison.json` n'est jamais lu en test.
    const FIXTURE: &str = r#"{
"a":[{"inf":"avoir","temps":"présent","personnes":["3s"]}],
"ai":[{"inf":"avoir","temps":"présent","personnes":["1s"]}],
"as":[{"inf":"avoir","temps":"présent","personnes":["2s"]}],
"avons":[{"inf":"avoir","temps":"présent","personnes":["1p"]}],
"ont":[{"inf":"avoir","temps":"présent","personnes":["3p"]}],
"aurais":[{"inf":"avoir","temps":"conditionnel présent","personnes":["1s","2s"]}],
"avais":[{"inf":"avoir","temps":"imparfait","personnes":["1s","2s"]}],
"mange":[{"inf":"manger","temps":"impératif","personnes":["2s"]},{"inf":"manger","temps":"présent","personnes":["1s","3s"]}],
"mangent":[{"inf":"manger","temps":"présent","personnes":["3p"]}],
"est":[{"inf":"être","temps":"présent","personnes":["3s"]}],
"sont":[{"inf":"être","temps":"présent","personnes":["3p"]}],
"fait":[{"inf":"faire","temps":"participe passé","personnes":["-"]},{"inf":"faire","temps":"présent","personnes":["3s"]}],
"allé":[{"inf":"aller","temps":"participe passé","personnes":["-"]}],
"allée":[{"inf":"aller","temps":"participe passé","personnes":["-"]}],
"allés":[{"inf":"aller","temps":"participe passé","personnes":["-"]}],
"allées":[{"inf":"aller","temps":"participe passé","personnes":["-"]}],
"dîne":[{"inf":"dîner","temps":"impératif","personnes":["2s"]},{"inf":"dîner","temps":"présent","personnes":["1s","3s"]}],
"dînent":[{"inf":"dîner","temps":"présent","personnes":["3p"]}],
"regarder":[{"inf":"regarder","temps":"infinitif","personnes":["-"]}],
"voir":[{"inf":"voir","temps":"infinitif","personnes":["-"]}]
}"#;

    /// Comme app.rs : prefixe = buffer moins le mot, moins le délimiteur final.
    fn c() -> Conj {
        let conj = Conj::from_json(FIXTURE).expect("fixture valide");
        assert!(conj.formes.len() >= 14);
        conj
    }

    #[test]
    fn regle_a_disaccord() {
        let c = c();
        assert_eq!(c.suggest("ils ", "a"), Some("ont".to_string()));
        assert_eq!(c.suggest("nous ", "a"), Some("avons".to_string()));
        // verbe d'être mal conjugué (règle A couvre aussi « elle/ils est »)
        assert_eq!(c.suggest("elles ", "est"), Some("sont".to_string()));
    }

    #[test]
    fn regle_a_silence() {
        let c = c();
        // présent couvre 1s et 3s : pas de mismatch (lecture impératif ignorée)
        assert_eq!(c.suggest("il ", "mange"), None);
        // pas de pronom → rien
        assert_eq!(c.suggest("", "mange"), None);
        // accordé
        assert_eq!(c.suggest("elles ", "ont"), None);
        // mot absent de la table → rien
        assert_eq!(c.suggest("ils ", "zzz"), None);
        // verbe juste avant : « j'ai fait » → « fait » est le participe de l'auxiliaire,
        // pas le verbe du sujet (sans le garde-fou, A proposerait « fais »)
        assert_eq!(c.suggest("j'ai ", "fait"), None);
    }

    #[test]
    fn regle_b_si_conditionnel() {
        let c = c();
        // le clitic « j' » est conservé : la correction remplace tout le mot
        assert_eq!(c.suggest("si ", "j'aurais"), Some("j'avais".to_string()));
        // sans « si » → rien
        assert_eq!(c.suggest("", "j'aurais"), None);
        // sans pronom déductible → rien
        assert_eq!(c.suggest("si ", "aurais"), None);
    }

    #[test]
    fn regle_c_accord_etre() {
        let c = c();
        assert_eq!(c.suggest("elle est ", "allé"), Some("allée".to_string()));
        assert_eq!(c.suggest("elle est ", "allée"), None);
        // sujet masculin : hors motif, on se tait
        assert_eq!(c.suggest("il est ", "allé"), None);
        // particule entre être et participe
        assert_eq!(
            c.suggest("elle n'est pas ", "allé"),
            Some("allée".to_string())
        );
        // pronominal : « s'est » résolu vers « est »
        assert_eq!(c.suggest("elle s'est ", "allé"), Some("allée".to_string()));
        // pas de verbe d'être juste avant → rien
        assert_eq!(c.suggest("elle ", "allé"), None);
    }

    #[test]
    fn variantes_meme_personne_et_temps() {
        let c = c();
        let dîner = ["dîner".to_string()];
        // « je la mange » → présent 1s → « dîne » (clitic absent ici)
        assert_eq!(c.variantes("je la ", "mange", &dîner), vec!["dîne"]);
        // « tu manges » → impératif 2s couvert par « mange » → « dîne »
        assert_eq!(c.variantes("tu ", "mange", &dîner), vec!["dîne"]);
        // « ils mangent » → présent 3p → « dînent »
        assert_eq!(c.variantes("ils ", "mangent", &dîner), vec!["dînent"]);
        // pronom élidé dans le mot : clitic ré-attaché à la variante
        assert_eq!(
            c.variantes("la ", "j'aime", &["adorer".to_string()]),
            Vec::<String>::new() // « aime » absent de la fixture : silence
        );
    }

    #[test]
    fn variantes_silences() {
        let c = c();
        let dîner = ["dîner".to_string()];
        // personne non déductible → rien
        assert!(c.variantes("la ", "mange", &dîner).is_empty());
        // mot inconnu → rien
        assert!(c.variantes("je ", "zzzz", &dîner).is_empty());
        // synonyme hors table → ignoré
        assert!(
            c.variantes("je la ", "mange", &["zzzverbe".to_string()])
                .is_empty()
        );
        // identité : le synonyme est le verbe lui-même → pas de doublon
        assert!(
            c.variantes("je la ", "mange", &["manger".to_string()])
                .is_empty()
        );
    }

    #[test]
    fn variantes_infinitif_verbatim() {
        let c = c();
        // « je peux regarder » → branche infinitif : synonyme renvoyé tel quel
        assert_eq!(
            c.variantes("je peux ", "regarder", &["voir".to_string()]),
            vec!["voir"]
        );
        // l'identité reste filtrée aussi à l'infinitif
        assert!(
            c.variantes("je peux ", "regarder", &["regarder".to_string()])
                .is_empty()
        );
    }

    #[test]
    fn infinitif_de_renvoie_l_infinitif() {
        let c = c();
        assert_eq!(c.infinitif_de("mange"), Some("manger".to_string()));
        assert_eq!(c.infinitif_de("regarder"), Some("regarder".to_string()));
        // clitic ignoré pour l'lookup
        assert_eq!(c.infinitif_de("j'aime"), None); // « aime » absent de la fixture
        assert_eq!(c.infinitif_de("zzzz"), None);
    }
}
