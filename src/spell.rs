//! Orthographe : dico `data/mots.txt` (428k mots) + suggestions à distance ≤ 2.
//! Levenshtein maison (pas de dépendance), méthode `&mut self` → usage mono-thread dans App.
//! Mémoire : tous les mots dans un seul blob trié (recherche binaire) → ≈ 8 Mo
//! au lieu de ~25-35 Mo (avant : `HashSet<String>`, un allocateur par mot).

use std::collections::HashMap;

use crate::config;

fn str_at(blob: &str, off: u32, len: u32) -> &str {
    &blob[off as usize..off as usize + len as usize]
}

pub struct Spell {
    /// dico en minuscules : tous les mots dans une String, séparés par '\n'
    blob: String,
    /// plages (offset, longueur) de `blob`, triées par mot → recherche binaire
    idx: Vec<(u32, u32)>,
    /// suggestions déjà calculées (y compris les `[]`)
    cache: HashMap<String, Vec<String>>,
}

impl Spell {
    /// Charge le dico. Synchrone au démarrage : 428k lignes ≈ 150 ms.
    // shortcut: lu depuis le répertoire courant, upgrade: chemin figé alongside l'exécutable
    // fichier manquant → set vide = vérification désactivée (aucun panic)
    pub fn load() -> Spell {
        let content = std::fs::read_to_string("data/mots.txt").unwrap_or_default();
        Spell::build(content.lines())
    }

    /// Trie et dédoublonne les mots dans le blob. mots.txt est aujourd'hui trié
    /// en octets, mais rien ne l'oblige à le rester (réécriture du fichier).
    fn build<'a>(words: impl IntoIterator<Item = &'a str>) -> Spell {
        let mut sorted: Vec<&str> = words
            .into_iter()
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .collect();
        sorted.sort_unstable();
        sorted.dedup();
        let mut blob = String::new();
        let mut idx = Vec::with_capacity(sorted.len());
        for w in sorted {
            let off = blob.len() as u32;
            blob.push_str(w);
            blob.push('\n');
            idx.push((off, w.len() as u32));
        }
        Spell {
            blob,
            idx,
            cache: HashMap::new(),
        }
    }

    pub fn known(&self, word: &str) -> bool {
        self.idx
            .binary_search_by(|&(off, len)| str_at(&self.blob, off, len).cmp(word))
            .is_ok()
    }

    /// Suggestions pour `word` (minuscules) : au plus 3, distance ≤ 2, triées par
    /// distance puis alphabétiquement. `[]` = mot correct ou aucune idée.
    pub fn suggest(&mut self, word: &str) -> Vec<String> {
        if self.idx.is_empty() {
            return Vec::new(); // dico absent : vérification désactivée
        }
        if let Some(v) = self.cache.get(word) {
            return v.clone();
        }
        if self.known(word) {
            return Vec::new(); // mot correct : pas de cache inutile
        }
        let len = word.len(); // octets, même unité que `w.len()` pour la coupe
        // 1re lettre déaccentuée : « etes » doit voir « êtres », pas seulement « e »…
        let first = word.chars().next().map(deaccent);
        let mut found: Vec<(usize, String)> = Vec::new();
        for &(off, wlen) in &self.idx {
            let w = str_at(&self.blob, off, wlen);
            // coupe grossière : longueur ±2 et même 1re lettre
            if w.len().abs_diff(len) > 2 || w.chars().next().map(deaccent) != first {
                continue;
            }
            // borne précoce : la distance est coupée à 2 dès la 1re ligne trop haute
            let d = levenshtein(w, word, 2);
            if d <= 2 {
                found.push((d, w.to_string()));
            }
        }
        found.sort_unstable(); // (distance, mot) : les plus proches d'abord
        found.truncate(3);
        let out: Vec<String> = found.into_iter().map(|(_, w)| w).collect();
        self.cache.insert(word.to_string(), out.clone());
        out
    }
}

/// Mot juste avant le délimiteur final de `buffer` : (mot tapé, longueur en chars).
/// Recule sur les lettres, apostrophes (droite/courbe) et tirets ; ignore le délimiteur.
pub fn extract_word(buffer: &str) -> Option<(String, usize)> {
    let mut chars: Vec<char> = buffer.chars().collect();
    if chars.last().is_some_and(|c| config::is_delimiter(*c)) {
        chars.pop();
    }
    let end = chars.len();
    let mut start = end;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    Some((chars[start..].iter().collect(), end - start))
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '\'' | '’' | '-')
}

/// Spam : 3 lettres identiques consécutives (« ccccc ») — jamais une faute
/// réelle, on se tait plutôt que de popper à côté.
pub fn spam(mot: &str) -> bool {
    let mut run = 0u8;
    let mut prev = '\0';
    for c in mot.chars() {
        run = if c == prev { run + 1 } else { 1 };
        prev = c;
        if run >= 3 {
            return true;
        }
    }
    false
}

/// Accent → lettre de base : « grace » a distance 0 de « grâce » (sinon les 2
/// substitutions d'accents le font sortir du top-3 au profit de « glace »…).
fn deaccent(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' => 'a',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'ì' => 'i',
        'ô' | 'ö' | 'ò' => 'o',
        'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ÿ' => 'y',
        c => c,
    }
}

