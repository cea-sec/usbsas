# Écarts avec usbsas upstream

Base : `cea-sec/usbsas`, commit `69ebcf4` (usbsas-usbsas 0.3.4).

Aucun fichier du code usbsas n'est modifié. Tout le code Barbacane est sous `barbacane/`.
Ajouts hors code : `docs/`, `.github/workflows/barbacane.yml`.

## En attente de validation

### seccomp `files2fs` : comparaison du descripteur sur 64 bits
- Symptôme : `files2fs` est tué (SIGSYS) au formatage ; le transfert clé → clé échoue (« failed to fill whole buffer »).
- Cause : la règle `lseek` compare l'argument 0 sur 64 bits, or les 32 bits hauts du registre ne sont pas garantis nuls
  pour un `int` (ABI x86_64). Observé : `lseek(0x55bb00000003, 0x1fe, SEEK_SET)`. Dépend du compilateur.
- Correctif proposé (1 ligne, `usbsas-sandbox/src/files2fs.rs`) : `Cmp::MaskedEq` avec masque `0xffffffff`.
  Les autres règles sur descripteurs ont la même faiblesse latente ; à proposer au CEA en une PR.
