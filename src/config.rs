use std::collections::{HashMap, HashSet};

/// Entrée `déclencheur|alt1|alt2|alt3` : `tics.txt`, `adjectifs.txt` ou `verbes.txt` (cwd).
pub struct Tic {
    pub trigger: String,
    pub alts: Vec<String>,
}

pub struct Config {
    pub tics: Vec<Tic>,
    /// adjectifs (`adjectifs.txt`) : match avec inflection d'accord en plus
    pub adjs: Vec<Tic>,
    /// verbes (`verbes.txt`) : infinitifs synonymes, conjugués via `Conj::variantes`
    pub verbs: Vec<Tic>,
    /// définitions courtes (`data/defs.txt`) des alternatives proposées (tics +
    /// adjs, quelques centaines de clés sur 362 819 lignes) : affichées à côté
    /// de chaque alt
    pub defs: HashMap<String, String>,
}

pub struct Match {
    pub trigger: String,
    /// longueur en caractères du déclencheur tapé
    pub trigger_len: usize,
    pub capitalized: bool,
    pub alts: Vec<String>,
}

impl Config {
    pub fn load() -> Config {
        let tics = load_file("tics.txt", DEFAULT_TICS);
        let adjs = load_file("adjectifs.txt", DEFAULT_ADJS);
        // seed vide : verbes.txt absent → recréé vide → feature silencieusement off
        let verbs = load_file("verbes.txt", DEFAULT_VERBS);
        // 27 Mo lus une fois au démarrage (~30-80 ms) : acceptable pour un one-shot ;
        // seules les définitions des alternatives (quelques centaines) restent en mémoire.
        // Fichier absent → map vide, la feature se tait silencieusement.
        let defs = std::fs::read_to_string("data/defs.txt")
            .map(|c| filter_defs(&c, &tics, &adjs, &verbs))
            .unwrap_or_default();
        crate::dlog::dlog(&format!("defs: {} chargées", defs.len()));
        Config {
            tics,
            adjs,
            verbs,
            defs,
        }
    }

    /// Synonymes (infinitifs) de `inf` dans verbes.txt, ou `None` si le verbe
    /// n'est pas déclaré.
    pub fn verb_alts(&self, inf: &str) -> Option<&[String]> {
        self.verbs
            .iter()
            .find(|t| t.trigger == inf)
            .map(|t| t.alts.as_slice())
    }

    /// Définition courte d'une alternative proposée (`None` si inconnue :
    /// fréquent en spell/conj, c'est normal et silencieux).
    pub fn def(&self, word: &str) -> Option<&str> {
        let word = normalize(word);
        if let Some(d) = self.defs.get(&word) {
            return Some(d);
        }
        // adjectif décliné : « grande » → « grand », suffixes testés dans l'ordre es, e, s, x ;
        // le résultat n'est retenu que s'il est un déclencheur connu (déjà en mémoire)
        // shortcut: les formes irrégulières dédiées (belle, heureuse…) sont des déclencheurs
        // à part entière → déjà trouvées à l'étape 1 ci-dessus
        for suf in ["es", "e", "s", "x"] {
            if let Some(base) = word.strip_suffix(suf)
                && let Some(d) = self.defs.get(base)
            {
                return Some(d);
            }
        }
        None
    }

