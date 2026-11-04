//! Extraction des conteneurs avec 7-Zip, dans un bac à sable bubblewrap
//! (aucun réseau, aucun accès au système de fichiers hors entrée/sortie,
//! limites mémoire, taille de fichier et durée).

use anyhow::{Result, bail};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Reconnaît un conteneur à ses octets magiques (jamais à l'extension).
pub fn sniff(path: &Path) -> Option<&'static str> {
    let mut head = vec![0u8; 0x8006];
    let n = std::fs::File::open(path)
        .and_then(|mut f| read_full(&mut f, &mut head))
        .ok()?;
    let h = &head[..n];
    let at = |off: usize, magic: &[u8]| {
        h.len() >= off + magic.len() && &h[off..off + magic.len()] == magic
    };
    Some(match () {
        _ if at(0, b"PK\x03\x04") || at(0, b"PK\x05\x06") => "zip",
        _ if at(0, b"7z\xBC\xAF\x27\x1C") => "7z",
        _ if at(0, b"Rar!\x1A\x07") => "rar",
        _ if at(0, b"\x1F\x8B") => "gzip",
        _ if at(0, b"BZh") => "bzip2",
        _ if at(0, b"\xFD7zXZ\x00") => "xz",
        _ if at(0, b"\x28\xB5\x2F\xFD") => "zstd",
        _ if at(0, b"\x1F\x9D") => "Z",
        _ if at(0, b"\x5D\x00\x00") => "lzma",
        _ if at(0, b"MSCF") => "cab",
        _ if at(0, b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") => "ole",
        _ if at(0, b"KDMV") || at(0, b"# Disk DescriptorFile") => "vmdk",
        _ if at(0, b"conectix") => "vhd",
        _ if at(0, b"vhdxfile") => "vhdx",
        _ if at(0, b"QFI\xFB") => "qcow",
        _ if at(0x40, b"\x7F\x10\xDA\xBE") => "vdi",
        _ if at(0, b"MSWIM\x00") => "wim",
        _ if at(0, b"070707") || at(0, b"070701") || at(0, b"070702") || at(0, b"\xC7\x71") => {
            "cpio"
        }
        _ if at(0, b"\x60\xEA") => "arj",
        _ if at(2, b"-lh") => "lzh",
        _ if at(0, b"ITSF") => "chm",
        _ if at(0, b"xar!") => "xar",
        _ if at(0, b"\xED\xAB\xEE\xDB") => "rpm",
        _ if at(0, b"!<arch>\n") => "ar",
        _ if at(0, b"hsqs") => "squashfs",
        _ if at(257, b"ustar") => "tar",
        _ if at(512, b"EFI PART") => "gpt",
        _ if at(3, b"NTFS    ") => "ntfs",
        _ if at(0x36, b"FAT1") || at(0x52, b"FAT32") => "fat",
        _ if at(0x438, b"\x53\xEF") => "ext",
        _ if at(0x8001, b"CD001") || at(0x8001, b"BEA01") => "iso",
        _ => return None,
    })
}

fn read_full(f: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

pub struct Extractor {
    pub seven_zip: PathBuf,
    pub bwrap: Option<PathBuf>,
    pub timeout: Duration,
    pub memory: u64,
}

/// Contenu annoncé par 7-Zip avant extraction.
#[derive(Debug, Default)]
pub struct Listing {
    pub encrypted: bool,
    pub entries: u64,
    pub unpacked_size: u64,
    /// Chemins absolus ou remontant d'un niveau (traversée de répertoire)
    pub dangerous_names: bool,
}

#[derive(Debug)]
pub enum Outcome {
    Ok,
    /// Mot de passe absent ou incorrect
    WrongPassword,
    /// Format non reconnu ou archive corrompue
    Failed(String),
}

impl Extractor {
    /// Liste le conteneur (détection du chiffrement, nombre et taille annoncés).
    pub fn list(&self, archive: &Path, password: Option<&str>) -> Result<(Outcome, Listing)> {
        let out = tempfile::tempdir()?;
        let (outcome, stdout) = self.run(archive, out.path(), "l", password, u64::MAX)?;
        let mut listing = Listing::default();
        let mut in_entries = false;
        for line in stdout.lines() {
            if line.starts_with("----------") {
                in_entries = true;
                continue;
            }
            if !in_entries {
                continue;
            }
            if let Some(p) = line.strip_prefix("Path = ") {
                listing.entries += 1;
                listing.dangerous_names |= dangerous(p);
            } else if let Some(s) = line.strip_prefix("Size = ") {
                listing.unpacked_size = listing
                    .unpacked_size
                    .saturating_add(s.trim().parse().unwrap_or(0));
            } else if line == "Encrypted = +" {
                listing.encrypted = true;
            }
        }
        if matches!(outcome, Outcome::WrongPassword) {
            listing.encrypted = true;
        }
        Ok((outcome, listing))
    }

    /// Extrait le conteneur dans `out`. `max_file_size` borne chaque fichier écrit.
    pub fn extract(
        &self,
        archive: &Path,
        out: &Path,
        password: Option<&str>,
        max_file_size: u64,
    ) -> Result<Outcome> {
        Ok(self.run(archive, out, "x", password, max_file_size)?.0)
    }

    fn run(
        &self,
        archive: &Path,
        out: &Path,
        verb: &str,
        password: Option<&str>,
        fsize: u64,
    ) -> Result<(Outcome, String)> {
        let name = sanitize_basename(archive);
        let inner = format!("/in/{name}");
        // Toujours fournir -p : 7-Zip ne doit jamais attendre une saisie.
        // shortcut: le mot de passe est visible dans /proc/<pid>/cmdline pour l'utilisateur de l'analyseur, acceptable sur borne mono-utilisateur ; passer par un fichier -p@ si mode serveur multi-locataires
        let pw = format!("-p{}", password.unwrap_or(""));
        let args7: Vec<&str> = match verb {
            "l" => vec!["l", "-slt", "-bd", "-y", &pw, &inner],
            _ => vec!["x", "-bd", "-y", &pw, "-o/out", &inner],
        };
        let mut cmd = match &self.bwrap {
            Some(bwrap) => {
                let mut c = Command::new(bwrap);
                c.args([
                    "--unshare-all",
                    "--die-with-parent",
                    "--new-session",
                    "--cap-drop",
                    "ALL",
                    "--clearenv",
                    "--setenv",
                    "PATH",
                    "/usr/bin",
                    "--setenv",
                    "LANG",
                    "C.UTF-8",
                    "--ro-bind",
                    "/usr",
                    "/usr",
                    "--symlink",
                    "usr/lib",
                    "/lib",
                    "--symlink",
                    "usr/lib64",
                    "/lib64",
                    "--symlink",
                    "usr/bin",
                    "/bin",
                    "--proc",
                    "/proc",
                    "--dev",
                    "/dev",
                    "--tmpfs",
                    "/tmp",
                ]);
                c.arg("--ro-bind").arg(archive).arg(&inner);
                c.arg("--bind")
                    .arg(out)
                    .arg("/out")
                    .args(["--chdir", "/out", "--"]);
                c.arg(&self.seven_zip).args(&args7);
                c
            }
            None => {
                // Tests sans bac à sable : on remplace les chemins internes.
                let mut c = Command::new(&self.seven_zip);
                let outarg = format!("-o{}", out.display());
                for a in &args7 {
                    match *a {
                        x if x == inner => c.arg(archive),
                        "-o/out" => c.arg(&outarg),
                        x => c.arg(x),
                    };
                }
                c
            }
        };
        let (memory, cpu) = (self.memory, self.timeout.as_secs() + 1);
        unsafe {
            cmd.pre_exec(move || {
                set_rlimit(libc::RLIMIT_AS, memory)?;
                set_rlimit(libc::RLIMIT_FSIZE, fsize)?;
                set_rlimit(libc::RLIMIT_CPU, cpu)?;
                set_rlimit(libc::RLIMIT_CORE, 0)?;
                Ok(())
            });
        }
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // Lecture des sorties dans des fils dédiés pour ne pas bloquer 7-Zip.
        let mut so = child.stdout.take().unwrap();
        let mut se = child.stderr.take().unwrap();
        let t_out = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = so.by_ref().take(16 << 20).read_to_end(&mut s);
            s
        });
        let t_err = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = se.by_ref().take(1 << 20).read_to_end(&mut s);
            s
        });
        let start = Instant::now();
        let status = loop {
            if let Some(st) = child.try_wait()? {
                break st;
            }
            if start.elapsed() > self.timeout {
                let _ = child.kill();
                let _ = child.wait();
                bail!("délai d'extraction dépassé");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
        let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
        let all = format!("{stdout}\n{stderr}");
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            bail!(match sig {
                libc::SIGXFSZ => "taille maximale décompressée dépassée".to_string(),
                libc::SIGXCPU => "délai d'extraction dépassé".to_string(),
                s => format!("extraction interrompue (signal {s})"),
            });
        }
        let outcome =
            if all.contains("Wrong password") || all.contains("Can not open encrypted archive") {
                Outcome::WrongPassword
            } else if status.code() == Some(0) {
                Outcome::Ok
            } else if all.contains("ERROR") && all.contains("Can not open the file as archive")
                || all.contains("Cannot open the file as archive")
            {
                Outcome::Failed("format non reconnu".into())
            } else if all.contains("Data Error")
                || all.contains("CRC Failed")
                || all.contains("Headers Error")
            {
                Outcome::Failed("archive corrompue".into())
            } else if all.contains("Can't allocate") || status.code() == Some(8) {
                bail!("mémoire maximale d'extraction dépassée")
            } else {
                Outcome::Failed(format!("erreur 7-Zip (code {:?})", status.code()))
            };
        Ok((outcome, stdout))
    }
}

