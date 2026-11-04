#!/bin/sh
# Installateur Debian (busybox) : choisit le plus grand disque interne, jamais un
# périphérique USB (support d'installation, clés). Sans candidat : la question est posée.
media=$(mount | awk '$3 == "/cdrom" { print $1 }')
meilleur='' taille=0
for d in $(list-devices disk); do
    n=${d#/dev/}
    case "$media" in "$d"*) continue ;; esac
    case "$(readlink -f "/sys/block/$n")" in */usb*) continue ;; esac
    [ "$(cat "/sys/block/$n/removable" 2>/dev/null)" = 1 ] && continue
    s=$(cat "/sys/block/$n/size" 2>/dev/null || echo 0)
    [ "$s" -gt "$taille" ] && meilleur=$d taille=$s
done
if [ -n "$meilleur" ]; then
    logger -t barbacane "disque d'installation : $meilleur"
    debconf-set partman-auto/disk "$meilleur"
    debconf-set grub-installer/bootdev "$meilleur"
fi
