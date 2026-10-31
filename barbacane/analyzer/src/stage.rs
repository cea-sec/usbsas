//! Dossier de préparation : arbre final des fichiers sains + rapports,
//! relu ensuite par usbsas (source `LocalDir`) pour écrire la clé de sortie.

use crate::job::{Job, State, Status};
use crate::report;
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Contenu à écrire : chemin relatif de sortie → fichier local ou données.
pub enum Source {
    File(PathBuf),
    Data(Vec<u8>),
}

/// Arbre final : fichiers sains (noms uniques, exFAT ignorant la casse) + rapports.
pub fn plan(job: &Job, device: &str) -> Result<Vec<(PathBuf, Source)>> {
    let s = job.lock();
    if s.status != Status::Scanned {
        bail!("analyse non terminée");
    }
    let mut taken = HashMap::new();
    let mut out = Vec::new();
    for e in s.entries.iter() {
        let Some(o) = &e.output else { continue };
        out.push((
            unique(safe_rel(o)?, &mut taken),
            Source::File(e.local.clone()),
        ));
    }
    let stamp = s.started.strftime("%Y%m%d-%H%M%S").to_string();
    out.push((
        format!("rapport-barbacane-{stamp}.txt").into(),
        Source::Data(report::text(job, &s, device).into_bytes()),
    ));
    out.push((
        format!("rapport-barbacane-{stamp}.html").into(),
        Source::Data(report::html(job, &s, device).into_bytes()),
    ));
    Ok(out)
}

/// Remplit `dir` (vidé au préalable). Renvoie le nombre de fichiers sains écrits.
pub fn stage(dir: &Path, job: &Job, device: &str) -> Result<usize> {
    let plan = plan(job, device)?;
    clear(dir)?;
    let mut n = 0;
    for (rel, src) in plan {
        let dst = dir.join(&rel);
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p)?;
        }
        match src {
            Source::File(f) => {
                std::fs::copy(f, &dst)?;
                n += 1;
            }
            Source::Data(d) => std::fs::write(&dst, d)?,
        }
    }
    Ok(n)
}

/// Même arbre, dans un tar (mode serveur : la borne le déballe dans son dossier de préparation).
pub fn write_tar(path: &Path, job: &Job, device: &str) -> Result<()> {
    let mut b = tar::Builder::new(std::fs::File::create(path)?);
    for (rel, src) in plan(job, device)? {
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o644);
        match src {
            Source::File(f) => {
                let mut file = std::fs::File::open(f)?;
                h.set_size(file.metadata()?.len());
                b.append_data(&mut h, &rel, &mut file)?;
            }
            Source::Data(d) => {
                h.set_size(d.len() as u64);
                b.append_data(&mut h, &rel, d.as_slice())?;
            }
        }
    }
    b.finish()?;
    Ok(())
}

/// Déballe un tar produit par [`write_tar`] (venu du serveur) dans `dir`, vidé au préalable.
/// Seuls des fichiers ordinaires à chemin relatif sûr sont acceptés.
pub fn unpack_tar(tar_path: &Path, dir: &Path) -> Result<usize> {
    clear(dir)?;
    let mut ar = tar::Archive::new(std::fs::File::open(tar_path)?);
    let mut n = 0;
    for e in ar.entries()? {
        let mut e = e?;
        if e.header().entry_type() != tar::EntryType::Regular {
            bail!("entrée refusée dans l'archive du serveur");
        }
        let rel = safe_rel(&String::from_utf8_lossy(&e.path_bytes()))?;
        let dst = dir.join(&rel);
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::io::copy(&mut e, &mut std::fs::File::create(&dst)?)?;
        if !rel.to_string_lossy().starts_with("rapport-barbacane-") || rel.components().count() > 1
        {
            n += 1;
        }
    }
    Ok(n)
}

/// Vide un dossier sans le supprimer (il est déclaré dans la configuration usbsas).
pub fn clear(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if std::fs::symlink_metadata(&p)?.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// Chemin relatif sûr : uniquement des composants normaux.
fn safe_rel(p: &str) -> Result<PathBuf> {
    let path = Path::new(p);
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Normal(x) => out.push(x),
            Component::CurDir => (),
            _ => bail!("chemin de sortie refusé : {p}"),
        }
    }
    if out.as_os_str().is_empty() {
        bail!("chemin de sortie vide");
    }
    Ok(out)
}

/// Nom final sans collision de casse (exFAT) : un dossier déjà vu sous une
/// autre casse est réutilisé, un fichier en collision est renommé `nom (2).ext`.
/// `taken` : chemin en minuscules → (nom réel, est un dossier).
fn unique(rel: PathBuf, taken: &mut HashMap<String, (String, bool)>) -> PathBuf {
    let comps: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let mut cur = PathBuf::new();
    for (i, c) in comps.iter().enumerate() {
        let is_dir = i + 1 < comps.len();
        let key = |n: &str| cur.join(n).to_string_lossy().to_lowercase();
        let mut name = c.clone();
        match taken.get(&key(&name)) {
            Some((real, true)) if is_dir => name = real.clone(),
            Some(_) => {
                let (stem, ext) = match name.rfind('.') {
                    Some(k) if k > 0 && !is_dir => (name[..k].to_string(), name[k..].to_string()),
                    _ => (name.clone(), String::new()),
                };
                let mut k = 2;
                while taken.contains_key(&key(&name)) {
                    name = format!("{stem} ({k}){ext}");
                    k += 1;
                }
                taken.insert(key(&name), (name.clone(), is_dir));
            }
            None => {
                taken.insert(key(&name), (name.clone(), is_dir));
            }
        }
        cur.push(&name);
    }
    cur
}

/// Rapport JSON local, avec rotation.
pub fn save_report(dir: &Path, max: usize, job: &Job, s: &State) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let name = format!("{}-{}.json", s.started.strftime("%Y%m%d-%H%M%S"), job.id);
    std::fs::write(
        dir.join(name),
        serde_json::to_vec_pretty(&report::detailed(job, s))?,
    )?;
    let mut all: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    all.sort();
    if all.len() > max {
        for p in &all[..all.len() - max] {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_paths() {
        assert!(safe_rel("../x").is_err());
        assert!(safe_rel("/x").is_err());
        assert_eq!(safe_rel("a/./b").unwrap(), PathBuf::from("a/b"));
    }

    #[test]
    fn case_collisions() {
        let mut t = HashMap::new();
        assert_eq!(unique("a/X.txt".into(), &mut t), PathBuf::from("a/X.txt"));
        assert_eq!(
            unique("a/x.txt".into(), &mut t),
            PathBuf::from("a/x (2).txt")
        );
        assert_eq!(
            unique("a/x.txt".into(), &mut t),
            PathBuf::from("a/x (3).txt")
        );
        assert_eq!(unique("A/y".into(), &mut t), PathBuf::from("a/y"));
        assert_eq!(
            unique("a/X.txt/z".into(), &mut t),
            PathBuf::from("a/X.txt (2)/z")
        );
    }
}
