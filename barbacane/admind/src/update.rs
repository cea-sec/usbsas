//! Paquets de mise à jour hors ligne `*.bcu` (tar.zst) signés par minisign
//! (`*.bcu.minisig`).
//!
//! Contenu du paquet :
//! ```text
//! manifest.json   {"format":1,"version":N,"date":"…","files":{"chemin":"sha256",…}}
//! clamav/*.cvd|cld|cud   signatures ClamAV
//! yara/*.yar|yara        règles YARA
//! debs/*.deb             paquets Debian (OS, usbsas, Barbacane)
//! cles/maj.pub           (facultatif) nouvelle clé publique de signature
//! ```
//! Ordre : signature du fichier entier → version strictement supérieure à la
//! version installée (anti-retour arrière) → extraction → empreintes du
//! manifeste → application (sauvegarde puis remplacement ; restauration si
//! une étape échoue).

use crate::Ctx;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub version: u64,
    pub date: String,
    pub files: BTreeMap<String, String>,
}

pub struct Outcome {
    pub version: u64,
    pub report: String,
    pub reboot: bool,
}

/// Trouve le paquet le plus récent (nom) et sa signature dans `dir`.
pub fn find(dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "bcu"))
        .collect();
    found.sort();
    let pkg = found
        .pop()
        .context("aucun paquet *.bcu trouvé sur la clé")?;
    let sig = PathBuf::from(format!("{}.minisig", pkg.display()));
    if !sig.is_file() {
        bail!(
            "signature {} absente",
            sig.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    Ok((pkg, sig))
}

/// Vérifie la signature minisign du fichier entier (en flux).
pub fn verify_signature(pubkey: &Path, pkg: &Path, sig: &Path) -> Result<()> {
    let pk = minisign_verify::PublicKey::from_file(pubkey)
        .map_err(|e| anyhow::anyhow!("clé publique : {e}"))?;
    let sig = minisign_verify::Signature::from_file(sig)
        .map_err(|e| anyhow::anyhow!("signature illisible : {e}"))?;
    let mut v = pk
        .verify_stream(&sig)
        .map_err(|_| anyhow::anyhow!("signature invalide (clé de signature inconnue)"))?;
    let mut f = std::fs::File::open(pkg)?;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        v.update(&buf[..n]);
    }
    v.finalize()
        .map_err(|_| anyhow::anyhow!("signature invalide : paquet modifié ou corrompu"))
}

fn sha256(p: &Path) -> Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Extrait le paquet (déjà authentifié) et contrôle le manifeste.
pub fn unpack(pkg: &Path, dst: &Path) -> Result<Manifest> {
    let dec = zstd::Decoder::new(std::fs::File::open(pkg)?)?;
    let mut ar = tar::Archive::new(dec);
    ar.set_preserve_permissions(false);
    ar.set_unpack_xattrs(false);
    for e in ar.entries()? {
        let mut e = e?;
        let path = e.path()?.into_owned();
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            bail!("chemin interdit dans le paquet : {}", path.display());
        }
        match e.header().entry_type() {
            tar::EntryType::Regular | tar::EntryType::Directory => {
                e.unpack_in(dst)?;
            }
            t => bail!(
                "type d'entrée interdit dans le paquet ({t:?}) : {}",
                path.display()
            ),
        }
    }
    let m: Manifest = serde_json::from_slice(
        &std::fs::read(dst.join("manifest.json")).context("manifest.json absent")?,
    )
    .context("manifest.json invalide")?;
    if m.format != 1 {
        bail!("format de paquet {} non pris en charge", m.format);
    }
    for (rel, want) in &m.files {
        let p = dst.join(rel);
        if !p.is_file() {
            bail!("fichier annoncé absent : {rel}");
        }
        if &sha256(&p)? != want {
            bail!("empreinte incorrecte : {rel}");
        }
    }
    // Rien d'autre que le manifeste et les fichiers listés.
    for dir in ["clamav", "yara", "debs", "cles"] {
        if let Ok(rd) = std::fs::read_dir(dst.join(dir)) {
            for e in rd {
                let rel = format!("{dir}/{}", e?.file_name().to_string_lossy());
                if !m.files.contains_key(&rel) {
                    bail!("fichier non listé dans le manifeste : {rel}");
                }
            }
        }
    }
    Ok(m)
}

fn files_in(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    v.retain(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
    });
    v.sort();
    v
}

/// Copie `src` vers `dir` en gardant une sauvegarde du contenu actuel dans `backup`.
fn replace_dir(files: &[PathBuf], dir: &Path, backup: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let _ = std::fs::remove_dir_all(backup);
    std::fs::create_dir_all(backup)?;
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_file() {
            std::fs::copy(&p, backup.join(p.file_name().unwrap()))?;
        }
    }
    for f in files {
        let tmp = dir.join(format!(".{}.tmp", f.file_name().unwrap().to_string_lossy()));
        std::fs::copy(f, &tmp)?;
        std::fs::rename(&tmp, dir.join(f.file_name().unwrap()))?;
    }
    Ok(())
}

