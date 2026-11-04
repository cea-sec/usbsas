#!/bin/bash
# Environnement de démonstration / test de l'interface sans matériel :
# clamd (base de test), barbacane-analyzer, barbacane-admind (dry_run),
# usbsas-usbsas en mode mock servi sur socket (relancé après chaque session),
# Xvfb + barbacane-client (fonctionnalité x11).
#
# Usage : env.sh <dossier de travail> <image clé d'entrée> <image clé de sortie>
# Prérequis : barbacane/tests/phase1-mock.sh et `cargo build --features x11 -p barbacane-client`.
set -euo pipefail

W="$(realpath -m "$1")"
IN="$(realpath "$2")"
OUT="$(realpath "$3")"
REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
B="$REPO/barbacane"
FIX="$B/tests/fixtures"
mkdir -p "$W"/{run,sortie,export,rapports,import,yara,clamav,etat,usbsas,work}
cp "$FIX"/yara/*.yar "$W/yara/"

cat > "$W/clamd.conf" <<EOF
Foreground true
LocalSocket $W/clamd.sock
DatabaseDirectory $FIX/clamdb
TemporaryDirectory $W
User root
EOF
SEVEN=$(command -v 7zz || command -v 7z)
cat > "$W/analyzer.toml" <<EOF
listen = "127.0.0.1:18242"
work_dir = "$W/work"
clamd_socket = "$W/clamd.sock"
yara_rules_dir = "$W/yara"
seven_zip = "$SEVEN"
stage_dir = "$W/sortie"
reports_dir = "$W/rapports"
import_dir = "$W/import"
[limits]
password_timeout = 600
EOF
cat > "$W/admind.toml" <<EOF
socket = "$W/run/admin.sock"
socket_group = "$(id -gn)"
state_dir = "$W/etat"
analyzer_config = "$W/analyzer.toml"
analyzer_url = "http://127.0.0.1:18242"
import_dir = "$W/import"
export_dir = "$W/export"
reports_dir = "$W/rapports"
clamav_dir = "$W/clamav"
yara_dir = "$W/yara"
update_pubkey = "$W/maj.pub"
dry_run = true
EOF
cat > "$W/client.toml" <<EOF
usbsas_socket = "$W/run"
analyzer_url = "http://127.0.0.1:18242"
admin_socket = "$W/run/admin.sock"
stage_dir = "$W/sortie"
export_dir = "$W/export"
EOF
sed -e "s|/var/lib/usbsas/tmp|$W/usbsas|" -e "s|/var/lib/usbsas/reports|$W/rapports-usbsas|" \
    -e "s|/var/lib/barbacane/sortie|$W/sortie|" -e "s|/var/lib/barbacane/export|$W/export|" \
    -e "s|127.0.0.1:8042|127.0.0.1:18242|" "$B/system/etc/config.toml" > "$W/usbsas.toml"

/usr/sbin/clamd --config-file "$W/clamd.conf" & echo $! > "$W/pids"
"$B/target/debug/barbacane-analyzer" -c "$W/analyzer.toml" > "$W/analyzer.log" 2>&1 & echo $! >> "$W/pids"
"$B/target/debug/barbacane-admind" -c "$W/admind.toml" > "$W/admind.log" 2>&1 & echo $! >> "$W/pids"
echo 246813 | "$B/target/debug/barbacane-admind" -c "$W/admind.toml" --set-pin >/dev/null
(
    export USBSAS_MOCK_IN_DEV="$IN" USBSAS_MOCK_OUT_DEV="$OUT"
    while true; do setsid -w "$REPO/target/release/usbsas-usbsas" -c "$W/usbsas.toml" -s "$W/run" >> "$W/usbsas.log" 2>&1; sleep 0.2; done
) & echo $! >> "$W/pids"
Xvfb :42 -screen 0 1600x1000x24 >/dev/null 2>&1 & echo $! >> "$W/pids"
sleep 2
DISPLAY=:42 WGPU_BACKEND=gl "$B/target/debug/barbacane-client" -c "$W/client.toml" > "$W/client.log" 2>&1 & echo $! >> "$W/pids"
echo "environnement prêt dans $W (arrêt : kill \$(cat $W/pids))"