/// Distance de Levenshtein bornée : renvoie `max + 1` dès que la distance dépasse `max`.
/// Les minima de ligne ne font que croître → arrêt sûr dès qu'une ligne dépasse la borne.
fn levenshtein(a: &str, b: &str, max: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return max + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=b.len() {
            let cost = usize::from(deaccent(a[i - 1]) != deaccent(b[j - 1]));
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(cur[j]);
        }
        if row_min > max {
            return max + 1;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()].min(max + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn diag_real() {
        let mut s = Spell::load();
        println!("suggest(bonjur) = {:?}", s.suggest("bonjur"));
        println!("known(bonjour) = {}", s.known("bonjour"));
        println!("suggest(elle) = {:?}", s.suggest("elle"));
    }

    /// Mesure manuelle (données réelles) : `cargo test --release mesure_spell -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mesure_spell() {
        let t = std::time::Instant::now();
        let s = Spell::load();
        let ms = t.elapsed().as_millis();
        // approximation mémoire vive = somme des tailles de structures (heap inclus)
        let octets = s.blob.len()
            + s.idx.capacity() * (std::mem::size_of::<(u32, u32)>() + 1)
            + s.cache.capacity() * 64;
        println!(
            "APRÈS — chargement: {ms} ms | mots: {} | blob: {} o | mémoire ≈ {:.1} Mo | size_of::<Spell>(): {} B",
            s.idx.len(),
            s.blob.len(),
            octets as f64 / (1024.0 * 1024.0),
            std::mem::size_of::<Spell>()
        );
    }

    /// Dico de ~20 mots construit à la main : le vrai `data/mots.txt` n'est jamais lu en test.
    fn spell() -> Spell {
        let words = [
            "bonjour",
            "bonsoir",
            "jour",
            "notre",
            "votre",
            "maison",
            "raison",
            "chanson",
            "personne",
            "raisonnable",
            "éléphant",
            "maître",
            "être",
            "avoir",
            "faire",
            "chaque",
            "parce",
            "encore",
            "toujours",
            "souvent",
            "maintenant",
        ];
        Spell::build(words)
    }

    #[test]
    fn mot_connu_sans_suggestion() {
        let mut s = spell();
        assert!(s.suggest("bonjour").is_empty());
        assert!(s.known("bonjour"));
        assert!(!s.known("bonjur"));
        // pas de cache pour un mot correct
        assert!(!s.cache.contains_key("bonjour"));
    }

    #[test]
    fn faute_propose_le_bon_mot() {
        let mut s = spell();
        assert_eq!(s.suggest("bonjur"), vec!["bonjour".to_string()]);
        // le résultat est mis en cache (seconde lecture identique, sans rescaner)
        assert_eq!(s.suggest("bonjur"), vec!["bonjour".to_string()]);
        assert!(s.cache.contains_key("bonjur"));
    }

    #[test]
    fn distance_trop_grande_sans_suggestion() {
        let mut s = spell();
        // même 1re lettre et longueur ±2, mais distance > 2
        assert!(s.suggest("bocal").is_empty());
        // longueur hors bande : aucun candidat
        assert!(s.suggest("ba").is_empty());
        // 1re lettre hors candidats
        assert!(s.suggest("zzzz").is_empty());
        // dico vide = vérification désactivée
        let mut off = Spell::build(std::iter::empty::<&str>());
        assert!(off.suggest("bonjur").is_empty());
        assert!(!off.known("bonjur"));
    }

    /// Bug « grace » / « etes » : sans déaccentuation la distance 2 des accents
    /// noyait le mot juste sous des voisins sans rapport (glace, entes…).
    #[test]
    fn mot_accentue_propose_sans_accent_tape() {
        let mut s = Spell::build(["grâce", "êtes", "même", "glace", "gracie", "grade", "entes"]);
        assert_eq!(
            s.suggest("grace").first().map(String::as_str),
            Some("grâce")
        );
        assert_eq!(s.suggest("etes").first().map(String::as_str), Some("êtes"));
        assert_eq!(s.suggest("meme").first().map(String::as_str), Some("même"));
    }

    #[test]
    fn levenshtein_cas_limites() {
        assert_eq!(levenshtein("kitten", "sitting", 5), 3);
        assert_eq!(levenshtein("chat", "chat", 2), 0);
        assert_eq!(levenshtein("bonjour", "bonjur", 2), 1);
        // borné : au-delà de `max`, la valeur renvoyée est `max + 1`
        assert_eq!(levenshtein("abc", "xyz", 2), 3);
        assert_eq!(levenshtein("abc", "xyzdefgh", 2), 3);
    }

    #[test]
    fn spam_trois_lettres_identiques() {
        assert!(spam("cccc"));
        assert!(spam("aaaab"));
        assert!(spam("xxxy"));
        assert!(!spam("cc"));
        assert!(!spam("bonjour"));
        // égalité seulement : le garde alphabétique vit dans spell_popup
        assert!(spam("333"));
    }

    #[test]
    fn extraction_mot_avant_delimiteur() {
        assert_eq!(
            extract_word("il dit bonjur "),
            Some(("bonjur".to_string(), 6))
        );
        // apostrophe courbe (Word) et droite
        assert_eq!(extract_word("c’est "), Some(("c’est".to_string(), 5)));
        assert_eq!(extract_word("c'est "), Some(("c'est".to_string(), 5)));
        // tiret
        assert_eq!(
            extract_word("peut-être "),
            Some(("peut-être".to_string(), 9))
        );
        // autre délimiteur
        assert_eq!(
            extract_word("il dit bonjur."),
            Some(("bonjur".to_string(), 6))
        );
        // rien avant le délimiteur
        assert_eq!(extract_word(" "), None);
        assert_eq!(extract_word(""), None);
    }
}
