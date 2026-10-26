#!/bin/sh
# Fin d'installation (installateur Debian) : fichiers propres à la borne puis
# configuration dans le système installé. Le PIN est choisi au premier démarrage.
set -e
X=/cdrom/simple-cdd
mkdir -p /target/etc/barbacane /target/var/lib/clamav /target/var/lib/barbacane/yara
[ -f $X/maj.pub ] && cp $X/maj.pub /target/etc/barbacane/maj.pub
[ -f $X/donnees.tar ] && tar -xf $X/donnees.tar -C /target
cp $X/configurer.sh /target/tmp/configurer.sh
in-target bash /tmp/configurer.sh
rm -f /target/tmp/configurer.sh
