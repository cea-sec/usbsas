# Écarts avec usbsas upstream

Base : `cea-sec/usbsas`, commit `69ebcf4` (usbsas-usbsas 0.3.4).

Le code usbsas n'est modifié que par les écarts listés ci-dessous (commits dédiés). Tout le code Barbacane est sous `barbacane/`.
Ajouts hors code : `docs/`, `.github/workflows/barbacane.yml`.

## Écarts appliqués

### 1. seccomp `files2fs` : comparaison du descripteur sur 64 bits
- Symptôme : `files2fs` est tué (SIGSYS) au formatage ; le transfert clé → clé échoue (« failed to fill whole buffer »).
- Cause : la règle `lseek` compare l'argument 0 sur 64 bits, or les 32 bits hauts du registre ne sont pas garantis nuls
  pour un `int` (ABI x86_64). Observé : `lseek(0x55bb00000003, 0x1fe, SEEK_SET)`. Dépend du compilateur.
- Correctif (`usbsas-sandbox/src/*.rs`) : toutes les comparaisons de l'argument 0 (descripteur) passent en
  `Cmp::MaskedEq` avec masque `0xffffffff` (le noyau ne lit que 32 bits pour un `int`). À proposer au CEA en PR.
