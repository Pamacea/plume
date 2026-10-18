# tools/ — générateur des artefacts de données (phase 4)

Script reproductible qui télécharge trois sources libres, les convertit et
régénère les artefacts de `data/` consommés par les phases suivantes
(orthographe, cohérence de conjugaison, bulle de définition). 100 % local :
rien n'est envoyé ailleurs que les téléchargements de sources.

## Artefacts produits

| Fichier | Contenu | Taille constatée |
|---|---|---|
| `data/conjugaison.json` | `{forme: [{inf, temps, personnes}]}` — 272 349 formes, 330 783 lectures | 24,2 Mo |
| `data/mots.txt` | 428 452 mots minuscules, alphabétiques, triés (UTF-8, un par ligne) | 4,8 Mo |
| `data/defs.txt` | 362 819 `mot\|définition` (1ʳᵉ définition française, clé minuscule, ligne ≤ 140 caractères) | 27,1 Mo |

Temps produits : `présent`, `imparfait`, `futur`, `conditionnel présent`,
`passé simple`, `subjonctif présent`, `subjonctif imparfait` (= passé du
subjonctif), `impératif`, `infinitif`, `participe présent`, `participe passé`.
Personnes : `1s 2s 3s 1p 2p 3p`, `["-"]` pour infinitif/participes.

## Régénérer

```bash
cd tools
cargo run --release              # phase 4 : conjugaison.json + mots.txt
cargo run --release --bin defs   # bulle : data/defs.txt
# ou, depuis la racine du dépôt :
cargo run --release --manifest-path tools/Cargo.toml
cargo run --release --manifest-path tools/Cargo.toml --bin defs
```

