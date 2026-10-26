#!/bin/bash
# Scénarios de mise à jour de bout en bout : barbacane-update-prepare produit un
# paquet, barbacane-admind (dry_run) l'applique via sa socket.
# Valide / signature invalide / version plus ancienne / paquet corrompu.
# Prérequis : minisign, zstd, python3, `cargo build -p barbacane-admind`.
set -euo pipefail
B="$(cd "$(dirname "$0")/../.." && pwd)"
T="$(mktemp -d)"
trap 'kill "$PID" 2>/dev/null || true; rm -rf "$T"' EXIT
mkdir -p "$T"/{etat,import,clamav,yara,cles,src/clamav}

minisign -G -W -p "$T/cles/maj.pub" -s "$T/cles/maj.key" >/dev/null
minisign -G -W -p "$T/cles/autre.pub" -s "$T/cles/autre.key" >/dev/null
printf 'ClamAV-VDB:20 Oct 2026 08-00 +0000:27800:0:90:x:y:z:0' > "$T/src/clamav/daily.cld"

cat > "$T/admind.toml" <<EOF
socket = "$T/admin.sock"
socket_group = "$(id -gn)"
state_dir = "$T/etat"
analyzer_config = "$T/analyzer.toml"
analyzer_url = "http://127.0.0.1:9"
import_dir = "$T/import"
clamav_dir = "$T/clamav"
yara_dir = "$T/yara"
update_pubkey = "$T/cles/maj.pub"
dry_run = true
EOF
echo 246813 | "$B/target/debug/barbacane-admind" -c "$T/admind.toml" --set-pin >/dev/null
"$B/target/debug/barbacane-admind" -c "$T/admind.toml" 2>"$T/admind.log" &
PID=$!
for _ in $(seq 50); do [ -S "$T/admin.sock" ] && break; sleep 0.1; done

req() { python3 -I -c '
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
s.sendall((sys.argv[2] + "\n").encode()); print(s.makefile().readline().strip())' "$T/admin.sock" "$1"; }
TOK=$(req '{"op":"login","pin":"246813"}' | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["token"])')

prepare() { # version clé
    rm -rf "$T/out"
    "$B/update/barbacane-update-prepare" --cle "$T/cles/$2.key" --sortie "$T/out" --version "$1" \
        --clamav "$T/src/clamav" --yara "$B/tests/fixtures/yara" >/dev/null
}
deposer() { rm -f "$T"/import/*; cp "$T"/out/* "$T/import/"; }
appliquer() { req "{\"op\":\"apply_update\",\"token\":\"$TOK\"}"; }
ok() { echo "OK : $1"; }
attendu() { # motif réponse
    echo "$2" | grep -q "$1" || { echo "ÉCHEC : attendu « $1 », obtenu : $2" >&2; exit 1; }
}

prepare 100 maj; deposer; R=$(appliquer); attendu '"version":100' "$R"; ok "paquet valide appliqué"
if [ ! -f "$T/clamav/daily.cld" ] || [ ! -f "$T/yara/test.yar" ]; then echo "ÉCHEC : fichiers non installés" >&2; exit 1; fi
prepare 99 maj; deposer; attendu "retour arrière" "$(appliquer)"; ok "version plus ancienne refusée"
prepare 101 autre; deposer; attendu "signature invalide" "$(appliquer)"; ok "signature d'une autre clé refusée"
prepare 102 maj; deposer
python3 -I -c 'import sys; p=sys.argv[1]; b=bytearray(open(p,"rb").read()); b[len(b)//2]^=1; open(p,"wb").write(b)' "$T"/import/*.bcu
attendu "signature invalide" "$(appliquer)"; ok "paquet corrompu refusé"
prepare 103 maj; deposer; rm "$T"/import/*.minisig; attendu "signature" "$(appliquer)"; ok "signature absente refusée"
attendu '"update_version":100' "$(req '{"op":"status"}')"; ok "version installée inchangée après les refus"
echo "Tous les scénarios de mise à jour passent."
