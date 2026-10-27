# Notes pour Claude Code — projet BARBACANE

Borne de décontamination USB hors ligne (Debian 13) construite sur usbsas (CEA).
Code Barbacane : `barbacane/`. Documentation : `docs/` (conception, installation,
exploitation, administration, tests, ecarts-upstream).

## Règles permanentes (fixées par l'utilisateur)
- Ne pas modifier le cœur d'usbsas (`usbsas-*/`) sans validation ; tout écart minimal
  et documenté dans `docs/ecarts-upstream.md`.
- Rust, sécurité d'abord, petits commits, tests + CI. Interface et docs en français.
- La borne n'est JAMAIS sur le réseau, même à l'installation : seule l'ISO est valable ; `install.sh` = maquette uniquement.
- Branche de travail : `claude/affectionate-galileo-w0bjm2`.

## Vérifications locales
- `barbacane/tests/phase1-mock.sh` (usbsas mock, EICAR)
- `cd barbacane && cargo fmt --all --check && cargo clippy --no-deps --all-targets -- -D warnings && cargo test`
- `cd barbacane && cargo build -p barbacane-admind && tests/maj/maj.sh`
- `shellcheck barbacane/system/*.sh barbacane/system/iso/*.sh barbacane/tests/*.sh barbacane/tests/*/*.sh`
- Build debug lourd : profil dev à debuginfo réduit (disque).

## ISO d'installation hors ligne (`barbacane/system/iso/`)
- `build-iso.sh` (root, Debian 13 ou conteneur `debian:trixie --privileged`) :
  ISO netinst officielle + paquets Barbacane et dépendances (résolues dans un
  debootstrap) + base ClamAV + `maj.pub` (déposer la clé dans `barbacane/system/iso/maj.pub`).
- `preseed.cfg` : aucune question ; langue via `locale=` du menu ; mot de passe root
  aléatoire (exigé par d-i) puis `passwd -l` dans `configurer.sh`.
- `choisir-disque.sh` : plus grand disque non-USB. `fin-installation.sh` : installe
  les .deb depuis l'ISO puis lance `configurer.sh` (partagé avec mkosi).
- simple-cdd/debian-cd ont été ABANDONNÉS (exigent l'installateur i386, absent de trixie).
- CI : `.github/workflows/iso.yml` = construction + test `barbacane/tests/iso/qemu.sh`
  (QEMU UEFI, ISO en clé USB, installateur sur console série en anglais car la série
  n'affiche pas le français, puis inspection du journal du disque installé).

## État au 27/10/2026
- CI ISO verte (run n°29) : construction + installation automatique sous QEMU UEFI
  + premier démarrage avec les 5 services OK (analyzer, admind, kiosk, usbsas-server, clamav).
- Défauts corrigés grâce au test QEMU : tmpfs de l'analyseur avec uid numérique
  (drop-in écrit par `system/deb/postinst`), dépendances `libpam-systemd` et
  `ca-certificates` (sans elles usbsas échoue en « reqwest builder error »),
  dossier YARA hors StateDirectory d'admind (systemd le remettait en 0750).

- Écran noir sur vrai matériel corrigé : pam_systemd remplaçait XDG_RUNTIME_DIR par
  /run/user/<uid> (lecture seule, ProtectSystem=strict) → cage « Unable to open Wayland
  socket ». Rendu logiciel imposé (WLR_RENDERER=pixman, ICED_BACKEND=tiny-skia).
  Le test QEMU vérifie maintenant l'écran (pixels non noirs) et contient une sonde diag.

## Reste à faire
2. Installation pilote sur vrai matériel HP/Dell avec vraies clés USB ; vérifier
   l'installateur en français sur écran réel et le démarrage graphique (cage).
3. Image mkosi jamais construite.
4. Le drop-in tmpfs n'est pas supprimé à la purge du paquet (mineur).
