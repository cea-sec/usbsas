# Barbacane — Document de conception (phase 0)

Statut : **validé** (voir §8 pour les décisions).

Base étudiée : usbsas `usbsas-usbsas` 0.3.4, commit `69ebcf4` (dépôt courant).

## 1. Ce que fait usbsas aujourd'hui (constats dans le code)

### 1.1 Enchaînement d'un transfert
`usbsas-usbsas/src/states.rs` : machine à états
`Init → OpenSrcUsb → BrowseSrc → FileSelection → Analyze → WriteDstFile → TransferDst → End`,
plus deux états autonomes `Wipe` et `ImgDisk`. Tous les processus fils sont
relancés entre deux opérations.

- Les fichiers sélectionnés sont copiés dans un **tar de session** (`files2tar`).
- `WriteDstFile` relit **ce tar** (`tar2files`) et écrit chaque fichier propre
  dans un système de fichiers construit hors périphérique (`files2fs`),
  puis `fs2dev` l'écrit sur la clé de sortie (secteurs non nuls seulement).
- La destination USB est **toujours reformatée** (nouvelle table de partition + FS).

### 1.2 Protocole analyseur (vérifié dans `usbsas-net/src/analyzer.rs`, `jsonparser.rs`, `usbsas-analyzer-server`)
1. `POST {url}/{user_id}` — corps = tar de session (envoi en flux, taille connue).
   Réponse JSON : `{"id": "<analyze_id>", "status": "uploaded"}`.
2. Interrogation `GET {url}/{user_id}/{analyze_id}` **toutes les secondes, sans
   délai maximal** (`// XXX TODO timeout`) :
   - `status` = `"uploaded"` ou `"processing"` → on continue d'attendre ;
   - `"scanned"` → rapport final ;
   - toute autre valeur ou code HTTP ≠ 2xx → erreur, transfert avorté.
3. Rapport final (doit être désérialisable en `AnalyzeReport`, `version: 2` sinon `panic!`) :
   ```json
   {"id":"…","status":"scanned","version":2,
    "files":{"chemin/relatif":{"status":"CLEAN"|"DIRTY", …}},
    "antivirus":{"ClamAV":{"version":"…","database_version":"…","database_timestamp":"…"}}}
   ```
   Clés de `files` = chemins **sans le `/` initial**. Un fichier absent du rapport
   ou d'un autre statut reste « non analysé » et **n'est pas copié**. Les clés
   inconnues de usbsas sont ignorées. Les champs supplémentaires par fichier
   (sha256, signature…) sont tolérés si le schéma proto le permet — **à confirmer
   en phase 1** sur `AnalyzeReport` (sinon on les met uniquement dans notre
   rapport détaillé, cf. §3).
4. Le processus `analyzer` est sandboxé (seccomp + landlock), n'ouvre que le
   port de l'URL configurée → l'analyseur peut être local (`127.0.0.1:8042`)
   ou distant sans changement côté usbsas. Kerberos optionnel côté client.

### 1.3 Autres briques utiles
- **exFAT** : lecture (`ExFatFsReader`) et écriture (`ExFatFsWriter`, `FsType::EXFAT = 2`) **déjà supportés**. → §4.4 réglé sans rien faire.
- **Wipe** : `RequestWipe{id, fstype, quick}` → remplissage de zéros (sauf `quick`) puis nouveau FS.
- **Source `LocalDir`** (`[[src_local_dirs]]`, process `local2files`, landlock RO sur les dossiers) : usbsas sait lire un **dossier local** comme source et l'écrire sur une clé. Les sources non-USB **ne sont pas analysées** (`init_transfer`).
- **Destination `Command`** : exécute une commande avec le tar (`%SOURCE_FILE%`) ; analyse activable (`analyze_cmd`). Process `cmdexec` **non sandboxé** (avertissement upstream).
- `usb_port_accesses` (src/dst/hid), filtres par nom, rapport JSON local + sur destination, mode `mock`.
- Le client (`iced`) interroge l'état chaque seconde ; aucune limite de durée côté client relevée.

