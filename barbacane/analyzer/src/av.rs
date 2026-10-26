//! Moteurs d'analyse : ClamAV (clamd, INSTREAM) et YARA (yara-x).

use anyhow::{Context, Result, bail};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// Résultat d'un moteur sur un fichier.
pub enum Detection {
    Clean,
    Found { engine: &'static str, name: String },
}

pub struct Engines {
    clamd_socket: PathBuf,
    yara: Option<yara_x::Rules>,
}

impl Engines {
    pub fn new(clamd_socket: PathBuf, yara_dir: Option<&Path>) -> Result<Self> {
        let yara = match yara_dir {
            Some(dir) if dir.is_dir() => Some(compile_yara(dir)?),
            _ => None,
        };
        Ok(Engines { clamd_socket, yara })
    }

    /// Version ClamAV : (moteur, version base, date base).
    pub fn clamav_version(&self) -> Result<(String, String, String)> {
        let rep = self.clamd(b"zVERSION\0", None)?;
        let mut it = rep.split('/');
        Ok((
            it.next().unwrap_or_default().to_string(),
            it.next().unwrap_or_default().to_string(),
            it.next().unwrap_or_default().to_string(),
        ))
    }

    pub fn yara_enabled(&self) -> bool {
        self.yara.is_some()
    }

    /// Analyse un fichier. Toute erreur de moteur remonte : l'appelant refuse le fichier.
    pub fn scan(&self, path: &Path) -> Result<Detection> {
        let rep = self.clamd(b"zINSTREAM\0", Some(path))?;
        // Réponse : "stream: OK" ou "stream: <signature> FOUND" ou "... ERROR"
        let rep = rep.strip_prefix("stream: ").unwrap_or(&rep);
        if let Some(name) = rep.strip_suffix(" FOUND") {
            return Ok(Detection::Found {
                engine: "ClamAV",
                name: name.to_string(),
            });
        }
        if rep != "OK" {
            bail!("clamd : {rep}");
        }
        if let Some(rules) = &self.yara {
            let mut scanner = yara_x::Scanner::new(rules);
            let results = scanner.scan_file(path).context("yara")?;
            if let Some(rule) = results.matching_rules().next() {
                return Ok(Detection::Found {
                    engine: "YARA",
                    name: rule.identifier().to_string(),
                });
            }
        }
        Ok(Detection::Clean)
    }

    fn clamd(&self, cmd: &[u8], stream: Option<&Path>) -> Result<String> {
        let mut sock = UnixStream::connect(&self.clamd_socket)
            .with_context(|| format!("connexion à clamd ({})", self.clamd_socket.display()))?;
        sock.write_all(cmd)?;
        if let Some(path) = stream {
            let mut f = File::open(path)?;
            let mut buf = vec![0u8; 1 << 16];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                sock.write_all(&(n as u32).to_be_bytes())?;
                sock.write_all(&buf[..n])?;
            }
            sock.write_all(&0u32.to_be_bytes())?;
        }
        let mut rep = Vec::new();
        sock.read_to_end(&mut rep)?;
        Ok(String::from_utf8_lossy(&rep)
            .trim_end_matches(['\0', '\n'])
            .to_string())
    }
}

fn compile_yara(dir: &Path) -> Result<yara_x::Rules> {
    let mut compiler = yara_x::Compiler::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("yar" | "yara")))
        .collect();
    entries.sort();
    let mut ok = 0;
    for p in entries {
        let src = match std::fs::read_to_string(&p) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("règles YARA {} ignorées : {e}", p.display());
                continue;
            }
        };
        // Un espace de noms par fichier : une règle invalide n'écarte que son fichier.
        compiler.new_namespace(&p.file_stem().unwrap_or_default().to_string_lossy());
        match compiler.add_source(src.as_str()) {
            Ok(_) => ok += 1,
            Err(e) => log::warn!("règles YARA {} ignorées : {e}", p.display()),
        }
    }
    log::info!("{ok} fichier(s) de règles YARA chargé(s)");
    Ok(compiler.build())
}

#[cfg(test)]
mod tests {
    #[test]
    fn regle_invalide_ignoree() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("a.yar"),
            "rule a { strings: $x = \"ABC\" condition: $x }",
        )
        .unwrap();
        std::fs::write(
            d.path().join("b.yar"),
            "rule b { condition: filename == \"x\" }",
        )
        .unwrap();
        let rules = super::compile_yara(d.path()).unwrap();
        let mut sc = yara_x::Scanner::new(&rules);
        assert_eq!(sc.scan(b"xxABCxx").unwrap().matching_rules().count(), 1);
    }
}