    /// `buffer` se termine par le délimiteur qui vient d'être tapé.
    /// Retourne le déclencheur le plus long correspondant au suffixe.
    pub fn match_buffer(&self, buffer: &str) -> Option<Match> {
        let delim = buffer.chars().last()?;
        let lower = normalize(buffer);
        let b: Vec<char> = lower.chars().collect();
        let total = buffer.chars().count();

        // 1) tics : match exact uniquement, priorité absolue (pas d'inflection sur les tics)
        let mut best: Option<(usize, &Tic)> = None;
        for tic in &self.tics {
            let mut pat: Vec<char> = normalize(&tic.trigger).chars().collect();
            let tl = pat.len();
            pat.push(delim);
            if b.len() >= pat.len()
                && boundary(&b, b.len() - pat.len())
                && b[b.len() - pat.len()..] == pat[..]
                && best.is_none_or(|(l, _)| tl > l)
            {
                best = Some((tl, tic));
            }
        }

        // 2) adjectifs : déclencheur + inflection optionnelle (-e, -es, -s, -x) + délimiteur.
        // clé = (longueur de la forme tapée, forme de base sans inflection) : le plus long
        // gagne, à égalité la base exacte l'emporte (« heureuse » > « heureux »+« e »).
        let mut best_adj: Option<((usize, bool), &str, &Tic)> = None;
        for adj in &self.adjs {
            let base: Vec<char> = normalize(&adj.trigger).chars().collect();
            for infl in ["", "e", "es", "s", "x"] {
                let mut pat = base.clone();
                pat.extend(infl.chars());
                pat.push(delim);
                if b.len() < pat.len()
                    || !boundary(&b, b.len() - pat.len())
                    || b[b.len() - pat.len()..] != pat[..]
                {
                    continue;
                }
                let key = (base.len() + infl.chars().count(), infl.is_empty());
                if best_adj.is_none_or(|(k, _, _)| key > k) {
                    best_adj = Some((key, infl, adj));
                }
            }
        }

        let (trigger_len, trigger, alts) = if let Some((tl, tic)) = best {
            (tl, tic.trigger.clone(), tic.alts.clone())
        } else {
            let ((tl, _), infl, adj) = best_adj?;
            // forme tappée (ex. « grande ») : app.rs envoie `trigger_len + 1` backspaces
            let trigger = format!("{}{}", adj.trigger, infl);
            let alts: Vec<String> = adj
                .alts
                .iter()
                .map(|a| agree(a, &trigger, &adj.trigger))
                .collect();
            (tl, trigger, alts)
        };

        let start = total - (trigger_len + 1);
        let first = buffer.chars().nth(start)?;
        // début de phrase : rien avant le déclencheur, ou phrase précédente terminée (. ! ?)
        // shortcut: un collage (buffer vidé) avant le déclencheur peut donner un faux positif
        let prefix: String = buffer.chars().take(start).collect();
        let prefix = prefix.trim_end();
        let sentence_start = prefix.is_empty() || prefix.ends_with(['.', '!', '?']);
        Some(Match {
            trigger,
            trigger_len,
            capitalized: sentence_start || first.is_uppercase(),
            alts,
        })
    }
}

/// Lit un fichier `déclencheur|alt1|…`, le recrée avec `default` s'il manque.
// shortcut: lu dans le répertoire courant, upgrade: chemin figé alongside l'exéutable
fn load_file(path: &str, default: &str) -> Vec<Tic> {
    let content = std::fs::read_to_string(path).unwrap_or_else(|_| {
        let _ = std::fs::write(path, default);
        default.to_string()
    });
    parse(&content)
}

/// Ne garde que les définitions dont la clé (`mot|définition`) est une
/// alternative proposée (pas le mot tapé — la déf porte sur le remplacement) :
/// quelques centaines d'entrées sur 362 819 lignes, mémoire quasi nulle.
fn filter_defs(
    content: &str,
    tics: &[Tic],
    adjs: &[Tic],
    verbs: &[Tic],
) -> HashMap<String, String> {
    let alts: HashSet<String> = tics
        .iter()
        .chain(adjs)
        .chain(verbs)
        .flat_map(|t| t.alts.iter())
        .map(|a| normalize(a))
        .collect();
    let mut out = HashMap::new();
    for line in content.lines() {
        let Some((key, def)) = line.split_once('|') else {
            continue;
        };
        // même normalisation que le matching : clés kaikki en apostrophe courbe (« d’école »)
        let key = normalize(key);
        if alts.contains(&key) && !def.is_empty() {
            out.insert(key, def.to_string());
        }
    }
    out
}

