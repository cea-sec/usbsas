# Barbacane — SAS USB autonome hors ligne

> *Barbacane : ouvrage fortifié avancé qui protège l'entrée d'un château ; tout ce qui entre y est contrôlé avant d'atteindre la porte.*

Borne sans réseau qui lit une clé USB inconnue avec **usbsas** (pile USB en
espace utilisateur, CEA), analyse tous les fichiers y compris le contenu des
archives (ClamAV + YARA, extraction 7-Zip en bac à sable), copie fichier par
fichier les fichiers sains vers une clé de sortie (ou **blanchit** la clé
d'origine) et explique ce qui n'est pas passé. Mises à jour hors ligne signées.

## Organisation

Le dépôt est un fork d'usbsas : le code usbsas est à la racine (écarts listés
dans [`docs/ecarts-upstream.md`](../docs/ecarts-upstream.md)), Barbacane est ici.

| Dossier | Contenu |
|---|---|
| `analyzer/` | `barbacane-analyzer` : protocole analyseur usbsas, extraction récursive sandboxée, limites anti-bombe, ClamAV + YARA, rapports, dossier de préparation ; modes serveur (TLS + jetons) et relais |
| `parcours/` | bibliothèque des parcours (analyse, préparation, écriture, effacement) au-dessus d'usbsas |
| `client/` | interface graphique (iced) : Copier / Blanchir, clavier virtuel, résumé, mode admin ; produit le paquet `barbacane` |
| `admind/` | `barbacane-admind` (root) : PIN Argon2id, enrôlement, limites, export, mises à jour signées |
| `update/` | outils du poste administrateur : `barbacane-update-prepare`, `barbacane-update-keygen` |
| `system/` | `install.sh`, `build-debs.sh`, configurations, unités systemd, durcissement, image `mkosi/` |
| `tests/` | échantillons (`fixtures/`), scénarios de mise à jour (`maj/`), environnement d'interface (`ui/`) |

Documentation : [conception](../docs/conception.md), [installation](../docs/installation.md),
[utilisation](../docs/exploitation.md), [administration](../docs/administration.md),
[tests](../docs/tests.md).

## Développement

Prérequis (Debian/Ubuntu) : dépendances de compilation d'usbsas (`doc/build.md`),
`clamav-daemon 7zip bubblewrap zip genisoimage gcab msitools qemu-utils dosfstools mtools exfatprogs ntfs-3g fdisk minisign zstd`.

```shell
barbacane/tests/phase1-mock.sh        # usbsas en mode mock (clés simulées) + ses tests d'intégration
cd barbacane && cargo test            # analyseur, archives, mode serveur, admind, parcours de bout en bout
tests/maj/maj.sh                      # préparation → application des mises à jour (après cargo build)
```

Interface sans matériel : `cargo build --features x11 -p barbacane-client`, puis
`tests/ui/env.sh <dossier> <image clé entrée> <image clé sortie>` (Xvfb `:42`,
usbsas mock, analyseur, admind de test ; PIN `246813`).
