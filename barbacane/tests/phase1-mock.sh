#!/bin/bash
# Phase 1 : transfert clé → clé en mode mock, EICAR détecté par ClamAV (tests d'intégration usbsas).
# Prérequis : dépendances de compilation + clamd installés (voir system/install.sh).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build --release --all --features mock,integration-tests
cargo test --release -p usbsas-usbsas --features integration-tests
