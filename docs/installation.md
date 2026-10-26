# Installation d'une borne Barbacane

> Document en cours : il sera complété à chaque phase (durcissement, kiosque, image `mkosi` en phase 7).

## Phase 1 : socle usbsas

Sur une Debian 13 (trixie) minimale fraîchement installée, avec accès réseau **uniquement pendant l'installation** :

```shell
git clone <dépôt> barbacane && cd barbacane
sudo barbacane/system/install.sh
```

Le script (idempotent) :
- installe les dépendances, Rust (rustup) et `cargo-deb` ;
- construit et installe les paquets `usbsas-server` et `usbsas-analyzer-server` (analyseur ClamAV d'exemple, remplacé en phase 2) ;
  le paquet `usbsas-server` crée l'utilisateur `usbsas`, la règle udev et bloque `usb_storage`/`uas` (`/etc/modprobe.d/usbsas.conf`) ;
- installe `barbacane/system/etc/config.toml` en `/etc/usbsas/config.toml` ;
- désactive `freshclam` et `clamav-daemon` (borne hors ligne ; clamd est lancé par l'analyseur).

### À faire à la main
- **Base ClamAV** : copier `main.cvd`, `daily.cvd`, `bytecode.cvd` dans `/var/lib/clamav/` (en attendant les mises à jour signées, phase 6).
- **Ports USB** : brancher une clé sur chaque port et relever son chemin avec `usbsas-devmonitor` (paquet `usbsas-tools`) ou `lsusb -t`,
  puis renseigner `[usb_port_accesses]` dans `/etc/usbsas/config.toml`. Le port d'entrée doit figurer dans `ports_src` **et** `ports_dst` (option Blanchir).
  Attention : un port USB 3 apparaît sur deux bus logiques (USB 2 et USB 3), déclarer les deux.

### Vérification sans matériel (mode mock)

```shell
barbacane/tests/phase1-mock.sh
```

Construit usbsas avec périphériques simulés par fichiers image et exécute les tests d'intégration :
le transfert clé → clé doit rejeter `infected/eicar.com`.
