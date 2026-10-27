#!/bin/bash
# Construit l'ISO d'installation hors ligne de la borne : ISO officielle Debian 13
# « netinst » (système de base) + paquets Barbacane et toutes leurs dépendances.
# À lancer en root sur Debian 13 (ou dans un conteneur debian:trixie), réseau requis
# pendant la construction seulement. L'ISO installe sans aucune question ni réseau.
#
#   barbacane/system/iso/build-iso.sh [--debs DOSSIER] [--cle-publique maj.pub]
#                                     [--clamav DOSSIER] [--yara DOSSIER] [--netinst ISO] [--sortie FICHIER.iso]
#     --debs          paquets usbsas-server, usbsas-hid, barbacane (défaut : construits par build-debs.sh)
#     --cle-publique  clé minisign des mises à jour (défaut : barbacane/system/iso/maj.pub s'il existe)
#     --clamav        base ClamAV initiale (défaut : téléchargée avec freshclam)
#     --yara          règles YARA initiales
#     --netinst       ISO netinst Debian 13 amd64 déjà téléchargée (défaut : dernière version, empreinte vérifiée)
set -euo pipefail
ICI="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$ICI/../../.." && pwd)"
DEBS='' CLE_PUB='' CLAMAV='' YARA='' NETINST='' SORTIE="$PWD/barbacane.iso"
[ -f "$ICI/maj.pub" ] && CLE_PUB="$ICI/maj.pub"
while [ $# -gt 0 ]; do
    case "$1" in
        --debs) DEBS="$(realpath "$2")"; shift 2 ;;
        --cle-publique) CLE_PUB="$(realpath "$2")"; shift 2 ;;
        --clamav) CLAMAV="$(realpath "$2")"; shift 2 ;;
        --yara) YARA="$(realpath "$2")"; shift 2 ;;
        --netinst) NETINST="$(realpath "$2")"; shift 2 ;;
        --sortie) SORTIE="$(realpath -m "$2")"; shift 2 ;;
        -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
        *) echo "option inconnue : $1" >&2; exit 1 ;;
    esac
done
[ "$(id -u)" -eq 0 ] || { echo "À lancer en root." >&2; exit 1; }
export DEBIAN_FRONTEND=noninteractive
MIROIR=http://deb.debian.org/debian
apt-get update -q
apt-get install -y -q --no-install-recommends xorriso debootstrap wget ca-certificates openssl clamav-freshclam

if [ -z "$DEBS" ]; then
    "$REPO/barbacane/system/build-debs.sh"
    DEBS="$REPO/barbacane/system/mkosi/paquets"
fi

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
B="$T/iso/barbacane"
mkdir -p "$B/debs"

echo "== ISO netinst Debian 13"
if [ -z "$NETINST" ]; then
    URL=https://cdimage.debian.org/debian-cd/current/amd64/iso-cd
    wget -q -O "$T/SHA256SUMS" "$URL/SHA256SUMS"
    NOM=$(awk '/-amd64-netinst\.iso$/ { print $2; exit }' "$T/SHA256SUMS")
    wget -q -O "$T/$NOM" "$URL/$NOM"
    (cd "$T" && grep " $NOM\$" SHA256SUMS | sha256sum -c -)
    NETINST="$T/$NOM"
fi

echo "== Paquets et dépendances (résolus dans un système Debian 13 minimal)"
debootstrap --variant=minbase trixie "$T/chroot" "$MIROIR" >/dev/null
echo "deb $MIROIR trixie main contrib non-free non-free-firmware" > "$T/chroot/etc/apt/sources.list"
mkdir -p "$T/chroot/debs"
cp "$DEBS"/usbsas-server_*.deb "$DEBS"/usbsas-hid_*.deb "$DEBS"/barbacane_*.deb "$T/chroot/debs/"
chroot "$T/chroot" apt-get update -q
mapfile -t PAQUETS < <(grep -vE '^\s*(#|$)|^(barbacane|usbsas-server|usbsas-hid)$' "$ICI/paquets.txt")
chroot "$T/chroot" sh -c 'apt-get install -y -q --download-only --no-install-recommends "$@" /debs/*.deb' sh "${PAQUETS[@]}"
cp "$T"/chroot/var/cache/apt/archives/*.deb "$T"/chroot/debs/*.deb "$B/debs/"
echo "$(find "$B/debs" -name '*.deb' | wc -l) paquets embarqués"

echo "== Données initiales"
mkdir -p "$T/donnees/var/lib/clamav" "$T/donnees/var/lib/barbacane/yara"
if [ -n "$CLAMAV" ]; then
    cp "$CLAMAV"/*.c[lv]d "$T/donnees/var/lib/clamav/"
else
    printf 'DatabaseMirror database.clamav.net\nDatabaseDirectory %s\nDatabaseOwner root\n' \
        "$T/donnees/var/lib/clamav" > "$T/freshclam.conf"
    freshclam --config-file="$T/freshclam.conf" --foreground \
        || echo "ATTENTION : base ClamAV non téléchargée, la borne refusera tout jusqu'à une mise à jour." >&2
fi
[ -n "$YARA" ] && cp "$YARA"/*.yar "$T/donnees/var/lib/barbacane/yara/"
tar -cf "$B/donnees.tar" -C "$T/donnees" .
if [ -n "$CLE_PUB" ]; then
    cp "$CLE_PUB" "$B/maj.pub"
else
    echo "ATTENTION : aucune clé publique de mise à jour : mises à jour impossibles sur les bornes." >&2
fi
# L'installateur exige un mot de passe root : aléatoire, jamais conservé, compte verrouillé ensuite
sed "s|@HACHE_ROOT@|$(head -c 32 /dev/urandom | base64 | openssl passwd -6 -stdin)|" "$ICI/preseed.cfg" > "$B/preseed.cfg"
cp "$ICI/choisir-disque.sh" "$ICI/fin-installation.sh" "$ICI/configurer.sh" "$B/"

echo "== Assemblage"
# Menu de démarrage unique (BIOS et UEFI) : installation automatique après 10 s
PARAMS="auto=true priority=critical preseed/file=/cdrom/barbacane/preseed.cfg locale=fr_FR.UTF-8 keymap=fr --- quiet"
cat > "$T/grub.cfg" <<CFG
set timeout=10
set default=0
menuentry "Installer BARBACANE (EFFACE le disque interne)" {
    linux /install.amd/vmlinuz $PARAMS
    initrd /install.amd/initrd.gz
}
CFG
cat > "$T/isolinux.cfg" <<CFG
default barbacane
prompt 0
timeout 100
say Installation BARBACANE dans 10 s : le disque interne sera EFFACE.
label barbacane
  kernel /install.amd/vmlinuz
  append initrd=/install.amd/initrd.gz $PARAMS
CFG
rm -f "$SORTIE"
xorriso -indev "$NETINST" -outdev "$SORTIE" -map "$B" /barbacane \
    -map "$T/grub.cfg" /boot/grub/grub.cfg -map "$T/isolinux.cfg" /isolinux/isolinux.cfg \
    -boot_image any replay
ls -lh "$SORTIE"