## 2. Architecture retenue

Principe directeur : **ne pas toucher au cœur**, et composer les briques
existantes en **deux transferts usbsas successifs** séparés par une zone de
préparation locale (« sas intérieur »).

```
Transfert A :  clé inconnue ──usbsas──► tar ──► barbacane-analyzer (verdicts, extraction)
                                         └─► destination Command « barbacane-stage »
                                               → dossier de préparation /var/lib/barbacane/stage/<id>/
                                                 = fichiers sains d'origine
                                                 + fichiers sains extraits des archives
                                                 + rapport (txt/html/json)
Transfert B :  LocalDir(stage/<id>) ──usbsas──► clé de sortie (exFAT)     [Copier]
               ou   Wipe(clé inconnue) puis LocalDir ──► même clé         [Blanchir]
```

Composants :
| Composant | Rôle | Privilèges |
|---|---|---|
| usbsas (inchangé) | lecture USB userland, écriture FS, wipe | utilisateur `usbsas` |
| `barbacane-analyzer` | API compatible §1.2 + API locale Barbacane, orchestration de l'analyse | utilisateur `barbacane-analyzer` |
| `barbacane-extract` (enfant) | 1 exécution `7zz` par conteneur, dans bubblewrap (tmpfs taille fixe, `--unshare-all`, seccomp) + cgroup `MemoryMax`/`CPUQuota` via `systemd-run --scope` ou unité dédiée | aucun |
| clamd | `INSTREAM` sur socket Unix locale | `clamav` |
| YARA | crate `yara-x` dans l'analyseur (règles compilées au chargement) | — |
| `barbacane-stage` | commande appelée par usbsas en fin de transfert A : récupère depuis l'analyseur la liste finale et peuple le dossier de préparation | `barbacane-stage` (via sudo restreint ou socket vers l'analyseur, cf. risques) |
| client (fork limité) | parcours Copier/Blanchir, clavier virtuel, mots de passe, résumé, mode admin | utilisateur kiosque |

API analyseur :
- Compatible usbsas : `POST /api/scanbundle/{uid}`, `GET /api/scanbundle/{uid}/{id}`.
- Locale Barbacane (même service, préfixe `/barbacane/v1`, réservée au client/stage) :
  `GET  …/scans/{id}` (progression détaillée, questions en attente),
  `POST …/scans/{id}/passwords` (`{archive, password | abandon}`),
  `GET  …/scans/{id}/report` (rapport détaillé),
  `GET  …/scans/{id}/files.tar` (arbre final des fichiers sains, pour `barbacane-stage`).
- Mode serveur (phase 8) : même binaire, `listen` + TLS (rustls) + mTLS ou jeton,
  `uid` = identifiant de la borne. Désactivé par défaut.

## 3. Réponses aux points du §4

### 4.1 Mots de passe d'archives — **aucune modification du cœur**
Constat : usbsas interroge sans délai maximal tant que `status ∈ {uploaded, processing}`.

Mécanisme :
1. L'analyseur rencontre une archive chiffrée → il continue le reste de l'analyse,
   met la question en file : `pending: [{archive: "docs/a.7z", tentative: 1}]`,
   et **répond `processing`** à usbsas.
2. Le client, qui affiche l'état « Analyse », interroge en parallèle
   `GET /barbacane/v1/scans/{id}` (il connaît l'`id` : voir ci-dessous) et
   ouvre le clavier virtuel.
3. `POST …/passwords` → réessai (3 tentatives max). Mauvais mot de passe après 3
   essais, « Ignorer », ou **délai d'inactivité** (défaut 5 min, configurable,
   géré par l'analyseur puisque usbsas n'en a pas) → archive `DIRTY` avec
   raison « mot de passe absent/incorrect ».
