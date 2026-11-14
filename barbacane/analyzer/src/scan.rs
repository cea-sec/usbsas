//! Moteur d'analyse d'un transfert : dépaquetage du tar usbsas, analyse de
//! chaque fichier, extraction récursive des conteneurs, verdicts.

use crate::av::{Detection, Engines};
use crate::config::Limits;
use crate::extract::{Extractor, Outcome, sniff};
use crate::job::{Answer, Job};
use anyhow::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Clean,
    Dirty,
}

/// Une ligne du rapport détaillé.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// Chemin affiché : chemin sur la clé, suivi du chemin dans l'archive (`a.zip/b/c.txt`)
    pub path: String,
    /// Conteneur parent (chemin affiché), absent pour un fichier de la clé
    pub container: Option<String>,
    pub size: u64,
    pub sha256: String,
    /// Type réel (type MIME détecté sur le contenu)
    pub file_type: String,
    pub verdict: Verdict,
    /// Raison lisible (en français)
    pub reason: String,
    pub engine: Option<String>,
    pub signature: Option<String>,
    /// Chemin sur la clé de sortie si le fichier est copié
    pub output: Option<String>,
    #[serde(skip)]
    pub local: PathBuf,
}

struct Node {
    entry: Entry,
    children: Vec<Node>,
    /// Limite anti-bombe dépassée : tout le conteneur racine est refusé
    limit: bool,
}

/// Budget partagé par tout le transfert.
struct Budget {
    files: u64,
    size: u64,
}

pub struct Scanner<'a> {
    pub engines: &'a Engines,
    pub extractor: &'a Extractor,
    pub limits: &'a Limits,
    pub job: &'a Job,
    pub work: PathBuf,
    /// Lecture de la clé admin : les paquets `*.bcu` / `*.bcu.minisig` sont déposés ici, sans analyse
    /// (leur signature est vérifiée par barbacane-admind).
    pub import: Option<PathBuf>,
}

const TAR_DATA_DIR: &str = "data/";
const CLEAN_REASON: &str = "aucune menace détectée";

