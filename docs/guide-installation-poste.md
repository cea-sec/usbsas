# Guide pas à pas : installer un poste Barbacane

Ce guide s'adresse à la personne qui prépare une borne. Durée : environ 30 minutes
(dont 10 minutes d'installation automatique). Détails techniques : [installation.md](installation.md).

## Ce qu'il faut

| Élément | Remarque |
|---|---|
| Un PC x86_64 (HP, Dell…) | 8 Go de RAM minimum, disque interne ≥ 32 Go. **Le disque sera entièrement effacé.** |
| Une clé USB ≥ 2 Go | Elle servira de support d'installation (son contenu sera effacé). |
| Un poste administrateur connecté à Internet | Debian 13, ou n'importe quel Linux avec Docker, pour fabriquer l'ISO. |
| Un écran tactile, ou un écran et une souris | Le clavier n'est utile qu'au réglage du BIOS. |

## Étape 1 — Fabriquer l'ISO (une seule fois pour toutes les bornes)

Sur le poste administrateur :

```sh
git clone <dépôt> usbsas && cd usbsas
# Recommandé : la clé publique de signature des mises à jour
cp /chemin/barbacane-maj.pub barbacane/system/iso/maj.pub
# Debian 13, en root :
sudo barbacane/system/iso/build-iso.sh --sortie barbacane.iso
# ou, depuis un autre Linux :
docker run --rm --privileged -v "$PWD":/src -w /src debian:trixie \
    barbacane/system/iso/build-iso.sh --sortie barbacane.iso
```

Résultat : le fichier `barbacane.iso`. Il contient tout ce qu'il faut (Debian, Barbacane,
base antivirale du jour) : **l'installation ne demande aucun réseau**.

> Faute de poste administrateur, une ISO de test est disponible dans l'onglet
> *Actions* de GitHub (workflow « ISO », artefact `barbacane-iso`). Elle n'a pas de
> clé de mise à jour : à réserver aux essais.

## Étape 2 — Copier l'ISO sur la clé USB

- **Linux** : `sudo dd if=barbacane.iso of=/dev/sdX bs=4M status=progress conv=fsync`
  (remplacer `sdX` par la clé, vérifier avec `lsblk` : une erreur efface le mauvais disque).
- **Windows** : Rufus, en choisissant le mode **« image DD »** quand il le propose.
- **macOS / tous** : balenaEtcher.

## Étape 3 — Préparer le BIOS du poste

Entrer dans le BIOS (F10 sur HP, F2 sur Dell) :
1. Mode de démarrage **UEFI** (le BIOS classique fonctionne aussi).
2. Désactiver temporairement Secure Boot si le poste refuse de démarrer sur la clé.
3. Autoriser le démarrage sur USB, **le temps de l'installation seulement**.

## Étape 4 — Installer

1. Brancher la clé, allumer, choisir la clé dans le menu de démarrage (**F9** HP, **F12** Dell).
2. Le menu « Barbacane » s'affiche ; l'installation démarre seule après 10 secondes.
3. **Ne rien toucher.** Le plus grand disque interne est effacé (jamais un disque USB),
   le système est installé puis le poste redémarre tout seul (≈ 10 min).
4. **Retirer la clé** au redémarrage.

## Étape 5 — Premier démarrage

1. L'écran Barbacane apparaît.
2. Toucher l'accès **administrateur** : l'écran affiche
   « Premier démarrage : choisissez le PIN ». Saisir un PIN d'au moins 6 chiffres.
   **Le noter en lieu sûr** : il protège l'administration de la borne.

## Étape 6 — Verrouiller le BIOS

De retour dans le BIOS :
1. **Désactiver le démarrage sur USB** et laisser uniquement le disque interne.
2. Poser un **mot de passe administrateur BIOS**.
3. Réactiver Secure Boot si possible.
4. Ne pas brancher de câble réseau : la borne fonctionne entièrement hors ligne.

## Étape 7 — Vérifier

Insérer une clé contenant un fichier de test EICAR : la borne doit le signaler comme
menace. Insérer une clé saine : les fichiers doivent pouvoir être copiés vers une clé de sortie.

## En cas de problème

| Symptôme | Que faire |
|---|---|
| Le poste ne démarre pas sur la clé | Vérifier l'étape 3 (démarrage USB, Secure Boot) ; recopier l'ISO en mode « DD ». |
| L'installation s'arrête sur une question | Le poste a plusieurs disques ambigus : débrancher les disques en trop et relancer. |
| Écran noir après installation | Brancher un clavier, `Ctrl+Alt+F2`, et consulter `journalctl -b -u barbacane-kiosk` (voir [exploitation.md](exploitation.md)). |
| PIN oublié | Réinstaller le poste (étapes 4 à 6). |

Pour les mises à jour et la base antivirale : [administration.md](administration.md).