4. Quand plus rien n'est en attente → `scanned`.

Retrouver l'`id` côté client : le client ne voit pas l'`analyze_id` usbsas. On
utilise l'`user_id` (connu du client via `UserId`) : `GET /barbacane/v1/users/{uid}/current`.
Une seule analyse en cours par borne → pas d'ambiguïté. Le mot de passe ne
transite que par `127.0.0.1` (ou TLS en mode serveur), n'est jamais journalisé,
est effacé de la mémoire (`zeroize`) après usage.

### 4.2 Copie des fichiers sains extraits — **option C recommandée (aucune modification du cœur)**

| Option | Principe | + | − | Effort |
|---|---|---|---|---|
| A. Patch cœur « fichiers de remplacement » | le rapport fournit des fichiers supplémentaires ; usbsas les télécharge (process `downloader` existant) et les écrit | un seul transfert, intégré | modifie `states.rs`, proto, sandbox ; à maintenir hors upstream tant que la PR n'est pas acceptée | moyen/élevé |
| B. Archive assainie | l'analyseur reconstruit `x.zip` sans le fichier sale | rien à extraire côté utilisateur | impossible à faire passer : usbsas copie le fichier d'**origine** depuis son tar ; demande aussi un patch ; ré-encodage de formats exotiques (iso, vmdk, msi) irréaliste | élevé |
| **C. Deux transferts via dossier de préparation** | transfert A (USB → `Command`, analysé) ; `barbacane-stage` construit l'arbre final ; transfert B (`LocalDir` → USB) | **zéro patch** ; réutilise local2files (landlock) et files2fs ; même chemin pour Copier et Blanchir ; rapport libre (txt/html) écrit comme un fichier normal | deux passes d'écriture/lecture (temps) ; la sortie n'est pas analysée par usbsas (c'est voulu : elle l'a déjà été) ; il faut sécuriser le dossier de préparation | faible/moyen |

Présentation sur la clé de sortie : une archive sale `a.zip` contenant des
fichiers sains → dossier `a.zip.extrait/…` (nom à valider). L'archive d'origine
n'est pas copiée.

Point d'attention option C : `[[src_local_dirs]]` est **statique** dans la
config. On déclare un seul dossier fixe `/var/lib/barbacane/sortie` vidé et
rempli à chaque transfert (une opération à la fois sur une borne). Le client
le présente comme « source » invisible à l'utilisateur.

### 4.3 Blanchir (même périphérique)
Séquence, chaque étape étant une opération usbsas (fils relancés entre chaque) :
1. Transfert A : clé inconnue → `Command` (analyse) → dossier de préparation.
   La clé n'est **pas modifiée** à ce stade.
2. Écran de confirmation « La clé va être effacée. N fichiers seront réécrits. »
3. `Wipe{id, fstype: EXFAT, quick: false}`.
4. Transfert B : `LocalDir` → **même** clé (même n° de série/port).
5. Rapport sur la clé + local.