/// Parse le contenu d'un fichier tics/adjectifs (`#` = commentaire, champs en minuscules).
fn parse(content: &str) -> Vec<Tic> {
    content
        .lines()
        .filter(|l| !l.trim_start().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|line| {
            let mut parts = line.split('|');
            let trigger = parts.next()?.trim().to_lowercase();
            // en minuscules : la casse affichée/insérée dépend du contexte (début de phrase)
            let alts: Vec<String> = parts.map(|s| s.trim().to_lowercase()).collect();
            if trigger.is_empty() || alts.is_empty() || alts[0].is_empty() {
                return None;
            }
            Some(Tic { trigger, alts })
        })
        .collect()
}

/// Accorde chaque alternative à la forme tapée.
/// L'inflection est déduite : suffixe de `typed` après `base` (ex. « grande » - « grand » = « e »).
fn agree(alt: &str, typed: &str, base: &str) -> String {
    match typed.strip_prefix(base).unwrap_or("") {
        "" => alt.to_string(),
        "e" => agree_e(alt),
        "s" | "x" => agree_pluriel(alt),
        "es" => agree_pluriel(&agree_e(alt)),
        // shortcut: suffixe inattendu (forme écrite à la main) → alt inchangé
        _ => alt.to_string(),
    }
}

/// Règle `e` : féminin.
fn agree_e(alt: &str) -> String {
    if alt.ends_with('e') {
        alt.to_string()
    } else if let Some(stem) = alt.strip_suffix("eux") {
        format!("{stem}euse") // heureux → heureuse
    } else if let Some(stem) = alt.strip_suffix("eau") {
        format!("{stem}elle") // beau → belle
    } else if let Some(stem) = alt.strip_suffix('f') {
        format!("{stem}ve") // neuf → neuve
    } else if alt.ends_with("el") {
        format!("{alt}le") // cruel → cruelle, substantiel → substantielle
    } else if alt.ends_with("en") || alt.ends_with("on") {
        format!("{alt}ne") // ancien → ancienne, bon → bonne
    } else if alt.ends_with("et") {
        // shortcut: « secret » → « secrette » (accent manquant), entrée dédiée si besoin
        format!("{alt}te")
    } else if alt.ends_with('s') {
        format!("{alt}se") // gros → grosse
    } else if alt.ends_with('x') || alt.ends_with('z') || alt.ends_with("ch") || alt.ends_with('g')
    {
        // shortcut: irréguliers (long → longue) → entrée dédiée dans adjectifs.txt
        alt.to_string()
    } else {
        format!("{alt}e")
    }
}

/// Règle `s`/`x` : pluriel.
fn agree_pluriel(alt: &str) -> String {
    if let Some(stem) = alt.strip_suffix("eau") {
        format!("{stem}eaux") // nouveau → nouveaux
    } else if let Some(stem) = alt.strip_suffix("al") {
        format!("{stem}aux") // animal → animaux
    } else if let Some(stem) = alt.strip_suffix("eu") {
        // shortcut: « bleu » → « bleux » (devrait être « bleus »), cas minoritaire
        format!("{stem}eux")
    } else if alt.ends_with(['s', 'x', 'z']) {
        alt.to_string()
    } else {
        format!("{alt}s")
    }
}

/// minuscules + apostrophes droites (Word transforme ' en ’)
fn normalize(s: &str) -> String {
    // espace insécable (typo FR) ≡ espace pour le matching
    s.to_lowercase().replace('’', "'").replace('\u{00A0}', " ")
}

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn is_delimiter(c: char) -> bool {
    matches!(c, ' ' | ',' | '.' | ';' | ':' | '!' | '?')
}

/// True si `pos` est une frontière de mot dans `b` : début de buffer, ou le
/// caractère avant n'est ni lettre ni apostrophe/tiret — sans ça « à apprendre »
/// déclenche le tic « prendre » (suffixe présent au milieu du mot).
fn boundary(b: &[char], pos: usize) -> bool {
    pos == 0 || !b[pos - 1].is_alphabetic() && !matches!(b[pos - 1], '\'' | '’' | '-')
}

