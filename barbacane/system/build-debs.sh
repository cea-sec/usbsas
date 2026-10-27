#!/bin/bash
# Construit les paquets de production (sans « mock ») : usbsas-server, usbsas-hid, barbacane.
# Sortie : barbacane/system/mkosi/paquets/ (utilisés par install.sh --debs et par mkosi).
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$REPO/barbacane/system/mkosi/paquets"
# Outils de compilation installés si absents (ex. conteneur debian:trixie vierge).
# shellcheck disable=SC1091
[ -f "${CARGO_HOME:-$HOME/.cargo}/env" ] && . "${CARGO_HOME:-$HOME/.cargo}/env"
if ! command -v cargo >/dev/null; then
    [ "$(id -u)" = 0 ] || { echo "cargo absent : installer Rust (https://rustup.rs) ou lancer en root" >&2; exit 1; }
    apt-get update -q
    apt-get install -y -q --no-install-recommends pkgconf clang cmake git dpkg-dev curl ca-certificates \
        libssl-dev libkrb5-dev libseccomp-dev libudev-dev libusb-1.0-0-dev protobuf-compiler libdbus-1-dev \
        libwayland-dev libxkbcommon-dev libfuse3-dev build-essential
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # shellcheck disable=SC1091
    . "${CARGO_HOME:-$HOME/.cargo}/env"
fi
command -v cargo-deb >/dev/null || cargo install --locked cargo-deb
cd "$REPO"
export USBSAS_BIN_PATH=/usr/libexec
# Répertoire distinct des binaires « mock » des tests.
export CARGO_TARGET_DIR="$REPO/target/production"
cargo build --release --locked -p usbsas-usbsas -p usbsas-net -p usbsas-cmdexec -p usbsas-dev2scsi \
    -p usbsas-files2fs -p usbsas-files2tar -p usbsas-local2files -p usbsas-fs2dev -p usbsas-identifier \
    -p usbsas-scsi2files -p usbsas-tar2files -p usbsas-usbdev -p usbsas-hid
cargo-deb --no-build -p usbsas-usbsas
cargo-deb --no-build -p usbsas-hid
(cd barbacane && cargo build --release --locked -p barbacane-client -p barbacane-analyzer -p barbacane-admind \
    && cargo-deb --no-build -p barbacane-client)
mkdir -p "$OUT"
cp "$CARGO_TARGET_DIR"/debian/usbsas-server_*.deb "$CARGO_TARGET_DIR"/debian/usbsas-hid_*.deb "$CARGO_TARGET_DIR"/debian/barbacane_*.deb "$OUT/"
ls -l "$OUT"
