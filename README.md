# Plume

Assistant d'écriture français **100 % local** pour Windows — rien ne quitte jamais
votre machine : aucun compte, aucune API, aucun texte envoyé nulle part.

Plume observe ce que vous tapez (hook clavier global) et propose, au niveau du
curseur, dans une bulle élégante :

| Exemple tapé | Ce que propose Plume |
|---|---|
| `c'est ` | *Il s'agit de · Cela constitue · En fait* (tics de langage) |
| `grande taille ` | adjectifs **accordés** en genre/nombre |
| `bonjur ` | correction d'orthographe (428 000 mots) |
| `ils a ` | conjugaison : « ils **ont** » (règles de cohérence) |
| `je la regarde ` | verbes **synonymes conjugués** : « je l'observe… » (1 100 verbes) |
| `grace ` | suggestions accentuées : « grâce » |

↑↓ pour naviguer, **Entrée** pour insérer, **Échap** pour ignorer. Par défaut,
écrire ferme la bulle et continue la phrase ; cochez **Mode forcé** (menu tray)
pour verrouiller la bulle jusqu'à validation ou annulation.

## Télécharger

Dernière release : [Releases](../../releases) → déziper, lancer `plume.exe`.

Au premier lancement, Plume crée `exclusions.txt` (apps où le clavier doit être
laissé tranquille) et un raccourci `Plume` (menu Démarrer + Bureau).
`tics.txt`, `adjectifs.txt`, `verbes.txt` sont lus au démarrage — tous
éditables à la main (redémarrage pour prendre effet).

## Raccourcis

| Raccourci | Effet |
|---|---|
| `Entrée` | insère l'alternative sélectionnée |
| `↑` `↓` | naviguer dans la bulle |
| `Échap` | fermer sans insérer |
| `Ctrl+Alt+P` | pause totale (reprise : même raccourci) |

## Compiler

```bash
cargo build --release
```

Windows 10+, Rust stable (édition 2024). Les artefacts de données (`data/`)
sont versionnés ; pour les régénérer depuis leurs sources libres, voir
[`tools/README.md`](tools/README.md).

## Architecture

```
src/
├── hook.rs     hook WH_KEYBOARD_LL global : frappes, swallow, dead keys
├── app.rs      état principal : bulle egui, priorités de popup, remplacement
├── spell.rs    dico 428k mots + suggestions (Levenshtein ≤ 2, déaccentué)
├── conj.rs     conjugaison 272k formes + 3 règles de cohérence
├── config.rs   tics.txt / adjectifs.txt / verbes.txt + définitions
└── punct.rs    micro-corrections typographiques
data/           artefacts générés par tools/ (voir leur provenance ci-dessous)
```

## Licences

- **Code** : [MIT](LICENSE).
- **Données `data/`** : sous leurs propres licences d'origine (détails dans
  [`tools/README.md`](tools/README.md)) — conjugaison GPL-2.0-or-later
  (verbecc/conjugate-fr), dictionnaire CC BY-SA 3.0 (French-Dictionary /
  Wiktionnaire), définitions CC BY-SA 4.0 (kaikki.org / fr.wiktionary).
