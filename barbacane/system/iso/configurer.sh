#!/bin/bash
# Exécuté dans le système installé (image mkosi ou ISO) après installation des paquets.
set -euo pipefail
install -m 644 /usr/share/barbacane/usbsas-config.toml /etc/usbsas/config.toml
install -m 644 /usr/share/barbacane/nftables.conf /etc/nftables.conf
systemctl enable nftables.service
systemctl disable clamav-freshclam.service 2>/dev/null || true
for s in networking NetworkManager systemd-networkd systemd-resolved wpa_supplicant ssh; do
    systemctl mask "$s.service" 2>/dev/null || true
done
# Jamais d'Internet : ni synchro NTP ni tâches apt périodiques
systemctl mask systemd-timesyncd.service apt-daily.timer apt-daily-upgrade.timer 2>/dev/null || true
if [ -n "${BARBACANE_PIN:-}" ]; then
    echo "$BARBACANE_PIN" | barbacane-admind --set-pin
fi
passwd -l root >/dev/null
chown -R clamav:clamav /var/lib/clamav 2>/dev/null || true
if command -v update-grub >/dev/null; then update-grub; fi
[ -f /etc/barbacane/maj.pub ] || echo "ATTENTION : /etc/barbacane/maj.pub absent : mises à jour impossibles" >&2
