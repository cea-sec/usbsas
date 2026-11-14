#!/bin/bash
# Génère le contenu d'une « clé inconnue » de test dans $1 (dossier vide).
# Outils : zip, 7z, genisoimage, gcab, msibuild, qemu-img, mkfs.vfat, mtools, python3.
# Les limites de tests/fixtures/analyzer-test.toml sont calibrées sur ces fichiers.
set -euo pipefail

OUT="$1"
mkdir -p "$OUT"
cd "$OUT"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

# shellcheck disable=SC2016 # chaîne EICAR littérale
EICAR='X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*'
printf '%s' "$EICAR" > "$T/eicar.com"
echo "Bonjour, fichier sain." > "$T/sain.txt"

cp "$T/eicar.com" eicar.com
cp "$T/sain.txt" sain.txt
: > vide.txt
echo "BARBACANE-YARA-TEST" > yara.txt

# Archives propres / sales
(cd "$T" && zip -q -X propre.zip sain.txt) && mv "$T/propre.zip" .
(cd "$T" && zip -q -X mixte.zip eicar.com sain.txt) && mv "$T/mixte.zip" .
(cd "$T" && zip -q -X inner.zip eicar.com sain.txt && cp sain.txt autre.txt && 7z a -bd -y zip_dans_7z.7z inner.zip autre.txt >/dev/null) && mv "$T/zip_dans_7z.7z" .
mkdir -p "$T/iso" && cp "$T/eicar.com" "$T/sain.txt" "$T/iso/"
genisoimage -quiet -J -R -o eicar.iso "$T/iso"
(cd "$T" && tar czf mixte.tar.gz eicar.com sain.txt) && mv "$T/mixte.tar.gz" .

# Formats « disque » et Microsoft
truncate -s 8M "$T/fat.img"
mkfs.vfat -n TEST "$T/fat.img" >/dev/null
mcopy -i "$T/fat.img" "$T/eicar.com" ::eicar.com
mcopy -i "$T/fat.img" "$T/sain.txt" ::sain.txt
qemu-img convert -O vmdk "$T/fat.img" disque.vmdk
(cd "$T" && gcab -c -n mixte.cab eicar.com sain.txt) && mv "$T/mixte.cab" .
msibuild paquet.msi -s "$EICAR" "Barbacane" "Test" "{12345678-1234-1234-1234-123456789012}"

# Archives chiffrées (mot de passe : secret)
for n in bon faux abandon sans; do
    (cd "$T" && zip -q -X -P secret "chiffre-$n.zip" sain.txt) && mv "$T/chiffre-$n.zip" .
done
(cd "$T" && 7z a -bd -y -psecret -mhe=on chiffre-entetes.7z sain.txt >/dev/null) && mv "$T/chiffre-entetes.7z" .

# Bombes et limites
head -c 10000000 /dev/zero > "$T/zeros.bin"
(cd "$T" && zip -q -X -9 bombe-ratio.zip zeros.bin) && mv "$T/bombe-ratio.zip" .
cp "$T/sain.txt" "$T/n0.txt"
prev=n0.txt
for i in 1 2 3 4 5 6 7; do
    (cd "$T" && zip -q -X "n$i.zip" "$prev")
    prev="n$i.zip"
done
mv "$T/$prev" bombe-recursive.zip
mkdir -p "$T/many"
for i in $(seq 1 60); do echo "$i" > "$T/many/f$i.txt"; done
(cd "$T" && zip -q -X -r trop-de-fichiers.zip many) && mv "$T/trop-de-fichiers.zip" .
head -c 25000000 /dev/urandom > "$T/gros.bin"
(cd "$T" && zip -q -X -0 trop-gros.zip gros.bin) && mv "$T/trop-gros.zip" .

# Noms piégés
python3 -I - <<'PY'
import zipfile
with zipfile.ZipFile("noms-pieges.zip", "w") as z:
    z.writestr("../../../evasion.txt", "traversée")
    z.writestr("/absolu.txt", "absolu")
    z.writestr("été 𝄞 unicode.txt", "unicode")
    z.writestr("L" * 250 + ".txt", "long")
    z.writestr("dossier/Casse.txt", "1")
    z.writestr("dossier/casse.txt", "2")
PY