// seed vide : le fichier livré porte les ≥ 1000 verbes ; absent → recréé vide
const DEFAULT_VERBS: &str = "";

const DEFAULT_TICS: &str = "\
# Tics de langage — Plume. Format : déclencheur|alt1|alt2|alt3 (un par ligne, # = commentaire)
c'est|Cela constitue|Il s'agit de|Cela représente
de plus|En outre|Par ailleurs|Qui plus est
en fait|Concrètement|En vérité|Pour vrai
du coup|Par conséquent|Dès lors|Ainsi
par contre|En revanche|Toutefois|Néanmoins
après|Par la suite|Ultérieurement|Plus tard
en vrai|Honnêtement|En toute sincérité|Franchement
quand même|Néanmoins|Malgré tout|Toutefois
maintenant|À présent|Désormais|Dorénavant
donc|Ainsi|Par conséquent|En conséquence
très|Particulièrement|Extrêmement|Hautement
vraiment|Réellement|Effectivement|Singulièrement
super|Remarquable|Excellent|Parfait
super bien|À merveille|À souhait|À ravir
clairement|Manifestement|Sans équivoque|De façon évidente
honnêtement|En toute franchise|Pour être direct|Sincèrement
forcément|Inévitablement|Nécessairement|Logiquement
simplement|Tout bonnement|Uniquement|Rien de plus
globalement|Dans l'ensemble|En règle générale|De manière générale
tout simplement|À la limite|Uniquement|Rien de plus
un peu|Légèrement|Un tantinet|Modérément
il y a|On recense|Il existe|On compte
au final|Finalement|En définitive|En somme
finalement|En définitive|En somme|Pour finir
petit à petit|Progressivement|Graduellement|Pas à pas
de plus en plus|Progressivement|De jour en jour|De manière croissante
à la fin|À l'issue|Pour finir|Une fois terminé
tout d'abord|Premièrement|En premier lieu|D'emblée
le fait que|Ce que|Ce constat que|Ce qui
cela dit|Toutefois|Cela étant|Néanmoins
en ce qui concerne|Concernant|S'agissant de|Pour ce qui est de
il faut|Nous devons|Je dois|Il est impératif de
on peut|Nous pouvons|Il est possible de|Il est envisageable de
permet de|Offre la possibilité de|Autorise|Ouvre la voie à
afin de|Pour|De façon à|Dans le but de
grâce à|Via|Moyennant|Par l'intermédiaire de
d'un certain point de vue|Sous cet angle|De ce point de vue|Vu sous cet angle
à mon avis|Je considère|Pour ma part|Selon moi
quelque part|D'une certaine manière|À certains égards|En un sens
intéressant|Pertinent|Éclairant|Stimulant
important|Essentiel|Capital|Déterminant
basique|Élémentaire|Fondamental|Courant
simple|Limpide|Évident|Aisé
bon|Solide|Pertinent|Éprouvé
bonne|Solide|Pertinente|Éprouvée
gros|Considérable|Substantiel|Marqué
grosse|Considérable|Substantielle|Marquée
difficile|Complexe|Ardu|Corsé
facile|Fluide|Limpide|Aisé
magnifique|Superbe|Éblouissant|Merveilleux
génial|Remarquable|Exceptionnel|Abouti
critique|Pivotal|Décisif|Central
crucial|Essentiel|Vital|Déterminant
chose|Élément|Notion|Détail
truc|Élément|Affaire|Détail
genre|Notamment|Comme|Entre autres
en réalité|De fait|En vérité|Concrètement
";

