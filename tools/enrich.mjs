// Enrichit tics.txt / adjectifs.txt : ajoute les alternatives manquantes
// (liste dans enrich-data.json), sans jamais toucher au texte existant.
// Idempotent : ré-exécutable tant que les alts sont uniques et non vides.
//
//   node tools/enrich.mjs [--dry]
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const data = JSON.parse(readFileSync(join(root, "tools", "enrich-data.json"), "utf8"));
const dry = process.argv.includes("--dry");
let erreur = false;

for (const [fichier, map] of Object.entries(data)) {
  const chemin = join(root, fichier);
  const brut = readFileSync(chemin, "utf8");
  const fin = brut.endsWith("\n");
  const lignes = brut.split("\n");
  if (fin) lignes.pop();
  let avant = 0;
  let apres = 0;
  let n = 0;
  const vus = new Set();

  for (let i = 0; i < lignes.length; i++) {
    const ligne = lignes[i];
    if (!ligne || ligne.startsWith("#")) continue;
    const champs = ligne.split("|");
    const declencheur = champs[0];
    avant += champs.length - 1;
    n++;
    vus.add(declencheur);
    for (const a of map[declencheur] ?? []) {
      // 8 alts maximum : au-delà, la qualité chute sans besoin réel.
      if (champs.length - 1 >= 8) break;
      if (!a || a.includes("|") || a.includes("#")) {
        console.error(`ALT INVALIDE (${fichier} / ${declencheur}) : ${JSON.stringify(a)}`);
        erreur = true;
        continue;
      }
      if (!champs.includes(a)) champs.push(a);
    }
    const locaux = new Set();
    for (const c of champs) {
      const cle = c.toLowerCase();
      if (locaux.has(cle)) {
        console.error(`DOUBLON (${fichier} / ${declencheur}) : ${c}`);
        erreur = true;
      }
      locaux.add(cle);
    }
    if (champs.length < 5) {
      console.error(`TROP COURT (${fichier} / ${declencheur}) : ${champs.length - 1} alts`);
      erreur = true;
    }
    lignes[i] = champs.join("|");
    apres += champs.length - 1;
  }

  for (const declencheur of Object.keys(map)) {
    if (!vus.has(declencheur)) {
      console.error(`DECLENCHEUR INCONNU (${fichier}) : ${JSON.stringify(declencheur)}`);
      erreur = true;
    }
  }

  const sortie = lignes.join("\n") + (fin ? "\n" : "");
  if (!dry && sortie !== brut) writeFileSync(chemin, sortie, "utf8");
  console.log(
    `${fichier} : ${n} lignes, alts moy. ${(avant / n).toFixed(2)} → ${(apres / n).toFixed(2)}` +
      (dry ? " (dry)" : sortie === brut ? " (inchangé)" : ""),
  );
}

process.exit(erreur ? 1 : 0);
