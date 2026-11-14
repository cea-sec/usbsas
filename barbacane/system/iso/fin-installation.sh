#!/bin/sh
# Fin d'installation (installateur Debian) : paquets Barbacane et dépendances
# depuis l'ISO, fichiers propres à la borne, puis configuration du système installé.
# Le PIN est choisi au premier démarrage.
set -e
X=/cdrom/barbacane
mkdir -p /target/var/cache/barbacane-debs
cp $X/debs/*.deb /target/var/cache/barbacane-debs/
# Aucune source apt : sinon apt prend sur la source « cdrom: » ajoutée par l'installateur les
# paquets aussi présents dans le netinst, et sa méthode cdrom attend indéfiniment le disque
# (installation figée à 14 % de « Fin de l'installation »).
in-target sh -c 'apt-get install -y -q --no-install-recommends -o Dir::Etc::SourceList=/dev/null \
    -o Dir::Etc::SourceParts=/nonexistent /var/cache/barbacane-debs/*.deb'
rm -rf /target/var/cache/barbacane-debs
mkdir -p /target/etc/barbacane
[ -f $X/maj.pub ] && cp $X/maj.pub /target/etc/barbacane/maj.pub
[ -f $X/donnees.tar ] && tar -xf $X/donnees.tar -C /target
cp $X/configurer.sh /target/tmp/configurer.sh
in-target bash /tmp/configurer.sh
rm -f /target/tmp/configurer.sh
