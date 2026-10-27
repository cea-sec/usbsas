# Installation d'une borne Barbacane

Ce document permet d'installer une borne **de zéro**, soit par le script
d'installation sur une Debian 13 fraîche, soit en flashant une image disque.

## 1. Matériel et prérequis

| Élément | Exigence |
|---|---|
| Poste | HP ProDesk / Dell OptiPlex x86_64, UEFI, **16 Go de RAM** minimum |
| Disque | 64 Go minimum (journaux, rapports, fichiers temporaires usbsas) |
| Périphériques | écran + **souris USB** uniquement ; aucun clavier en production |
| Ports USB | 1 port « entrée » (clé inconnue, aussi utilisé pour Blanchir), 1 port « sortie », 1 port souris |
| Réseau | **jamais d'Internet, même à l'installation** : l'ISO est préparée sur un poste connecté puis copiée sur une clé USB |

Pour la préparation, prévoir aussi un **poste administrateur connecté** (Debian 13
de préférence) qui génère les clés de signature et les paquets de mise à jour.

## 2. Réglages BIOS / UEFI (à faire avant tout)

1. Mettre à jour le BIOS du constructeur (HP : *HP PC Hardware Diagnostics* / Dell : *Dell Command | Update*) depuis une clé USB (fichier téléchargé sur le poste administrateur), sans jamais brancher la borne au réseau.
2. **Mot de passe administrateur BIOS** (HP : *Security → Create BIOS Administrator Password* ; Dell : *Security → Admin Password*). Le conserver avec les secrets de l'administration.
3. **Ordre de démarrage** : disque interne uniquement ; désactiver le démarrage USB, réseau (PXE) et lecteur optique ; désactiver le menu de démarrage F9/F12 (HP : *Boot Options → USB Storage Boot* désactivé ; Dell : *Boot Configuration*, décocher *Enable USB Boot Support*, *Enable PXE*).
4. **Secure Boot** activé (mode *Deployed* / sans clés de tiers personnalisées). Debian 13 démarre en Secure Boot avec `shim` signé (installation par script). L'image mkosi utilise systemd-boot non signé : soit désactiver Secure Boot, soit signer l'image (voir §6.3).
5. Désactiver : Wake-on-LAN, *Intel AMT / vPro* (HP *Advanced → Remote Management* ; Dell *Intel AMT Capability : Disabled*), carte Wi-Fi et Bluetooth si l'option existe, ports Thunderbolt / *Thunderbolt Security Level : No Security* → désactivé.
6. Activer la virtualisation d'E/S (VT-d / IOMMU) : protège contre les attaques DMA (le noyau est lancé avec `iommu=force`).
7. Désactiver les ports USB non utilisés (si le BIOS le permet) et la **sortie** de veille par USB.
8. Activer la détection d'ouverture du boîtier (*Chassis Intrusion*) si disponible.

## 3. Méthode recommandée : ISO d'installation hors ligne

Guide pas à pas pour l'opérateur : [guide-installation-poste.md](guide-installation-poste.md).

Une seule ISO installe n'importe quel poste x86_64 (UEFI ou BIOS), **sans réseau
ni question**.

**Construire l'ISO** (une fois, sur le poste administrateur connecté, en root, Debian 13
ou conteneur `debian:trixie`) :

```sh
cp maj.pub barbacane/system/iso/maj.pub        # clé publique des mises à jour (recommandé)
barbacane/system/iso/build-iso.sh --sortie barbacane.iso
```

La base ClamAV du jour est téléchargée et embarquée (`--clamav DOSSIER` pour en
fournir une). La CI publie aussi une ISO de test (artefact `barbacane-iso`, sans clé de mise à jour).