Workspace indépendant (`tools/Cargo.toml` contient son propre `[workspace]`,
le `Cargo.toml` racine n'est pas touché). Dépendances Rust minimales :
`serde_json` (dump kaikki), `flate2` (décompression pur Rust, sans gzip
système) ; `curl` (fourni par Windows 10+) pour les téléchargements, parseur
XML maison pour la conjugaison.

Alternatives des fichiers de tics/adjectifs : `node tools/enrich.mjs [--dry]`
(lit `tools/enrich-data.json`, append les `|alt` manquants, idempotent,
n'écrase jamais le texte existant).

Les sources sont mises en cache dans `tools/cache/` : si le réseau tombe, le
script réutilise le cache (il ne réécrit les artefacts qu'après une
conversion complète). Supprimer `tools/cache/` force un téléchargement neuf.

## Sources, licences, URLs

| Source | Fichiers | URL | Licence |
|---|---|---|---|
| **conjugate-fr** (Pierre Sarrazin), embarqué dans **verbecc** (Brett Tolbert) | `verbs-fr.xml` (7 011 verbes → template), `conjugations-fr.xml` (146 templates) | `https://raw.githubusercontent.com/bretttolbert/verbecc/main/verbecc/data/xml/{verbs,conjugations}/…` | **GPL-2.0-or-later** (en-tête des fichiers XML ; dépôt verbecc : LGPL-3.0) |
| **French-Dictionary** (Hussem Ben Belgacem), dérivé de **kaikki.org** / Wiktionnaire français | `dictionary.csv` (404 837 formes : lemmes + formes fléchies) | `https://raw.githubusercontent.com/hbenbel/French-Dictionary/master/dictionary/dictionary.csv` | **MIT** (dépôt) ; données dérivées de fr.wiktionary — **CC BY-SA 3.0** (attribution conservée ici) |
| **kaikki.org** / **wiktextract** (ttraill), édition **fr.wiktionary** (`lang_code = fr`, glosses français) | `kaikki-frwiktionary.jsonl.gz` (392 Mo, 2,11 M lignes → 362 819 entrées retenues) | `https://kaikki.org/frwiktionary/Fran%C3%A7ais/kaikki.org-dictionary-Fran%C3%A7ais.jsonl.gz` | données fr.wiktionary : **CC BY-SA 4.0** (attribution conservée ici) ; outil wiktextract : MIT |

Téléchargement total ≈ 420 Mo, dont **392 Mo pour le seul dump kaikki-fr
(dérogation à la cible < 300 Mo)** : c'est la plus petite source de
définitions françaises réellement utilisable trouvée (les dumps Wiktionnaire
bruts pèsent plusieurs Go, les alternative testées — WOLF, Littré, French-Dictionary,
HF « french_dict » — sont sans définitions ou < 80 000 entrées).
Téléchargement unique : `tools/cache/kaikki-frwiktionary.jsonl.gz` fait foi
pour les régénérations suivantes (aucun appel réseau ensuite).

## Méthode

1. Chaque verbe est rattaché à son template (`manger → man:ger`) ; le radical
   = verbe privé du suffixe après `«:»` du template, puis concaténation avec
   les formes du template (`man` + `ge` = `mange`). Vérifié sur les 7 011
   verbes : 0 radical inattendu, 0 template manquant.
2. Plusieurs `<i>` dans un `<p>` = graphies alternatives (`ece/èce`,
   `iérai/eyerai`) → autant de clés. `<p>` vide = forme inexistante
   (verbe impersonnel) → personne non émise.
3. Regroupement par `(inf, temps)` : les personnes sont fusionnées dans un
   même objet de lecture, dédupliquées, triées dans l'ordre canonique
   `1s 2s 3s 1p 2p 3p`. Clés JSON triées → sortie déterministe.
4. `mots.txt` = mots alphabétiques minuscules de `dictionary.csv`
   **∪** toutes les formes de `conjugaison.json` (cohérence entre les deux
   artefacts), triés, sans doublon.
5. `defs.txt` : dump kaikki-fr streamé ligne à ligne, `lang_code = fr`,
   1ʳᵉ gloss de la 1ʳᵉ entrée non écartée par mot (BTreeMap → trié).
   Écartés : `pos_title` « Forme … » (1,4 M de pages de conjugaison/fléchies,
   glosses type « Première personne du singulier… ») et noms propres
   (« Nom propre », « Nom de famille », « Prénom », « Patronyme » — des
   étiquettes, pas des définitions). Sans ces filtres : 1,93 M entrées
   = 141 Mo > 30 Mo.
6. Chaque ligne `mot|définition` est budgétée à 140 caractères au total :
   la définition reçoit `140 − len(mot) − 1` caractères, tronquée sur un
   espace + `…` (jamais au milieu d'un mot).

## Vérifications

À chaque exécution (échec ⇒ code de sortie 1) :

- `mange → manger/présent/1s+3s`, `suis → être/présent/1s`,
  `irai → aller/futur/1s`, `allé → aller/participe passé/-`,
  `fûmes → être/passé simple/1p`, `ont → avoir/présent/3p`
- `bonjour`, `grandes`, `écrire` présents dans `mots.txt`
- `defs.txt` : `bonjour`, `être`, `grand` présents et affichés ;
  1 « | » exact par ligne, aucun champ vide, ≥ 80 000 entrées, ≤ 30 Mo
  (`c'est` : absent du dump, non bloquant — les contractions n'y sont pas)

Contrôles externes déjà passés sur les artefacts produits :

```bash
node -e "JSON.parse(require('fs').readFileSync('data/conjugaison.json','utf8')); console.log('JSON OK')"
LC_ALL=C sort -c data/mots.txt          # tri C strict
grep -c '[A-Z0-9]' data/mots.txt        # 0 : ni majuscule ni chiffre
grep -c '^$' data/mots.txt              # 0 : aucune ligne vide
# defs.txt : ligne ≤ 140 caractères, clés minuscules, UTF-8, LF final
node -e "const t=require('fs').readFileSync('data/defs.txt','utf8');const l=t.trimEnd().split('\n');console.log(l.length, Math.max(...l.map(x=>[...x].length)))"
```

## Limites connues (volontaires)

- **Temps composés absents** (passé composé, plus-que-parfait, plus-que-parfait
  du subjonctif) : la clé du format est une forme unique or ces temps sont
  multi-mots (« ai mangé »). Ils se dérivent côté consommateur :
  auxiliaire conjugué (présent dans la table) + participe passé (présent).
  shortcut: la source ne les contient pas non plus, passer par un assemblage
  si un jour un temps composé mono-mot est nécessaire.
- **Gérondif absent** : la source n'a pas de balise gérondif (le participe
  présent couvre les formes en « -ant »).
- **Formes à apostrophe/tiret** (117 + 1 576, ex. `entr'aima`,
  `arc-boutait`) : conservées dans `conjugaison.json` (telles qu'écrites),
  exclues de `mots.txt` (spec « ni ponctuation »).
- **Couverture** : les 7 011 verbes usuels du modèle conjugate-fr ; les verbes
  rares hors liste sont absents. Le template `brumass:er` existe dans la
  source mais n'est utilisé par aucun verbe (sans effet).
- Apostrophes/hyphens globalement absents de `mots.txt` (`aujourd'hui`…),
  conformément à la spec de la phase 4 ; à réintégrer si la phase 5 les
  juge nécessaires.
- `defs.txt` ne contient **que des lemmes** : formes fléchies/conjugées
  (`mange`, `suis`…) et noms propres sont écartés pour tenir 30 Mo — la
  bulle sur une forme doit d'abord lemmatiser (via `conjugaison.json`).
  Les contractions (`c'est`) ne sont pas dans la source.
