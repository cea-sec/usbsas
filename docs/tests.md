# Tests

Tous les tests automatisés tournent en CI (`.github/workflows/barbacane.yml`) :
shellcheck, phase 1 (usbsas mock), `cargo fmt`, `clippy -D warnings`, `cargo test`,
scénarios de mise à jour, `cargo audit`.

## Correspondance avec le cahier des charges (§5)

| Exigence | Test | Où |
|---|---|---|
| Environnement de développement, mode mock d'usbsas | clés simulées par fichiers image (MBR + FAT/NTFS/exFAT) | `parcours/tests/e2e.rs`, `tests/phase1-mock.sh` |
| EICAR seul / dans zip / zip dans 7z / dans iso | `eicar.com`, `mixte.zip`, `zip_dans_7z.7z`, `eicar.iso` (+ tar.gz, cab, vmdk, msi) | `analyzer/tests/archives.rs` |
| Archives chiffrées : bon / mauvais / sans mot de passe / abandon | `chiffre-bon.zip`, `chiffre-faux.zip` (3 essais), `chiffre-sans.zip` (délai), `chiffre-abandon.zip`, `chiffre-entetes.7z` (en-têtes chiffrés) | `analyzer/tests/archives.rs` ; saisie à la souris : captures `02`, `03` |
| Zip bombs récursive et à fort ratio | `bombe-recursive.zip` (profondeur 7), `bombe-ratio.zip` (~1000:1) | `analyzer/tests/archives.rs` |
| Dépassement de chaque limite | profondeur, nombre de fichiers (`trop-de-fichiers.zip`), taille (`trop-gros.zip`), ratio ; conteneur racine refusé en entier | `analyzer/tests/archives.rs` |
| Noms piégés (traversée, absolu, unicode, très longs, casse) | `noms-pieges.zip` ; aucune sortie ne remonte, collisions de casse renommées | `analyzer/tests/archives.rs`, `analyzer/src/stage.rs` |
| Fichiers vides | `vide.txt` | `analyzer/tests/archives.rs` |
| Conteneur Zed | abandonné (décision 6) : traité comme un fichier quelconque | — |
| vmdk / msi / cab | `disque.vmdk` (FAT dans VMDK), `paquet.msi`, `mixte.cab` | `analyzer/tests/archives.rs` |
| Systèmes de fichiers source FAT / exFAT / NTFS | parcours complet sur chaque type | `parcours/tests/e2e.rs` |
| Copie vers clé connue | sortie relue (exFAT) : fichiers sains, `mixte.zip.extrait/`, rapport | `parcours/tests/e2e.rs` ; captures `01`→`05` |
| Archive mixte seule (rien de sain à la racine) | contenu sain extrait quand même copié | `parcours/tests/e2e.rs` |
| Blanchiment | même périphérique effacé puis réécrit, EICAR absent du périphérique brut | `parcours/tests/e2e.rs` ; capture `06` |
| Mode clés enrôlées | enrôlement, doublons, clé sans série, activation | `admind/src/tests.rs` ; capture `08` |
| MAJ signée valide / signature invalide / version plus ancienne / paquet corrompu | + signature absente, manifeste incohérent, fichier non listé, rotation de clé, retour arrière | `admind/src/tests.rs`, `tests/maj/maj.sh` ; capture `10` |
| PIN : hachage, verrouillage | Argon2id, 5 échecs → verrouillage croissant | `admind/src/tests.rs` |
| Analyseur indisponible | ClamAV injoignable → statut `error`, usbsas avorte | `analyzer/tests/archives.rs` |
| Mode serveur (phase 8) | analyseur central TLS + jetons, deux bornes relais, isolation, jeton inconnu refusé | `analyzer/tests/serveur.rs` |
| Lecture de la clé admin | paquets `.bcu` déposés seulement après armement, jamais copiés en sortie | `analyzer/tests/archives.rs` |

## Tests manuels d'interface (souris uniquement)

Réalisés sous Xvfb avec `xdotool` (clics aux coordonnées, aucun clavier) et
usbsas mock : Copier (avec mot de passe saisi au clavier virtuel), Blanchir
(avec « Ignorer cette archive »), connexion admin au pavé numérique, enrôlement,
export des journaux, mise à jour signée. Captures dans [`captures/`](captures/).

## Paquets

`barbacane/system/build-debs.sh` construit `usbsas-server`, `usbsas-hid` et
`barbacane` (production, sans mock). Installés sur Ubuntu 24.04 (faute de
Debian 13) : comptes et groupes créés, réglages clamd appliqués, unités,
PAM, modprobe, sysctl en place. Les postinst d'usbsas échouent dans un conteneur
sans `udevadm` (normal hors système complet).

## Non couvert ici

- Installation réelle sur Debian 13 et matériel HP/Dell (miroirs Debian
  inaccessibles depuis l'environnement de développement) : `install.sh`, le
  paquet `barbacane` et l'image `mkosi` sont à valider sur une première borne.
- Vraies clés USB (seul le mode mock d'usbsas a été utilisé) et vraie base ClamAV
  (base de test limitée à EICAR).