fn dangerous(p: &str) -> bool {
    p.starts_with(['/', '\\'])
        || p.as_bytes().get(1) == Some(&b':')
        || p.split(['/', '\\']).any(|c| c == "..")
}

fn set_rlimit(res: libc::__rlimit_resource_t, val: u64) -> std::io::Result<()> {
    let lim = libc::rlimit {
        rlim_cur: val,
        rlim_max: val,
    };
    if unsafe { libc::setrlimit(res, &lim) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Nom de base sûr pour l'argument bwrap / 7-Zip (7-Zip s'en sert pour nommer
/// le contenu des flux gz/bz2/xz).
fn sanitize_basename(p: &Path) -> String {
    let n: String = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() || c == '/' { '_' } else { c })
        .take(200)
        .collect();
    if n.is_empty() || n.starts_with('-') || n == "." || n == ".." {
        format!("archive{n}")
    } else {
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename() {
        assert_eq!(sanitize_basename(Path::new("/a/b.zip")), "b.zip");
        assert_eq!(sanitize_basename(Path::new("/a/-rf")), "archive-rf");
        assert_eq!(sanitize_basename(Path::new("/a/x\ny")), "x_y");
        assert!(
            dangerous("../a")
                && dangerous("a/../b")
                && dangerous("/a")
                && dangerous("C:\\a")
                && dangerous("a\\..\\b")
        );
        assert!(!dangerous("a/b..c/d") && !dangerous("été.txt"));
    }
}
