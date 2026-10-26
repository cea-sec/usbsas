//! Dossier de préparation : arbre final des fichiers sains + rapports,
//! relu ensuite par usbsas (source `LocalDir`) pour écrire la clé de sortie.

use crate::job::{Job, State, Status};
use crate::report;
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Remplit `dir` (vidé au préalable). Renvoie le nombre de fichiers écrits.
pub fn stage(dir: &Path, job: &Job, device: &str) -> Result<usize> {
    let s = job.lock();
    if s.status != Status::Scanned {
        bail!("analyse non terminée");
    }
    clear(dir)?;
    // exFAT ignore la casse : deux noms ne différant que par la casse se renomment.
    let mut taken = HashMap::new();
    let mut n = 0;
    for e in s.entries.iter() {
        let Some(out) = &e.output else { continue };
        let rel = unique(safe_rel(out)?, &mut taken);
        let dst = dir.join(&rel);
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::copy(&e.local, &dst)?;
        n += 1;
    }
    let stamp = s.started.strftime("%Y%m%d-%H%M%S").to_string();
    std::fs::write(
        dir.join(format!("rapport-barbacane-{stamp}.txt")),
        report::text(job, &s, device),
    )?;
    std::fs::write(
        dir.join(format!("rapport-barbacane-{stamp}.html")),
        report::html(job, &s, device),
    )?;
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
