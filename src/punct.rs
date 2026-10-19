//! Corrections typographiques françaises silencieuses (pas de bulle).
//! Zéro appel enigo ici : uniquement la décision, l'application se fait dans `App::on_msg`.

/// Ce que l'application cible doit faire pour corriger le caractère fraîchement frappé.
#[derive(Debug, PartialEq, Eq)]
pub enum Fix {
    /// Annuler le dernier caractère injecté (et ne pas le pousser dans le buffer).
    CancelLast,
    /// Retirer `backspaces` caractères déjà présents, insérer `insert`
    /// (qui contient le caractère final selon la règle).
    Rewrite { backspaces: usize, insert: String },
}

/// Décide des corrections pour `c` ; `buffer` est le suffixe cru de l'application
/// SANS `c` (le push se fait ensuite, dans `App::on_msg`).
/// Le caractère vient d'être injecté par l'OS avant notre traitement.
pub fn corrections(c: char, buffer: &str) -> Vec<Fix> {
    let last = buffer.chars().last();

    // 1) apostrophe droite après une lettre → courbe
    if c == '\'' && last.is_some_and(char::is_alphabetic) {
        return vec![Fix::Rewrite {
            backspaces: 1,
            insert: '’'.to_string(),
        }];
    }
    // 2) double espace : annuler le 2e (le 1er a déjà servi de délimiteur)
    if c == ' ' && last == Some(' ') {
        return vec![Fix::CancelLast];
    }
    // 3) espace insécable avant ; : ! ? » › après un mot/digit
    // (`.` et `,` exclus : jamais d'espace avant)
    if matches!(c, ';' | ':' | '!' | '?' | '»' | '›') && last.is_some_and(char::is_alphanumeric)
    {
        return vec![Fix::Rewrite {
            backspaces: 1,
            insert: format!("\u{00A0}{c}"),
        }];
    }
    // 4) espace insécable après « ‹ avant une lettre
    // shortcut: `(` ignoré — pas d'espace avant le contenu d'une parenthèse en français,
    // upgrade: règle dédiée si un jour demandé
    if c.is_alphabetic() && matches!(last, Some('«' | '‹')) {
        return vec![Fix::Rewrite {
            backspaces: 1,
            insert: format!("\u{00A0}{c}"),
        }];
    }
    // 5) guillemet droit → français : ouvrant après début/espace (la nbsp viendra
    // de la règle 4 au mot suivant), fermant sinon (nbsp comme la règle 3)
    // shortcut: `"` après `(` ou `—` lu comme fermant, cas rare
    if c == '"' {
        return vec![Fix::Rewrite {
            backspaces: 1,
            insert: match last {
                None | Some(' ') | Some('\u{00A0}') => "«".to_string(),
                Some(c)
                    if c.is_alphanumeric()
                        || matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '…') =>
                {
                    "\u{00A0}»".to_string()
                }
                _ => "»".to_string(),
            },
        }];
    }
    // 6) guillemets depuis l'ASCII : le caractère fraîchement frappé est déjà
    // dans l'app → les backspaces le comptent. `<<`/`>>` resserrent la paire
    // (la nbsp éventuellement posée devant ‹/› est conservée : elle précède
    // le guillemet remplacé, pas le caractère tapé)
    if c == '<' {
        return vec![Fix::Rewrite {
            backspaces: if last == Some('‹') { 2 } else { 1 },
            insert: if last == Some('‹') { "«" } else { "‹" }.to_string(),
        }];
    }
    if c == '>' {
        return vec![Fix::Rewrite {
            backspaces: if last == Some('›') { 2 } else { 1 },
            insert: if last == Some('›') {
                "»".to_string()
            } else if last.is_some_and(char::is_alphanumeric) {
                "\u{00A0}›".to_string()
            } else {
                "›".to_string()
            },
        }];
    }
    // 7) tirets : `--` → cadratin court, `–-`/`—-` → cadratin long (le tiret
    // simple reste un trait d'union). `.` : `...` → ellipsis, `….` se resserre
    if c == '-' {
        let insert = if last == Some('-') { "–" } else { "—" };
        return if matches!(last, Some('-' | '–' | '—')) {
            vec![Fix::Rewrite {
                backspaces: 2,
                insert: insert.to_string(),
            }]
        } else {
            vec![]
        };
    }
    if c == '.' && (buffer.ends_with("..") || last == Some('…')) {
        return vec![Fix::Rewrite {
            backspaces: if buffer.ends_with("..") { 3 } else { 2 },
            insert: '…'.to_string(),
        }];
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apostrophe_droite_devient_courbe() {
        assert_eq!(
            corrections('\'', "il"),
            vec![Fix::Rewrite {
                backspaces: 1,
                insert: '’'.to_string()
            }]
        );
        // pas de lettre avant : rien (début de buffer, chiffres)
        assert!(corrections('\'', "").is_empty());
        assert!(corrections('\'', "1990").is_empty());
        // déjà une apostrophe courbe : rien
        assert!(corrections('\'', "c’").is_empty());
    }

    #[test]
    fn double_espace_annule() {
        assert_eq!(corrections(' ', "mot "), vec![Fix::CancelLast]);
        // simple espace : délimiteur normal, aucune correction
        assert!(corrections(' ', "mot").is_empty());
    }

    #[test]
    fn espace_insécable_avant_signes() {
        for c in [';', ':', '!', '?', '»'] {
            assert_eq!(
                corrections(c, "mot"),
                vec![Fix::Rewrite {
                    backspaces: 1,
                    insert: format!("\u{00A0}{c}")
                }]
            );
        }
        // déjà correct : espace avant le signe
        assert!(corrections('?', "mot ").is_empty());
        // virgule et point : jamais d'espace avant
        assert!(corrections(',', "mot").is_empty());
        assert!(corrections('.', "mot").is_empty());
        // début de buffer
        assert!(corrections('!', "").is_empty());
    }

    #[test]
    fn espace_apres_guillemet_ouvrant() {
        assert_eq!(
            corrections('c', "«"),
            vec![Fix::Rewrite {
                backspaces: 1,
                insert: "\u{00A0}c".to_string()
            }]
        );
        // déjà suivi d'espace : rien
        assert!(corrections('c', "« ").is_empty());
        // début de buffer (pas de « avant) : rien
        assert!(corrections('c', "").is_empty());
    }

    #[test]
    fn lettres_non_corrigees_au_défaut() {
        // lettre ailleurs qu'après « : aucune correction
        assert!(corrections('x', "mot").is_empty());
        // autres ponctuations hors liste
        assert!(corrections('(', "mot").is_empty());
    }

    #[test]
    fn guillemet_droit_devient_français() {
        // ouvrant : début de buffer ou après espace
        for buf in ["", "il a dit "] {
            assert_eq!(
                corrections('"', buf),
                vec![Fix::Rewrite {
                    backspaces: 1,
                    insert: '«'.to_string()
                }]
            );
        }
        // fermant après mot : espace insécable + »
        assert_eq!(
            corrections('"', "bonjour"),
            vec![Fix::Rewrite {
                backspaces: 1,
                insert: "\u{00A0}»".to_string()
            }]
        );
        // fermant après ponctuation (« Bonjour ! »)
        assert_eq!(
            corrections('"', "dit !"),
            vec![Fix::Rewrite {
                backspaces: 1,
                insert: "\u{00A0}»".to_string()
            }]
        );
        // déjà fermé par une parenthèse : » nu (le contexte entoure déjà)
        assert_eq!(
            corrections('"', "bonjour)"),
            vec![Fix::Rewrite {
                backspaces: 1,
                insert: '»'.to_string()
            }]
        );
    }

    #[test]
    fn guillemets_depuis_ascii() {
        // < → ‹, << → « (2 backspaces : le 2e < fraîchement frappé + le ‹)
        assert_eq!(
            corrections('<', "mot"),
            vec![Fix::Rewrite {
                backspaces: 1,
                insert: '‹'.to_string()
            }]
        );
        assert_eq!(
            corrections('<', "‹"),
            vec![Fix::Rewrite {
                backspaces: 2,
                insert: '«'.to_string()
            }]
        );
        // > après mot → espace insécable + › ; sans mot → › nu
        assert_eq!(
            corrections('>', "mot"),
            vec![Fix::Rewrite {
                backspaces: 1,
                insert: "\u{00A0}›".to_string()
            }]
        );
        assert_eq!(
            corrections('>', ""),
            vec![Fix::Rewrite {
                backspaces: 1,
                insert: '›'.to_string()
            }]
        );
        // >> → » : la nbsp posée devant ‹/› est conservée (2e cas ci-dessus)
        assert_eq!(
            corrections('>', "mot\u{00A0}›"),
            vec![Fix::Rewrite {
                backspaces: 2,
                insert: '»'.to_string()
            }]
        );
        // règles 3/4 étendues aux guillemets simples tapés directement
        assert_eq!(
            corrections('›', "mot"),
            vec![Fix::Rewrite {
                backspaces: 1,
                insert: "\u{00A0}›".to_string()
            }]
        );
        assert_eq!(
            corrections('x', "‹"),
            vec![Fix::Rewrite {
                backspaces: 1,
                insert: "\u{00A0}x".to_string()
            }]
        );
    }

    #[test]
    fn tirets_et_ellipsis() {
        // -- → cadratin court, –- → cadratin long
        assert_eq!(
            corrections('-', "-"),
            vec![Fix::Rewrite {
                backspaces: 2,
                insert: '–'.to_string()
            }]
        );
        assert_eq!(
            corrections('-', "–"),
            vec![Fix::Rewrite {
                backspaces: 2,
                insert: '—'.to_string()
            }]
        );
        // trait d'union simple : intact
        assert!(corrections('-', "porte").is_empty());
        assert!(corrections('-', "").is_empty());
        // ... → ellipsis ; … se resserre quand un 4e point arrive
        assert_eq!(
            corrections('.', ".."),
            vec![Fix::Rewrite {
                backspaces: 3,
                insert: '…'.to_string()
            }]
        );
        assert_eq!(
            corrections('.', "…"),
            vec![Fix::Rewrite {
                backspaces: 2,
                insert: '…'.to_string()
            }]
        );
        // points isolés : fin de phrase normale, inchangé
        assert!(corrections('.', "mot.").is_empty());
        assert!(corrections('.', ".").is_empty());
    }
}