fn restore_dir(dir: &Path, backup: &Path) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_file() && !backup.join(p.file_name().unwrap()).exists() {
            std::fs::remove_file(&p)?;
        }
    }
    for e in std::fs::read_dir(backup)? {
        let p = e?.path();
        std::fs::copy(&p, dir.join(p.file_name().unwrap()))?;
    }
    Ok(())
}

pub fn apply(ctx: &Ctx) -> Result<Outcome> {
    let c = &ctx.conf;
    let (pkg, sig) = find(&c.import_dir)?;
    verify_signature(&c.update_pubkey, &pkg, &sig)?;
    let work = tempfile::Builder::new()
        .prefix("maj-")
        .tempdir_in(&c.state_dir)?;
    let m = unpack(&pkg, work.path())?;
    let current = ctx.update_version();
    if m.version <= current {
        bail!(
            "paquet version {} refusé : version installée {current} (retour arrière interdit)",
            m.version
        );
    }
    let mut report = format!(
        "Paquet de mise à jour version {} du {}\n",
        m.version, m.date
    );
    let backup = c.state_dir.join("sauvegarde");
    let clam = files_in(
        &work.path().join("clamav"),
        &["cvd", "cld", "cud", "ign2", "ftm"],
    );
    let yara = files_in(&work.path().join("yara"), &["yar", "yara"]);
    let debs = files_in(&work.path().join("debs"), &["deb"]);

    let mut done_clam = false;
    let mut done_yara = false;
    let res: Result<()> = (|| {
        if !clam.is_empty() {
            replace_dir(&clam, &c.clamav_dir, &backup.join("clamav"))?;
            done_clam = true;
            ctx.run(&["systemctl", "restart", "clamav-daemon"])?;
            report += &format!("- signatures ClamAV : {} fichier(s)\n", clam.len());
        }
        if !yara.is_empty() {
            replace_dir(&yara, &c.yara_dir, &backup.join("yara"))?;
            done_yara = true;
            report += &format!("- règles YARA : {} fichier(s)\n", yara.len());
        }
        if done_clam || done_yara {
            ctx.run(&["systemctl", "restart", "barbacane-analyzer"])?;
        }
        Ok(())
    })();
    if let Err(e) = res {
        // Retour arrière des signatures et règles.
        if done_clam {
            let _ = restore_dir(&c.clamav_dir, &backup.join("clamav"));
        }
        if done_yara {
            let _ = restore_dir(&c.yara_dir, &backup.join("yara"));
        }
        let _ = ctx.run(&["systemctl", "restart", "clamav-daemon"]);
        let _ = ctx.run(&["systemctl", "restart", "barbacane-analyzer"]);
        bail!("échec de l'application, signatures et règles restaurées : {e:#}");
    }
    let mut reboot = false;
    if !debs.is_empty() {
        // Installation hors ligne : uniquement les paquets fournis, aucune source réseau.
        let mut argv: Vec<String> = [
            "apt-get",
            "install",
            "-y",
            "--no-download",
            "-o",
            "Dir::Etc::SourceList=/dev/null",
            "-o",
            "Dir::Etc::SourceParts=/dev/null",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        argv.extend(debs.iter().map(|d| d.display().to_string()));
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        if let Err(e) = ctx.run(&argv) {
            let _ = ctx.run(&["dpkg", "--configure", "-a"]);
            bail!(
                "échec de l'installation des paquets Debian ({e:#}). Signatures et règles déjà appliquées ; \
                 version du paquet non enregistrée : réappliquez un paquet corrigé."
            );
        }
        report += &format!("- paquets Debian : {} installé(s)\n", debs.len());
        reboot = true;
    }
    // Rotation de la clé de signature : la nouvelle clé publique arrive signée par l'ancienne.
    let newkey = work.path().join("cles/maj.pub");
    if newkey.is_file() {
        minisign_verify::PublicKey::from_file(&newkey)
            .map_err(|e| anyhow::anyhow!("nouvelle clé publique invalide : {e}"))?;
        let tmp = c.update_pubkey.with_extension("pub.tmp");
        std::fs::copy(&newkey, &tmp)?;
        std::fs::rename(&tmp, &c.update_pubkey)?;
        report += "- clé de signature des mises à jour remplacée\n";
    }
    ctx.set_update_version(m.version)?;
    let _ = std::fs::remove_file(&pkg);
    let _ = std::fs::remove_file(&sig);
    if reboot {
        report += "Redémarrage nécessaire pour terminer la mise à jour.\n";
    }
    Ok(Outcome {
        version: m.version,
        report,
        reboot,
    })
}
