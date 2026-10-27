#!/bin/bash
# Installe l'ISO dans une machine virtuelle UEFI (ISO branchée comme une clé USB),
# redémarre sur le disque installé, capture l'écran puis inspecte le journal.
# Usage : qemu.sh barbacane.iso DOSSIER   (prérequis : qemu-system-x86, ovmf, qemu-utils, KVM)
set -euo pipefail
ISO="$(realpath "$1")"
W="$(realpath -m "$2")"
mkdir -p "$W"
OVMF=/usr/share/ovmf/OVMF.fd
qemu-img create -q -f qcow2 "$W/disque.qcow2" 16G
commun=(-enable-kvm -cpu host -m 4096 -smp 2 -bios "$OVMF" -vga std -display none
    -drive "file=$W/disque.qcow2,if=none,id=disque" -device "virtio-blk-pci,drive=disque,bootindex=1"
    -device qemu-xhci)

echo "== installation (sans intervention)"
# Même noyau et mêmes paramètres que le menu de l'ISO, plus une console série :
# l'installateur s'y affiche en texte (diagnostic lisible si une question bloque).
xorriso -osirrox on -indev "$ISO" -extract /install.amd/vmlinuz "$W/vmlinuz" \
    -extract /install.amd/initrd.gz "$W/initrd.gz" -extract /boot/grub/grub.cfg "$W/grub.cfg" >/dev/null 2>&1
params=$(awk '$1 == "linux" { $1 = ""; $2 = ""; print; exit }' "$W/grub.cfg")
# Sur console série, l'installateur ne propose que les langues ASCII (pas le français)
params=${params/locale=fr_FR.UTF-8/locale=en_US.UTF-8}
timeout 15m qemu-system-x86_64 "${commun[@]}" -no-reboot -serial "file:$W/installation.log" \
    -kernel "$W/vmlinuz" -initrd "$W/initrd.gz" -append "${params/---/console=ttyS0,115200 ---}" \
    -drive "if=none,id=cle,format=raw,readonly=on,file=$ISO" -device "usb-storage,drive=cle" \
    -monitor "unix:$W/mon-install.sock,server,nowait" &
PID=$!
# Captures régulières de l'installateur (diagnostic si blocage sur une question)
while kill -0 $PID 2>/dev/null; do
    sleep 60
    echo "screendump $W/installateur.png -f png" | socat - "UNIX-CONNECT:$W/mon-install.sock" >/dev/null 2>&1 || true
done
wait $PID || {
    echo "ÉCHEC : installation interrompue (délai dépassé ?), fin de la console :" >&2
    sed 's/\x1b\[[0-9;?]*[a-zA-Z]/\n/g' "$W/installation.log" | tr -s '\r\n' '\n' \
        | grep -v 'installer)\|^\s*$' | awk '!vu[$0]++' | tail -n 60
    exit 1
}

echo "== premier démarrage"
qemu-system-x86_64 "${commun[@]}" -serial "file:$W/demarrage.log" \
    -monitor "unix:$W/mon.sock,server,nowait" &
PID=$!
sleep 180
echo "screendump $W/ecran.png -f png" | socat - "UNIX-CONNECT:$W/mon.sock"
echo system_powerdown | socat - "UNIX-CONNECT:$W/mon.sock"
for _ in $(seq 60); do kill -0 $PID 2>/dev/null || break; sleep 2; done
kill $PID 2>/dev/null || true
wait $PID 2>/dev/null || true

echo "== inspection du disque installé"
modprobe nbd max_part=8
qemu-nbd -r -c /dev/nbd0 "$W/disque.qcow2"
trap 'umount "$W/racine" 2>/dev/null || true; qemu-nbd -d /dev/nbd0 >/dev/null' EXIT
sleep 2
mkdir -p "$W/racine"
racine=$(lsblk -lnpo NAME,FSTYPE /dev/nbd0 | awk '$2 == "ext4" { print $1; exit }')
mount -o ro,noload "$racine" "$W/racine"
J=(journalctl -D "$W/racine/var/log/journal" --no-pager)
"${J[@]}" -o short -u barbacane-kiosk -u barbacane-analyzer -u barbacane-admind -u usbsas-server \
    -u clamav-daemon -u hid-dealer > "$W/services.log" || true
"${J[@]}" -p err > "$W/erreurs.log" || true
for u in barbacane-analyzer barbacane-admind barbacane-kiosk usbsas-server; do
    echo "-- $u"; "${J[@]}" -o cat -u "$u" | awk '!vu[$0]++' | tail -n 12
done
echo "-- erreurs"; tail -n 40 "$W/erreurs.log"
echo "-- droits"; ls -ld "$W/racine" "$W/racine/var/lib/barbacane" "$W/racine/var/lib/barbacane/yara"
echo "-- base ClamAV"; ls -l "$W/racine/var/lib/clamav/"
ko=0
for u in barbacane-analyzer barbacane-admind barbacane-kiosk usbsas-server clamav-daemon; do
    if grep -q "Started $u.service" "$W/services.log" && ! grep -q "$u.service: Failed\|Failed to start $u" "$W/services.log"; then
        echo "OK : $u"
    else
        echo "ÉCHEC : $u" >&2; ko=1
    fi
done
# Restart=always masque un kiosque qui plante en boucle (écran noir).
n=$(grep -c "Started barbacane-kiosk.service" "$W/services.log" || true)
[ "$n" -le 2 ] || { echo "ÉCHEC : barbacane-kiosk redémarré $n fois" >&2; ko=1; }
exit $ko
