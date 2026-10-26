#!/bin/bash
# Construit l'ISO d'installation hors ligne de la borne (Debian 13 + Barbacane).
# À lancer en root sur Debian 13 (ou dans un conteneur debian:trixie), réseau requis
# pendant la construction seulement. L'ISO installe sans aucune question ni réseau.
#
#   barbacane/system/iso/build-iso.sh [--debs DOSSIER] [--cle-publique maj.pub]
#                                     [--clamav DOSSIER] [--yara DOSSIER] [--sortie FICHIER.iso]
#     --debs          paquets usbsas-server, usbsas-hid, barbacane (défaut : construits par build-debs.sh)
#     --cle-publique  clé minisign des mises à jour (défaut : barbacane/system/iso/maj.pub s'il existe)
#     --clamav        base ClamAV initiale (défaut : téléchargée avec freshclam)
#     --yara          règles YARA initiales
set -euo pipefail
ICI="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$ICI/../../.." && pwd)"
DEBS='' CLE_PUB='' CLAMAV='' YARA='' SORTIE="$PWD/barbacane.iso"
[ -f "$ICI/maj.pub" ] && CLE_PUB="$ICI/maj.pub"
while [ $# -gt 0 ]; do
    case "$1" in
        --debs) DEBS="$(realpath "$2")"; shift 2 ;;
        --cle-publique) CLE_PUB="$(realpath "$2")"; shift 2 ;;
        --clamav) CLAMAV="$(realpath "$2")"; shift 2 ;;
        --yara) YARA="$(realpath "$2")"; shift 2 ;;
        --sortie) SORTIE="$(realpath -m "$2")"; shift 2 ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) echo "option inconnue : $1" >&2; exit 1 ;;
    esac
done
[ "$(id -u)" -eq 0 ] || { echo "À lancer en root." >&2; exit 1; }
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -y -q --no-install-recommends simple-cdd distro-info-data xorriso isolinux syslinux-utils \
    grub-efi-amd64-bin grub-pc-bin mtools dosfstools cpio wget ca-certificates clamav-freshclam

if [ -z "$DEBS" ]; then
    "$REPO/barbacane/system/build-debs.sh"
    DEBS="$REPO/barbacane/system/mkosi/paquets"
fi

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
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
tar -cf "$T/donnees.tar" -C "$T/donnees" .
[ -n "$CLE_PUB" ] || echo "ATTENTION : aucune clé publique de mise à jour : mises à jour impossibles sur les bornes." >&2

# shortcut: simple-cdd télécharge encore l'installateur i386, supprimé de trixie ; à retirer quand simple-cdd sera corrigé
sed -i 's/if a == "amd64" and "i386" not in self.env.get("ARCHES"):/if False:/' \
    /usr/bin/build-simple-cdd /usr/share/simple-cdd/build-simple-cdd

# simple-cdd : dépôt hors ligne (paquets + dépendances) et installateur Debian
cp -r "$ICI/profiles" "$T/"
EXTRAS="$ICI/choisir-disque.sh $ICI/fin-installation.sh $ICI/configurer.sh $T/donnees.tar"
[ -n "$CLE_PUB" ] && { cp "$CLE_PUB" "$T/maj.pub"; EXTRAS="$EXTRAS $T/maj.pub"; }
cat > "$T/barbacane.conf" <<CONF
profiles="barbacane"
ARCH="amd64"
ARCHES="amd64"
auto_profiles="barbacane"
debian_mirror="http://deb.debian.org/debian/"
mirror_components="main contrib non-free non-free-firmware"
local_packages="$DEBS"
all_extras="$EXTRAS"
CONF
# build-simple-cdd refuse root : compte de construction dédié
id -u barbacane-iso >/dev/null 2>&1 || useradd --system --home-dir "$T" barbacane-iso
chown -R barbacane-iso "$T"
chmod 755 "$T"
(cd "$T" && runuser -u barbacane-iso -- env HOME="$T" ARCH=amd64 ARCHES=amd64 \
    build-simple-cdd --verbose --conf barbacane.conf --dist trixie --locale fr_FR --keyboard fr)
ISO="$(find "$T/images" -name "*.iso" | head -n1)"

# Menu de démarrage unique (BIOS et UEFI) : installation automatique après 10 s
PARAMS="auto=true priority=critical preseed/file=/cdrom/simple-cdd/barbacane.preseed locale=fr_FR.UTF-8 keymap=fr --- quiet"
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
xorriso -indev "$ISO" -outdev "$SORTIE" \
    -map "$T/grub.cfg" /boot/grub/grub.cfg -map "$T/isolinux.cfg" /isolinux/isolinux.cfg \
    -boot_image any replay
ls -lh "$SORTIE"