impl Scanner<'_> {
    /// Analyse le tar usbsas et renvoie le rapport à plat (ordre du tar, puis contenu des archives).
    pub fn run(&self, tar_path: &Path) -> Result<Vec<Entry>> {
        let src = self.work.join("src");
        std::fs::create_dir_all(&src)?;
        // Dépaquetage sous des noms internes : les noms de la clé ne touchent jamais le disque.
        let mut files = Vec::new();
        let mut ar = tar::Archive::new(std::fs::File::open(tar_path)?);
        for (i, e) in ar.entries()?.enumerate() {
            let mut e = e?;
            // Fichiers creux : tar les relit en clair.
            if !matches!(
                e.header().entry_type(),
                tar::EntryType::Regular | tar::EntryType::GNUSparse | tar::EntryType::Continuous
            ) {
                if e.header().entry_type() != tar::EntryType::Directory {
                    log::warn!(
                        "entrée tar ignorée ({:?}) : {}",
                        e.header().entry_type(),
                        String::from_utf8_lossy(&e.path_bytes())
                    );
                }
                continue;
            }
            // Comme usbsas-analyzer-server : seuls les fichiers sous `data/` sont à analyser
            // (usbsas ajoute `config.json` à côté).
            let raw = String::from_utf8_lossy(&e.path_bytes())
                .trim_start_matches('/')
                .to_string();
            let Some(path) = raw.strip_prefix(TAR_DATA_DIR).map(str::to_string) else {
                continue;
            };
            let local = src.join(i.to_string());
            std::io::copy(&mut e, &mut std::fs::File::create(&local)?)?;
            files.push((path, local));
        }
        self.job.set_progress(0, files.len());
        let mut budget = Budget { files: 0, size: 0 };
        let mut out = Vec::new();
        for (i, (path, local)) in files.into_iter().enumerate() {
            if let Some(dir) = &self.import
                && !path.contains('/')
                && (path.ends_with(".bcu") || path.ends_with(".bcu.minisig"))
            {
                std::fs::create_dir_all(dir)?;
                std::fs::copy(&local, dir.join(&path))?;
                let (size, sha256) = hash(&local).unwrap_or((0, String::new()));
                out.push(Entry {
                    path,
                    container: None,
                    size,
                    sha256,
                    file_type: "paquet de mise à jour".into(),
                    verdict: Verdict::Dirty,
                    reason:
                        "paquet de mise à jour : transmis au service d'administration, non copié"
                            .into(),
                    engine: None,
                    signature: None,
                    output: None,
                    local,
                });
                continue;
            }
            let mut node = self.node(&local, path.clone(), None, 0, &mut budget);
            assign_output(&mut node, path);
            flatten(node, &mut out);
            self.job.set_progress(i + 1, out.len());
        }
        Ok(out)
    }

    fn node(
        &self,
        local: &Path,
        path: String,
        container: Option<String>,
        depth: u32,
        budget: &mut Budget,
    ) -> Node {
        let (size, sha256) = hash(local).unwrap_or((0, String::new()));
        let file_type = infer::get_from_path(local)
            .ok()
            .flatten()
            .map(|t| t.mime_type().to_string())
            .unwrap_or_else(|| {
                if size == 0 {
                    "vide".into()
                } else {
                    "application/octet-stream".into()
                }
            });
        let mut entry = Entry {
            path,
            container,
            size,
            sha256,
            file_type,
            verdict: Verdict::Clean,
            reason: CLEAN_REASON.into(),
            engine: None,
            signature: None,
            output: None,
            local: local.to_path_buf(),
        };
        match self.engines.scan(local) {
            Ok(Detection::Clean) => (),
            Ok(Detection::Found { engine, name }) => {
                entry.verdict = Verdict::Dirty;
                entry.reason = format!("menace détectée par {engine} : {name}");
                entry.engine = Some(engine.into());
                entry.signature = Some(name);
            }
            Err(err) => {
                log::error!("analyse de {} : {err:#}", entry.path);
                entry.verdict = Verdict::Dirty;
                entry.reason = format!("analyse impossible ({err})");
            }
        }
        let mut node = Node {
            entry,
            children: Vec::new(),
            limit: false,
        };
        if let Some(kind) = sniff(local) {
            self.container(&mut node, kind, depth, budget);
        }
        node
    }

    /// Ouvre un conteneur ; en cas de refus, tout le conteneur est refusé.
    fn container(&self, node: &mut Node, kind: &str, depth: u32, budget: &mut Budget) {
        let refuse = |node: &mut Node, reason: String| {
            node.entry.verdict = Verdict::Dirty;
            node.entry.reason = reason;
            node.children.clear();
        };
        let over_limit = |node: &mut Node, reason: String| {
            refuse(node, reason);
            node.limit = true;
        };
        if depth >= self.limits.max_depth {
            return over_limit(
                node,
                format!(
                    "archive trop imbriquée (profondeur maximale {})",
                    self.limits.max_depth
                ),
            );
        }
        let local = node.entry.local.clone();
        let (outcome, mut listing) = match self.extractor.list(&local, None) {
            Ok(r) => r,
            Err(err) => return refuse(node, format!("archive illisible : {err}")),
        };
        let mut password = None;
        match outcome {
            Outcome::Failed(why) => {
                // Signatures faibles : souvent un faux positif sur un fichier ordinaire.
                if !matches!(kind, "lzma" | "Z" | "arj" | "lzh" | "cpio" | "ext") {
                    refuse(
                        node,
                        format!("archive {kind} illisible ou corrompue ({why})"),
                    );
                }
                return;
            }
            Outcome::WrongPassword | Outcome::Ok if listing.encrypted => {
                match self.ask_password(&node.entry.path, &local) {
                    Ok((pw, l)) => {
                        password = Some(pw);
                        listing = l;
                    }
                    Err(reason) => return refuse(node, reason),
                }
            }
            Outcome::Ok | Outcome::WrongPassword => (),
        }
        let l = self.limits;
        if budget.files + listing.entries > l.max_files {
            return over_limit(
                node,
                format!(
                    "trop de fichiers dans les archives (maximum {})",
                    l.max_files
                ),
            );
        }
        if budget.size.saturating_add(listing.unpacked_size) > l.max_total_size {
            return over_limit(
                node,
                format!(
                    "taille décompressée trop grande (maximum {})",
                    human(l.max_total_size)
                ),
            );
        }
        if listing.unpacked_size / node.entry.size.max(1) > l.max_ratio {
            return over_limit(
                node,
                format!(
                    "taux de compression suspect (bombe de décompression, plus de {} pour 1)",
                    l.max_ratio
                ),
            );
        }
        let out = self.work.join("x").join(uuid::Uuid::new_v4().to_string());
        if let Err(err) = std::fs::create_dir_all(&out) {
            return refuse(node, format!("erreur interne : {err}"));
        }
        let remaining = l.max_total_size - budget.size;
        match self.extractor.extract(
            &local,
            &out,
            password.as_deref().map(|s| s.as_str()),
            remaining,
        ) {
            Ok(Outcome::Ok) => (),
            Ok(Outcome::WrongPassword) => {
                return refuse(node, "archive chiffrée : mot de passe incorrect".into());
            }
            Ok(Outcome::Failed(why)) => {
                return refuse(node, format!("extraction impossible ({why})"));
            }
            Err(err) => return over_limit(node, format!("extraction refusée : {err}")),
        }
        // Inventaire réel (les tailles annoncées peuvent mentir).
        let mut found = Vec::new();
        if let Err(err) = walk(&out, &out, &mut found) {
            return refuse(node, format!("extraction illisible : {err}"));
        }
        let real_size: u64 = found.iter().map(|f| f.size).sum();
        if budget.files + found.len() as u64 > l.max_files {
            return over_limit(
                node,
                format!(
                    "trop de fichiers dans les archives (maximum {})",
                    l.max_files
                ),
            );
        }
        if budget.size + real_size > l.max_total_size {
            return over_limit(
                node,
                format!(
                    "taille décompressée trop grande (maximum {})",
                    human(l.max_total_size)
                ),
            );
        }
        if real_size / node.entry.size.max(1) > l.max_ratio {
            return over_limit(
                node,
                format!(
                    "taux de compression suspect (bombe de décompression, plus de {} pour 1)",
                    l.max_ratio
                ),
            );
        }
        budget.files += found.len() as u64;
        budget.size += real_size;
        let parent = node.entry.path.clone();
        for f in found {
            let path = format!("{parent}/{}", f.rel);
            let child = match f.problem {
                Some(why) => Node {
                    entry: Entry {
                        path,
                        container: Some(parent.clone()),
                        size: f.size,
                        sha256: String::new(),
                        file_type: "spécial".into(),
                        verdict: Verdict::Dirty,
                        reason: why.into(),
                        engine: None,
                        signature: None,
                        output: None,
                        local: f.local,
                    },
                    children: Vec::new(),
                    limit: false,
                },
                None => self.node(&f.local, path, Some(parent.clone()), depth + 1, budget),
            };
            node.children.push(child);
        }
        if let Some(c) = node.children.iter().find(|c| c.limit) {
            let reason = c.entry.reason.clone();
            return over_limit(node, reason);
        }
        if listing.dangerous_names && node.entry.verdict == Verdict::Clean {
            node.entry.verdict = Verdict::Dirty;
            node.entry.reason =
                "archive contenant des chemins dangereux (les fichiers sains sont extraits)".into();
        }
        if node
            .children
            .iter()
            .any(|c| c.entry.verdict == Verdict::Dirty)
        {
            node.entry.verdict = Verdict::Dirty;
            if node.entry.engine.is_none() {
                node.entry.reason =
                    "archive contenant des fichiers refusés (les fichiers sains sont extraits)"
                        .into();
            }
        }
    }

    fn ask_password(
        &self,
        path: &str,
        local: &Path,
    ) -> std::result::Result<(zeroize::Zeroizing<String>, crate::extract::Listing), String> {
        for attempt in 1..=self.limits.password_attempts {
            match self.job.ask_password(
                path,
                attempt,
                self.limits.password_attempts,
                self.limits.password_timeout,
            ) {
                Answer::Password(pw) => match self.extractor.list(local, Some(&pw)) {
                    Ok((Outcome::Ok, l)) => return Ok((pw, l)),
                    Ok((Outcome::WrongPassword, _)) => continue,
                    Ok((Outcome::Failed(why), _)) => {
                        return Err(format!("archive chiffrée illisible ({why})"));
                    }
                    Err(err) => return Err(format!("archive chiffrée illisible : {err}")),
                },
                Answer::Abandon => {
                    return Err("archive chiffrée : saisie du mot de passe abandonnée".into());
                }
                Answer::Timeout => return Err("archive chiffrée : aucun mot de passe saisi".into()),
            }
        }
        Err("archive chiffrée : mot de passe incorrect".into())
    }
}