**Installer une borne** :
1. Copier l'ISO sur une clé USB (`dd if=barbacane.iso of=/dev/sdX bs=4M`, ou Rufus / Etcher en mode « image DD »).
2. Démarrer le poste sur la clé (F9 HP / F12 Dell ; démarrage USB autorisé le temps de l'installation).
3. Le menu lance l'installation au bout de 10 s : **le plus grand disque interne est effacé**
   (jamais un disque USB). Environ 10 minutes, puis redémarrage automatique.
4. Retirer la clé. La borne démarre sur l'interface Barbacane.
5. Cliquer sur l'accès administrateur et **choisir le PIN** (6 chiffres minimum) : le premier PIN saisi devient le PIN.
6. Appliquer les réglages BIOS du §2 (désactiver le démarrage USB).

Les ports USB n'ont pas à être configurés : par défaut tout port peut servir
d'entrée ou de sortie. Les restreindre reste possible (§3.3).

## 3 bis. Installation par script (maquette ou développement uniquement)

> **Interdit pour une borne en service** : cette méthode exige le réseau. Une borne
> n'est **jamais** connectée, même pour l'installation : l'installer exclusivement avec l'ISO (§3).

### 3.1 Debian minimale

Installer Debian 13 (trixie) depuis l'image `netinst` :
- partitionnement : disque entier, chiffrement non requis (aucune donnée sensible persistante : les fichiers analysés sont en mémoire) ;
- **aucun environnement de bureau**, cocher seulement « utilitaires usuels du système » ;
- ne pas installer de serveur SSH ;
- compte `root` avec mot de passe fort (il ne servira plus une fois la borne en service).

### 3.2 Récupérer le dépôt et lancer l'installation

Préparer sur une clé (depuis le poste administrateur) :
- la clé publique de mise à jour `barbacane-maj.pub` (voir [administration.md](administration.md#cles)) ;
- facultatif : une base ClamAV (`main.cvd`, `daily.cvd`, `bytecode.cvd`) et des règles YARA ;
- facultatif : les paquets déjà construits (`barbacane/system/build-debs.sh` sur le poste administrateur).

Sur la machine de maquette (connectée), avec un clavier :

```shell
apt install git
git clone <dépôt> barbacane && cd barbacane
sudo barbacane/system/install.sh --pin --cle-publique /media/cle/barbacane-maj.pub \
     [--clamav /media/cle/clamav] [--yara /media/cle/yara] [--debs /media/cle/paquets] \
     --couper-reseau
```

Le script (relançable sans risque) :
1. active `contrib non-free` (paquet `7zip-rar` pour lire les archives RAR ; `--sans-rar` pour s'en passer) ;
2. installe les dépendances (ClamAV, 7-Zip, bubblewrap, cage, nftables, minisign…) ;
3. construit les paquets `usbsas-server`, `usbsas-hid`, `barbacane` si `--debs` n'est pas fourni (Rust installé si besoin) ;
4. installe les paquets : le paquet `barbacane` crée les comptes, applique les réglages système et active les services ;
5. pose `/etc/usbsas/config.toml` (première installation seulement), `/etc/nftables.conf`, la clé publique, les règles YARA ;
6. installe la base ClamAV initiale (dossier fourni ou téléchargement unique) ;
7. demande le **PIN administrateur** (6 chiffres minimum) ;
8. avec `--couper-reseau` : désactive et masque tous les services réseau.

### 3.3 Affecter les ports USB (facultatif)

Sans section `[usb_port_accesses]`, tout port est accepté. Pour restreindre chaque rôle à un port physique, avant de débrancher le clavier :

```shell
sudo systemctl stop barbacane-kiosk
watch -n1 lsusb -t
```

Brancher une clé successivement sur chaque port physique et noter le chemin
affiché : numéro de bus puis numéros de port (ex. `Bus 001` → `Port 002` →
chemin `[1, 2]` ; derrière un concentrateur interne `[1, 2, 5]`). L'outil
`usbsas-devmonitor` (crate `usbsas-tools`) affiche directement ces chemins. **Un port USB 3 apparaît sur deux bus** (USB 2 et USB 3) :
déclarer les deux chemins. Renseigner `/etc/usbsas/config.toml` :

```toml
[usb_port_accesses]
ports_src = [ [1, 2], [2, 2] ]            # port d'entrée (bus USB 2 et USB 3)
ports_dst = [ [1, 2], [2, 2], [1, 3], [2, 3] ]   # entrée (Blanchir) + sortie
ports_hid = [ [1, 4] ]                     # souris
```

Étiqueter physiquement les ports (« ENTRÉE », « SORTIE », souris) et obturer les autres.

### 3.4 Redémarrer

```shell
sudo reboot
```

La borne démarre directement sur l'interface Barbacane (console virtuelle 2,
session `cage`). Le clavier est désactivé (pilote HID noyau bloqué, seule la
souris est gérée par usbsas-hid en espace utilisateur).

## 4. Vérifications après installation

À faire une fois, avec une clé de test contenant le fichier EICAR, une archive
zip mixte (EICAR + fichier sain) et une archive protégée par mot de passe :

| Vérification | Résultat attendu |
|---|---|
| Démarrage | interface plein écran, aucun bureau, impossible d'en sortir |
| Indicateur en haut à droite | « Signatures antivirales : N jour(s) » (pas d'alerte orange) |
| Copier vers une clé | EICAR refusé, fichier sain de l'archive copié dans `<archive>.extrait/`, rapport texte + HTML sur la clé |
| Archive protégée | demande du mot de passe au clavier virtuel ; « Ignorer » → archive refusée |
| Blanchir | clé effacée puis réécrite avec les seuls fichiers sains |
| Mode administrateur | bouton « Administration » en bas à droite, PIN, menu complet |
| Réseau | `nft list ruleset` : politique `drop` ; `ip link` : aucune interface active hors `lo` |

## 5. Comptes et services installés

| Service | Compte | Rôle |
|---|---|---|
| `usbsas-server` | `usbsas` | lecture/écriture des clés en espace utilisateur (seccomp + landlock par processus) |
| `hid-dealer` | `usbsas-hid` | souris en espace utilisateur |
| `barbacane-analyzer` | `barbacane-analyzer` | analyse (ClamAV via clamd, YARA, archives dans bubblewrap) |
| `clamav-daemon` | `clamav` | moteur ClamAV |
| `barbacane-admind` | `root` | mode administrateur (PIN, mises à jour, export, arrêt) |
| `barbacane-kiosk` | `barbacane-kiosk` | interface graphique dans `cage` |

Fichiers de configuration : `/etc/usbsas/config.toml`, `/etc/barbacane/{analyzer,client,admind}.toml`,
clé publique `/etc/barbacane/maj.pub`. Données : `/var/lib/barbacane/` (rapports, état admin, règles YARA).

## 6. Image disque prête à flasher (mkosi)

### 6.1 Construction (poste administrateur, Debian 13, mkosi ≥ 25)

```shell
barbacane/system/build-debs.sh                       # paquets → barbacane/system/mkosi/paquets/
cp barbacane-maj.pub barbacane/system/mkosi/mkosi.extra/etc/barbacane/maj.pub
# facultatif : base ClamAV initiale
mkdir -p barbacane/system/mkosi/mkosi.extra/var/lib/clamav && cp clamav/*.cvd "$_"/
cd barbacane/system/mkosi && BARBACANE_PIN=xxxxxx mkosi -f build
```

### 6.2 Flashage

Démarrer la borne sur un support de maintenance (autorisé temporairement dans le
BIOS), puis `dd if=barbacane.raw of=/dev/nvme0n1 bs=4M conv=fsync` ; ou retirer
le disque et le flasher sur le poste administrateur. Rétablir ensuite les
réglages BIOS du §2 et affecter les ports (§3.3, en démarrant temporairement
avec un clavier : le blocage du clavier est appliqué au premier démarrage par usbsas-hid).

### 6.3 Secure Boot avec l'image mkosi

L'image embarque un noyau unifié (UKI) démarré par systemd-boot. Pour garder
Secure Boot : générer une clé de signature (`mkosi genkey`), ajouter
`SecureBoot=yes` et `SecureBootKey=`/`SecureBootCertificate=` dans `mkosi.conf`,
et enregistrer le certificat dans le firmware (mode *Setup*, menu Secure Boot du BIOS).

## 7. Limites connues de cette version

- Le script et l'image ont été écrits pour Debian 13 mais **testés seulement en
  partie** hors Debian 13 (environnement de développement sans accès aux
  miroirs Debian) : valider sur une première borne avant déploiement.
- La lecture des archives RAR nécessite `7zip-rar` (dépôt `non-free`).
