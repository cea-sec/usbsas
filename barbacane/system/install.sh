#!/bin/bash
# Installation d'une borne Barbacane sur Debian 13 (trixie) minimale fraîche.
# Idempotent : peut être relancé (mise à niveau, reprise après erreur).
#
#   sudo barbacane/system/install.sh [options]
#     --debs DOSSIER        paquets déjà construits (usbsas-server, usbsas-hid, barbacane) ;
#                           sinon ils sont compilés depuis ce dépôt (Rust installé si besoin)
#     --cle-publique FICHIER   clé publique minisign des mises à jour (/etc/barbacane/maj.pub)
#     --clamav DOSSIER      base ClamAV initiale (*.cvd/*.cld) ; sinon téléchargée une fois (réseau requis)
#     --yara DOSSIER        règles YARA initiales
#     --pin                 demander le PIN administrateur (6 chiffres minimum)
#     --sans-rar            ne pas activer « non-free » pour 7zip-rar (archives RAR non lues)
#     --couper-reseau       en fin d'installation, désactiver définitivement le réseau
#
# Le réseau n'est utile que pendant l'installation (paquets Debian, compilation).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
DEBS='' CLE_PUB='' CLAMAV='' YARA='' PIN=0 RAR=1 COUPER=0
while [ $# -gt 0 ]; do
    case "$1" in
        --debs) DEBS="$2"; shift 2 ;;
        --cle-publique) CLE_PUB="$2"; shift 2 ;;
        --clamav) CLAMAV="$2"; shift 2 ;;
        --yara) YARA="$2"; shift 2 ;;
        --pin) PIN=1; shift ;;
        --sans-rar) RAR=0; shift ;;
        --couper-reseau) COUPER=1; shift ;;
        -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
        *) echo "option inconnue : $1" >&2; exit 1 ;;
    esac
done

etape() { echo; echo "== $*"; }
attention() { echo "ATTENTION : $*" >&2; }
[ "$(id -u)" -eq 0 ] || { echo "À lancer en root." >&2; exit 1; }
grep -q 'VERSION_CODENAME=trixie' /etc/os-release || attention "Debian 13 (trixie) attendue."
export DEBIAN_FRONTEND=noninteractive

etape "Dépôts Debian"
if [ "$RAR" = 1 ] && ! grep -rqs 'non-free' /etc/apt/sources.list /etc/apt/sources.list.d/; then
    # 7zip-rar (lecture des archives RAR) est dans « non-free ».
    if [ -f /etc/apt/sources.list.d/debian.sources ]; then
        sed -i 's/^Components: main.*/Components: main contrib non-free non-free-firmware/' /etc/apt/sources.list.d/debian.sources
    else
        sed -i 's/^\(deb .* trixie[^ ]* main\).*$/\1 contrib non-free non-free-firmware/' /etc/apt/sources.list
    fi
fi
apt-get update -q

etape "Paquets d'exécution"
apt-get install -y -q --no-install-recommends \
    clamav-daemon clamav-freshclam 7zip bubblewrap cage nftables minisign zstd \
    fonts-dejavu-core libwayland-egl1 libegl1 libgl1 mesa-vulkan-drivers dbus systemd-sysv udev kbd usbutils
[ "$RAR" = 1 ] && { apt-get install -y -q 7zip-rar || attention "7zip-rar indisponible : archives RAR non lues."; }

if [ -z "$DEBS" ]; then
    etape "Compilation (usbsas, Barbacane)"
    apt-get install -y -q --no-install-recommends pkgconf clang cmake git dpkg-dev curl ca-certificates \
        libssl-dev libkrb5-dev libseccomp-dev libudev-dev libusb-1.0-0-dev protobuf-compiler libdbus-1-dev \
        libwayland-dev libxkbcommon-dev
    if ! command -v cargo >/dev/null; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    fi
    # shellcheck disable=SC1091
    . "${CARGO_HOME:-$HOME/.cargo}/env"
    "$REPO/barbacane/system/build-debs.sh"
    DEBS="$REPO/barbacane/system/mkosi/paquets"
fi

etape "Installation des paquets"
apt-get install -y -q "$DEBS"/usbsas-server_*.deb "$DEBS"/barbacane_*.deb "$DEBS"/usbsas-hid_*.deb

etape "Configuration"
# Configuration usbsas : posée à la première installation, ensuite conservée (ports relevés sur place).
if ! grep -q 'barbacane-analyzer local' /etc/usbsas/config.toml 2>/dev/null; then
    install -m 644 /usr/share/barbacane/usbsas-config.toml /etc/usbsas/config.toml
fi
install -m 644 /usr/share/barbacane/nftables.conf /etc/nftables.conf
systemctl enable nftables.service
[ -n "$CLE_PUB" ] && install -m 644 "$CLE_PUB" /etc/barbacane/maj.pub
[ -f /etc/barbacane/maj.pub ] || attention "aucune clé publique de mise à jour (--cle-publique) : mises à jour impossibles."
if [ -n "$YARA" ]; then
    install -m 644 "$YARA"/*.yar /var/lib/barbacane/yara/ 2>/dev/null || true
fi
if command -v update-grub >/dev/null; then update-grub; fi

etape "Base antivirale initiale"
if [ -n "$CLAMAV" ]; then
    install -o clamav -g clamav -m 644 "$CLAMAV"/*.c[lv]d /var/lib/clamav/
elif ! ls /var/lib/clamav/*.c[lv]d >/dev/null 2>&1; then
    freshclam || attention "téléchargement ClamAV impossible : déposer une base avec --clamav ou une mise à jour."
fi
systemctl disable clamav-freshclam.service 2>/dev/null || true

etape "PIN administrateur"
if [ "$PIN" = 1 ]; then
    read -r -s -p "Nouveau PIN (6 chiffres minimum) : " p1; echo
    read -r -s -p "Confirmation : " p2; echo
    [ "$p1" = "$p2" ] || { echo "Les deux saisies diffèrent." >&2; exit 1; }
    echo "$p1" | barbacane-admind --set-pin
fi
[ -f /var/lib/barbacane/admin/pin ] || attention "aucun PIN administrateur : relancer avec --pin."

if [ "$COUPER" = 1 ]; then
    etape "Coupure du réseau"
    for s in networking NetworkManager systemd-networkd systemd-resolved wpa_supplicant ssh avahi-daemon; do
        systemctl disable --now "$s.service" 2>/dev/null || true
        systemctl mask "$s.service" 2>/dev/null || true
    done
    rfkill block all 2>/dev/null || true
fi

etape "Terminé"
cat <<'EOF'
Étapes restantes (voir docs/installation.md) :
  1. Relever les ports USB (usbsas-devmonitor) et renseigner [usb_port_accesses] dans /etc/usbsas/config.toml
     (port d'entrée dans ports_src ET ports_dst, port de sortie dans ports_dst, souris dans ports_hid).
  2. Réglages BIOS : mot de passe, démarrage sur le disque interne uniquement, Secure Boot.
  3. Redémarrer : la borne démarre directement sur l'interface Barbacane.
EOF