/// Décide où chaque fichier est écrit : un nœud sain est copié tel quel ;
/// le contenu sain d'une archive refusée va dans `<archive>.extrait/`.
fn assign_output(node: &mut Node, out: String) {
    if node.entry.verdict == Verdict::Clean {
        node.entry.output = Some(out);
        return;
    }
    let prefix = format!("{}/", node.entry.path);
    for c in &mut node.children {
        let rel = c
            .entry
            .path
            .strip_prefix(&prefix)
            .unwrap_or(&c.entry.path)
            .to_string();
        assign_output(c, format!("{out}.extrait/{rel}"));
    }
}

fn flatten(node: Node, out: &mut Vec<Entry>) {
    out.push(node.entry);
    for c in node.children {
        flatten(c, out);
    }
}

struct Found {
    rel: String,
    local: PathBuf,
    size: u64,
    problem: Option<&'static str>,
}

/// Parcourt une extraction sans jamais suivre de lien.
fn walk(root: &Path, dir: &Path, out: &mut Vec<Found>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let md = std::fs::symlink_metadata(&p)?;
        let rel_raw = p.strip_prefix(root).unwrap_or(&p);
        let rel = rel_raw.to_string_lossy().into_owned();
        if md.is_dir() {
            walk(root, &p, out)?;
            continue;
        }
        let problem = if md.file_type().is_symlink() {
            Some("lien symbolique : non copié")
        } else if !md.is_file() {
            Some("fichier spécial : non copié")
        } else if rel_raw.to_str().is_none() {
            Some("nom de fichier invalide (encodage) : non copié")
        } else if rel_raw.iter().any(|c| c.len() > 255) || rel.len() > 1024 {
            Some("nom de fichier trop long : non copié")
        } else if rel.chars().any(|c| c.is_control()) {
            Some("nom de fichier contenant des caractères de contrôle : non copié")
        } else {
            None
        };
        out.push(Found {
            rel,
            local: p,
            size: md.len(),
            problem,
        });
    }
    Ok(())
}

fn hash(p: &Path) -> std::io::Result<(u64, String)> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        h.update(&buf[..n]);
    }
    Ok((size, hex::encode(h.finalize())))
}

pub fn human(n: u64) -> String {
    const U: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} o")
    } else {
        format!("{v:.1} {}", U[i])
    }
}
