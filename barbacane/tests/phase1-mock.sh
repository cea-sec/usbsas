#!/bin/bash
# Phase 1 : transfert clé → clé en mode mock, EICAR détecté par ClamAV (tests d'intégration usbsas).
# Prérequis : dépendances de compilation + clamd installés (voir system/install.sh).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --release --all --features mock,integration-tests
# assert_cmd ≥ 2.1 ne retrouve plus seul un binaire d'un autre paquet avec les Cargo
# récents (build-dir séparé) : on lui donne son chemin (le test usbsas reste inchangé).
CIBLE="${CARGO_TARGET_DIR:-$PWD/target}/release"
env "CARGO_BIN_EXE_usbsas-analyzer-server=$CIBLE/usbsas-analyzer-server" \
    cargo test --release -p usbsas-usbsas --features integration-tests