Garanties contre l'état ambigu : le dossier de préparation est conservé tant que
l'étape 4 n'est pas confirmée ; si 3 ou 4 échoue, message « Clé effacée mais
réécriture incomplète : ne pas l'utiliser, rebrancher pour réessayer » et
possibilité de **rejouer l'étape 4** sans redemander l'analyse. Contrôle avant
l'étape 3 : la clé rebranchée/présente doit avoir le même VID/PID/série.
Remarque : le transfert B reformate de toute façon ; le `Wipe` sert à
l'effacement complet (zéros) des données d'origine, y compris l'espace libre.
Le port d'entrée doit aussi être autorisé en sortie pour cette opération
(`ports_dst` ⊇ port d'entrée) : la clé reste sur son port.

### 4.4 exFAT
Déjà géré par `files2fs` (`FsType::EXFAT`). Rien à faire. Taille max de
fichier : la limite 4 Go ne s'applique qu'à FAT (`max_file_size` dans `states.rs`).

## 4. Analyse dans `barbacane-analyzer`

- Pour chaque fichier du tar : sha256, type réel (crate `infer` + `file`/libmagic optionnel), ClamAV, YARA.
- Conteneur reconnu (signature magique, pas l'extension) → extraction sandboxée avec `7zz x` (formats : tout ce que 7-Zip ouvre), puis récursion.
- Chiffré : `7zz l -slt` (`Encrypted = +`) ou code retour « Wrong password » → §4.1.
- Limites (par transfert) : profondeur 5, 10 000 fichiers, 4 Go décompressés, ratio max (proposé 100:1 par conteneur), durée max par conteneur ; dépassement → conteneur `DIRTY` avec raison explicite, fichiers déjà validés conservés ? → conteneur entier refusé.
- Noms : normalisation, rejet de `..`, chemins absolus, liens symboliques/matériels, caractères de contrôle ; longueur max ; collisions de casse.
- Verdict d'un **conteneur** pour usbsas : `CLEAN` seulement si lui et tout son contenu sont sains ; sinon `DIRTY` et ses enfants sains vont en sortie via §4.2.
- Analyseur indisponible / clamd hors service → **tout refusé** (fail-closed).

## 5. Écarts upstream prévus
Aucun pour les phases 1 à 7 si l'option C est validée. Contributions possibles
au CEA, indépendantes : délai max de l'interrogation analyseur (`XXX TODO timeout`),
remplacement du `panic!` sur `version ≠ 2` par une erreur, `src_local_dirs`
dynamique. Le client sera un fork (autorisé par la consigne).

## 6. Risques
| Risque | Mesure |
|---|---|
| `cmdexec` non sandboxé (destination `Command`) | la commande est un binaire minimal qui ne lit pas le tar : il demande l'arbre à l'analyseur par socket ; ou alors on lit le tar dans un enfant bwrap. Unité systemd durcie. |
| Dossier de préparation = données non fiables sur disque local | tmpfs dédié (taille = limite 4 Go + marge ; nécessite assez de RAM, sinon partition chiffrée éphémère), `noexec,nosuid,nodev`, propriétaire dédié, purgé après chaque opération et au démarrage |
| Exploit 7-Zip / clamd sur fichier piégé | bubblewrap sans réseau, tmpfs, seccomp, cgroups ; clamd en utilisateur dédié ; mises à jour régulières |
| Attente infinie d'usbsas si l'analyseur plante | délai côté analyseur + supervision systemd ; statut `error` renvoyé → usbsas avorte proprement |
| Champs ajoutés au rapport refusés par `AnalyzeReport` | rapport détaillé servi par l'API Barbacane, rapport usbsas minimal |
| Différence de comportement usbsas entre versions | version épinglée, tests e2e mock en CI |
| Clavier virtuel : observation par épaule | masquage des caractères, pas de journalisation |
| RAM des HP/Dell cibles | 16 Go : tmpfs 6 Go + 6 Go |

## 6 bis. Écarts de réalisation (phase 3)

- **Pas de commande `barbacane-stage`** : c'est le client qui demande à l'analyseur
  (`POST /barbacane/v1/scans/{id}/stage`) de remplir le dossier de préparation. La
  destination « commande » du transfert A est `/bin/true` (aucune donnée lue).
  Raison : si aucun fichier de la clé n'est sain (ex. une seule archive mixte), usbsas
  arrête le transfert A (« Nothing to copy after analyzer ») avant la commande ; le
  contenu sain extrait doit pourtant passer.
- **Tar usbsas** : les fichiers sont sous `data/` (plus `config.json`), comme le traite
  `usbsas-analyzer-server` ; l'analyseur fait de même.
- La logique des parcours (analyse, préparation, écriture, effacement) est dans la
  bibliothèque `barbacane/parcours`, testée de bout en bout en mode mock, et utilisée
  par le client graphique.

## 7. Décisions (validées)
1. §4.2/4.3 : **option C**, deux transferts via dossier de préparation, aucun patch du cœur.
2. Dépôt : on garde ce fork. Le code usbsas reste **inchangé** à la racine (épinglé sur le commit upstream `69ebcf4`) ; tout Barbacane vit sous `barbacane/` (`analyzer/`, `client/`, `update/`, `system/`, `tests/`) avec son propre workspace Cargo. `docs/` à la racine. Toute divergence du code usbsas est listée dans `docs/ecarts-upstream.md`.
3. Blanchir : la clé reste sur le port d'entrée, qui est donc aussi déclaré dans `ports_dst`.
4. Limite dépassée dans une archive : **tout le conteneur est refusé**.
5. Bornes : 16 Go de RAM → tmpfs de préparation 6 Go, tmpfs d'extraction 6 Go.
6. **Zed abandonné** : aucune détection spécifique ; un conteneur Zed est traité comme un fichier inconnu (analysé, non extrait).
7. Fichiers extraits d'une archive refusée : dossier `<archive>.extrait/` sur la clé de sortie.

## 8. Réalisation (état final des phases 1 à 8)

| Composant | Réalisation |
|---|---|
| Analyse | `barbacane-analyzer` : protocole usbsas v2 (tar sous `data/`), 7-Zip dans bubblewrap (sans réseau, `--unshare-all`, limites mémoire / taille de fichier / CPU / durée), ClamAV par `INSTREAM`, YARA-X (un espace de noms par fichier de règles), type réel (contenu), sha256, rapports JSON / texte / HTML |
| Archives | détection par octets magiques (zip, 7z, rar, gzip, bzip2, xz, zstd, iso/udf, cab, OLE/msi, vmdk, vhd(x), qcow, vdi, wim, cpio, arj, lzh, chm, xar, rpm, ar, squashfs, tar, gpt, ntfs, fat, ext…) ; chemins dangereux (`..`, absolus) → archive non copiée telle quelle, contenu sain extrait |
| Mots de passe | l'analyseur reste « processing » pour usbsas et publie la question sur `/barbacane/v1/…` ; 3 essais, abandon, délai (§4.1) |
| Copie fichier par fichier | deux transferts usbsas + dossier de préparation rempli par l'analyseur (§4.2, option C) ; `<archive>.extrait/` ; collisions de casse exFAT renommées |
| Blanchir | analyse → confirmation → wipe usbsas (zéros) → réécriture exFAT ; réessai de l'écriture sans nouvelle analyse (§4.3) |
| Interface | `barbacane-client` (iced, fork fonctionnel du client usbsas : même pile, écrans réécrits) : souris seule, clavier virtuel AZERTY, résumé « passé / refusé / pourquoi », indicateur d'âge des signatures |
| Administration | `barbacane-admind` (root, socket Unix, groupe kiosque) : PIN Argon2id + verrouillage, jeton de session, enrôlement, limites, export (rapports, journal, état dpkg), mises à jour, arrêt |
| Mises à jour | `.bcu` (tar.zst) + minisign, manifeste sha256, anti-retour arrière, restauration ClamAV/YARA, apt sans source réseau, rotation de clé ; lecture de la clé admin par usbsas (dépôt des `.bcu` par l'analyseur après armement) |
| Système | paquet `barbacane`, unités durcies, kiosque `cage`, souris via `usbsas-hid`, modprobe / sysctl / nftables `drop`, paramètres noyau, `install.sh`, image `mkosi` |
| Serveur | mode serveur (TLS, jetons, isolation par borne) + mode relais sur la borne (usbsas inchangé, certificat épinglé) |

Écart au cœur d'usbsas : un seul, le correctif seccomp sur 32 bits (`docs/ecarts-upstream.md`).