const DEFAULT_ADJS: &str = "\
# Adjectifs — Plume. Format : déclencheur|alt1|alt2|alt3 (un par ligne, # = commentaire)
# La forme tapée peut être accordée (-e, -es, -s, -x) : les alternatives s'accordent avec.
# Les formes irrégulières (beau, heureux…) ont leur propre ligne, pas de règle dédiée.
grand|Considérable|Substantiel|Étendu
petit|Modeste|Restreint|Menu
bon|Solide|Pertinent|Éprouvé
bonne|Solide|Pertinente|Éprouvée
mauvais|Défectueux|Périmé|Blâmable
joli|Élégant|Recherché|Avenant
beau|Joli|Élégant|Superbe
belle|Jolie|Élégante|Superbe
heureux|Comblé|Favorisé|Gratifié
heureuse|Comblée|Favorisée|Gratifiée
nouveau|Récent|Inédit|Fraîchement
nouvelle|Récente|Inédite|Tout juste paru
rapide|Prompt|Véloce|Hâtif
lent|Mesuré|Posé|Tranquille
gros|Considérable|Substantiel|Marqué
long|Étendu|Prolixe|Continu
court|Concis|Summaire|Expéditif
chaud|Brûlant|Ardent|Cuisant
froid|Glacial|Gelé|Impassible
jeune|Tendre|Récent|Novice
vieux|Ancien|Vénérable|Honoré
neuf|Inédit|Actuel|Intact
clair|Évident|Manifeste|Lumineux
sombre|Funèbre|Obscur|Ténébreux
facile|Fluide|Limpide|Aisé
difficile|Complexe|Ardu|Corsé
simple|Limpide|Évident|Aisé
important|Essentiel|Capital|Déterminant
intéressant|Pertinent|Éclairant|Stimulant
utile|Salutaire|Propice|Pratique
nécessaire|Indispensable|Crucial|Vital
inutile|Vain|Superflu|Stérile
haut|Élevé|Sublime|Notable
bas|Infime|Modeste|Superficiel
large|Ample|Vaste|Étendu
étroit|Serré|Réduit|Confiné
plein|Rempli|Comble|Garni
vide|Dénudé|Nu|Dépourvu
vrai|Authentique|Sincère|Fondé
faux|Erroné|Fallacieux|Illusoire
franc|Sincère|Ouvert|Direct
parfait|Excellent|Abouti|Irréprochable
médiocre|Passable|Ordinaire|Banal
rare|Exceptionnel|Insolite|Hors norme
fréquent|Habituel|Usuel|Courant
commun|Ordinaire|Courant|Répandu
solide|Robuste|Ferme|Assuré
fragile|Délicat|Frêle|Vulnérable
sain|Salubre|Vigoureux|Robuste
aimable|Bienveillant|Agréable|Attentionné
poli|Courtois|Respectueux|Délicat
fier|Digne|Noble|Superbe
lourd|Pesant|Massif|Accablant
mince|Frêle|Délicat|Menu
riche|Aisé|Fortuné|Opulent
pauvre|Modeste|Dénué|Chétif
sûr|Assuré|Certain|Fiable
moderne|Actuel|Contemporain|Récent
ancien|Antique|Vénérable|Usé
";

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            tics: vec![
                Tic {
                    trigger: "c'est".into(),
                    alts: vec!["Il s'agit de".into()],
                },
                Tic {
                    trigger: "de plus".into(),
                    alts: vec!["En outre".into()],
                },
                Tic {
                    trigger: "plus".into(),
                    alts: vec!["Davantage".into()],
                },
                Tic {
                    trigger: "super".into(),
                    alts: vec!["Excellent".into()],
                },
                Tic {
                    trigger: "super bien".into(),
                    alts: vec!["À merveille".into()],
                },
                Tic {
                    trigger: "prendre".into(),
                    alts: vec!["Saisir".into()],
                },
            ],
            adjs: vec![],
            verbs: vec![],
            defs: HashMap::new(),
        }
    }

    /// Config centrée sur les adjectifs (construite à la main, fichier réel intact).
    fn cfg_adjs() -> Config {
        Config {
            tics: vec![],
            adjs: vec![
                Tic {
                    trigger: "grand".into(),
                    alts: vec!["considérable".into(), "substantiel".into(), "étendu".into()],
                },
                Tic {
                    trigger: "beau".into(),
                    alts: vec!["joli".into(), "élégant".into(), "superbe".into()],
                },
                Tic {
                    trigger: "belle".into(),
                    alts: vec!["jolie".into(), "élégante".into(), "superbe".into()],
                },
                Tic {
                    trigger: "heureux".into(),
                    alts: vec!["comblé".into(), "favorisé".into(), "gratifié".into()],
                },
                Tic {
                    trigger: "heureuse".into(),
                    alts: vec!["comblée".into(), "favorisée".into(), "gratifiée".into()],
                },
                Tic {
                    trigger: "nouveau".into(),
                    alts: vec!["récent".into(), "inédit".into(), "fraîchement".into()],
                },
                Tic {
                    trigger: "nouvelle".into(),
                    alts: vec!["récente".into(), "inédite".into()],
                },
            ],
            verbs: vec![],
            defs: HashMap::new(),
        }
    }

    #[test]
    fn suffixe_insensible_casse() {
        let c = cfg();
        let m = c.match_buffer("il dit c'est ").unwrap();
        assert_eq!(m.trigger, "c'est");
        assert_eq!(m.trigger_len, 5);
        assert!(!m.capitalized);
        let m = c.match_buffer("C'est ").unwrap();
        assert!(m.capitalized);
    }

    #[test]
    fn le_plus_long_gagne() {
        let c = cfg();
        assert_eq!(c.match_buffer("de plus ").unwrap().trigger, "de plus");
        assert_eq!(c.match_buffer("super bien ").unwrap().trigger, "super bien");
        // sans délimiteur final, pas de match
        assert!(c.match_buffer("c'est").is_none());
        // autre suffixe que le déclencheur
        assert!(c.match_buffer("investir ").is_none());
    }

    #[test]
    fn apostrophe_courbe() {
        let c = cfg();
        assert!(c.match_buffer("il dit c’est ").is_some());
    }

    /// Le déclencheur n'est valable qu'en frontière de mot : signalé sur
    /// « Tu as encore à apprendre » qui déclenchait le tic « prendre ».
    #[test]
    fn frontiere_de_mot() {
        let c = cfg();
        assert!(c.match_buffer("Tu as encore à apprendre ").is_none());
        assert_eq!(c.match_buffer("il va prendre ").unwrap().trigger, "prendre");
        let c = cfg_adjs();
        assert!(c.match_buffer("malheureux ").is_none());
        assert_eq!(
            c.match_buffer("il est heureux ").unwrap().trigger,
            "heureux"
        );
    }

    #[test]
    fn capititalisation_reprise() {
        assert_eq!(capitalize("il s'agit de"), "Il s'agit de");
        assert_eq!(capitalize("déjà"), "Déjà");
    }

    #[test]
    fn debut_de_phrase_detecte() {
        let c = cfg();
        // tout début : phrase ouverte
        assert!(c.match_buffer("c'est ").unwrap().capitalized);
        // après . ! ? : nouvelle phrase
        assert!(
            c.match_buffer("il dit bonjour. c'est ")
                .unwrap()
                .capitalized
        );
        assert!(c.match_buffer("pourquoi ? c'est ").unwrap().capitalized);
        // au milieu d'une phrase, en minuscule : non
        assert!(!c.match_buffer("il dit c'est ").unwrap().capitalized);
        // milieu de phrase mais frappe déjà en majuscule : on suit la frappe
        assert!(c.match_buffer("il dit C'est ").unwrap().capitalized);
    }

    #[test]
    fn adjectifs_defaut_parse() {
        let adjs = parse(DEFAULT_ADJS);
        assert!(adjs.len() >= 50, "{} entrées seulement", adjs.len());
        assert_eq!(adjs[0].trigger, "grand");
        let belle = adjs.iter().find(|t| t.trigger == "belle").unwrap();
        assert_eq!(belle.alts, vec!["jolie", "élégante", "superbe"]);
        // les déclencheurs sont en minuscules, comme tics.txt
        assert!(adjs.iter().all(|t| t.trigger == t.trigger.to_lowercase()));
    }

    #[test]
    fn adjectif_accorde() {
        let c = cfg_adjs();
        // forme féminine tapée : déclencheur = « grande », alts accordés
        let m = c.match_buffer("une taille grande ").unwrap();
        assert_eq!(m.trigger, "grande");
        assert_eq!(m.trigger_len, 6); // + le délimiteur géré par app.rs
        assert!(!m.capitalized);
        assert_eq!(m.alts, vec!["considérable", "substantielle", "étendue"]);
        // singulier inchangé
        let m = c.match_buffer("un grand ").unwrap();
        assert_eq!(m.trigger, "grand");
        assert_eq!(m.trigger_len, 5);
        assert_eq!(m.alts[0], "considérable");
        // pluriel
        let m = c.match_buffer("des grands ").unwrap();
        assert_eq!(m.trigger, "grands");
        assert_eq!(m.trigger_len, 6);
        assert_eq!(m.alts, vec!["considérables", "substantiels", "étendus"]);
        let m = c.match_buffer("des grandes ").unwrap();
        assert_eq!(m.trigger, "grandes");
        assert_eq!(m.trigger_len, 7);
        assert_eq!(m.alts, vec!["considérables", "substantielles", "étendues"]);
    }

    #[test]
    fn adjectif_capitalisation() {
        let c = cfg_adjs();
        assert!(!c.match_buffer("une taille grande ").unwrap().capitalized);
        assert!(c.match_buffer("Une taille GRANDE ").unwrap().capitalized);
        assert!(c.match_buffer("Voici. Grande ").unwrap().capitalized);
    }

    #[test]
    fn tic_prioritaire_sur_adjectif() {
        let mut c = cfg_adjs();
        c.tics = vec![Tic {
            trigger: "grand".into(),
            alts: vec!["vaste".into()],
        }];
        // même longueur : le tic gagne
        let m = c.match_buffer("un grand ").unwrap();
        assert_eq!(m.trigger, "grand");
        assert_eq!(m.alts, vec!["vaste"]);
        // l'inflection ne s'applique jamais aux tics : seul l'adjectif matche « grande »
        let m = c.match_buffer("une grande ").unwrap();
        assert_eq!(m.trigger, "grande");
        assert_eq!(m.alts, vec!["considérable", "substantielle", "étendue"]);
    }

    #[test]
    fn adjectif_irregulier_entree_dediee() {
        let c = cfg_adjs();
        // « heureuse » : l'entrée dédiée l'emporte sur « heureux »+« e », alts inchangés
        let m = c.match_buffer("une vie heureuse ").unwrap();
        assert_eq!(m.trigger, "heureuse");
        assert_eq!(m.trigger_len, 8);
        assert_eq!(m.alts, vec!["comblée", "favorisée", "gratifiée"]);
        // beau → belle : déclencheur = forme tapée, alts accordés
        let m = c.match_buffer("une belle ").unwrap();
        assert_eq!(m.trigger, "belle");
        assert_eq!(m.alts, vec!["jolie", "élégante", "superbe"]);
        let m = c.match_buffer("un beau ").unwrap();
        assert_eq!(m.trigger, "beau");
        // nouveau → nouvelle
        let m = c.match_buffer("une nouvelle ").unwrap();
        assert_eq!(m.trigger, "nouvelle");
        assert_eq!(m.alts, vec!["récente", "inédite"]);
    }

    #[test]
    fn agree_regles() {
        // inflection e : inchangé si déjà « e », cas spéciaux, +e sinon
        assert_eq!(agree("considérable", "grande", "grand"), "considérable");
        assert_eq!(agree("substantiel", "grande", "grand"), "substantielle");
        assert_eq!(agree("étendu", "grande", "grand"), "étendue");
        assert_eq!(agree("joli", "grande", "grand"), "jolie");
        assert_eq!(agree("beau", "grande", "grand"), "belle"); // eau → elle
        assert_eq!(agree("heureux", "grande", "grand"), "heureuse"); // eux → euse
        assert_eq!(agree("neuf", "grande", "grand"), "neuve"); // f → ve
        assert_eq!(agree("bon", "grande", "grand"), "bonne"); // on → ne
        assert_eq!(agree("gros", "grande", "grand"), "grosse"); // s → se
        // pluriel
        assert_eq!(agree("considérable", "grands", "grand"), "considérables");
        assert_eq!(agree("animal", "grands", "grand"), "animaux");
        assert_eq!(agree("nouveau", "grands", "grand"), "nouveaux");
        // es = règle e puis règle s
        assert_eq!(agree("considérable", "grandes", "grand"), "considérables");
        assert_eq!(agree("étendu", "grandes", "grand"), "étendues");
        // forme dédiée sans inflection : inchangé
        assert_eq!(agree("comblée", "heureuse", "heureuse"), "comblée");
        // suffixe inattendu : inchangé
        assert_eq!(agree("joli", "grandex", "grand"), "joli");
    }

    #[test]
    fn filter_defs_garde_les_alts_pas_le_mot_tape() {
        let tics = vec![Tic {
            trigger: "c'est".into(),
            alts: vec!["il s'agit de".into()],
        }];
        let adjs = vec![Tic {
            trigger: "grand".into(),
            alts: vec!["considérable".into(), "substantiel".into()],
        }];
        let verbs = vec![Tic {
            trigger: "regarder".into(),
            alts: vec!["voir".into(), "observer".into()],
        }];
        let content = "\
grand|Définition de grand.
petit|Définition de petit.
considérable|Définition de considérable.
il s’agit de|Définition de il s'agit de.
voir|Définition de voir.
substantiel|
ligne sans pipe du tout
";
        let defs = filter_defs(content, &tics, &adjs, &verbs);
        // le mot tapé (« grand ») n'est PAS une clé : la déf va aux alternatives ;
        // « petit » hors liste, définition vide, ligne sans pipe → ignorés ;
        // apostrophe courbe normalisée → « il s'agit de » ; alts de verbes inclus
        assert_eq!(defs.len(), 3, "{defs:?}");
        assert_eq!(defs["considérable"], "Définition de considérable.");
        assert_eq!(defs["il s'agit de"], "Définition de il s'agit de.");
    }

    #[test]
    fn verb_alts_de_verbes_txt() {
        let c = Config {
            tics: vec![],
            adjs: vec![],
            verbs: parse("regarder|voir|observer\nfaire|réaliser|exécuter\n"),
            defs: HashMap::new(),
        };
        assert_eq!(
            c.verb_alts("regarder"),
            Some(&["voir".to_string(), "observer".to_string()][..])
        );
        assert!(c.verb_alts("manger").is_none());
    }

    #[test]
    fn def_declencheur_accorde_et_inconnu() {
        let mut c = cfg();
        c.defs = HashMap::from([
            ("grand".to_string(), "Déf. grand".to_string()),
            ("c'est".to_string(), "Déf. c'est".to_string()),
            ("belle".to_string(), "Déf. belle".to_string()),
        ]);
        assert_eq!(c.def("Grand"), Some("Déf. grand")); // casse
        assert_eq!(c.def("c’est"), Some("Déf. c'est")); // apostrophe courbe
        // adjectif décliné : suffixes es, e, s, x testés dans cet ordre
        assert_eq!(c.def("grandes"), Some("Déf. grand"));
        assert_eq!(c.def("grande"), Some("Déf. grand"));
        assert_eq!(c.def("grands"), Some("Déf. grand"));
        assert_eq!(c.def("grandx"), Some("Déf. grand"));
        // forme irrégulière dédiée : trouvée à l'étape 1
        assert_eq!(c.def("belle"), Some("Déf. belle"));
        // inconnu, ou base sans définition → pas de déf
        assert_eq!(c.def("petit"), None);
        assert_eq!(c.def("petites"), None);
        assert_eq!(c.def(""), None);
    }
}
