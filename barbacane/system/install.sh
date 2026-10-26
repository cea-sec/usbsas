#!/bin/bash
# Installation Barbacane sur Debian 13 fraîche (phase 1 : socle usbsas + analyseur ClamAV d'exemple).
# Idempotent : peut être relancé. Usage : sudo ./install.sh
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
SYS="$REPO/barbacane/system"

[ "$(id -u)" -eq 0 ] || { echo "À lancer en root." >&2; exit 1; }
grep -q 'VERSION_CODENAME=trixie' /etc/os-release || echo "Attention : Debian 13 (trixie) attendue." >&2

echo "== Dépendances de compilation et d'exécution"
apt-get update -q
apt-get install -y -q --no-install-recommends \
    pkgconf clang cmake git dpkg-dev curl ca-certificates libssl-dev libkrb5-dev \
    libseccomp-dev libudev-dev libusb-1.0-0-dev protobuf-compiler libdbus-1-dev \
    clamav clamav-daemon

if ! command -v cargo >/dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck disable=SC1091
. "${CARGO_HOME:-$HOME/.cargo}/env"
command -v cargo-deb >/dev/null || cargo install --locked cargo-deb

echo "== Paquets usbsas (serveur, analyseur d'exemple)"
cd "$REPO"
cargo build --release --locked -p usbsas-usbsas -p usbsas-net -p usbsas-analyzer-server \
    -p usbsas-cmdexec -p usbsas-dev2scsi -p usbsas-files2fs -p usbsas-files2tar \
    -p usbsas-local2files -p usbsas-fs2dev -p usbsas-identifier -p usbsas-scsi2files \
    -p usbsas-tar2files -p usbsas-usbdev
export USBSAS_BIN_PATH=/usr/libexec
cargo-deb --no-build -p usbsas-usbsas
cargo-deb --no-build -p usbsas-analyzer-server
apt-get install -y -q ./target/debian/usbsas-server_*.deb ./target/debian/usbsas-analyzer-server_*.deb

echo "== Configuration Barbacane"
install -m 644 "$SYS/etc/config.toml" /etc/usbsas/config.toml
install -d -o usbsas -g usbsas -m 700 /var/lib/usbsas/tmp /var/lib/usbsas/reports /var/lib/usbsas/analyzer
# Modules noyau de stockage USB interdits (fichier fourni par le paquet usbsas-server)
rmmod uas usb_storage 2>/dev/null || true

echo "== ClamAV hors ligne : pas de freshclam, clamd lancé par l'analyseur"
systemctl disable --now clamav-freshclam clamav-daemon 2>/dev/null || true
# Base de signatures : copiée depuis un paquet de mise à jour (phase 6) ; en attendant, à déposer à la main.
ls /var/lib/clamav/*.c[lv]d >/dev/null 2>&1 || echo "Attention : aucune base ClamAV dans /var/lib/clamav." >&2

install -d /etc/systemd/system/usbsas-analyzer-server.service.d
cat > /etc/systemd/system/usbsas-analyzer-server.service.d/barbacane.conf <<'UNIT'
[Unit]
After=
[Service]
ExecStart=
ExecStart=/usr/bin/usbsas-analyzer-server --working-dir /var/lib/usbsas/analyzer
UNIT
systemctl daemon-reload
systemctl enable --now usbsas-analyzer-server usbsas-server

echo "== Terminé"
