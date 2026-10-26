//! Bout en bout en mode mock : usbsas (binaires `--features mock` de la racine
//! du dépôt), barbacane-analyzer, clamd (base de test EICAR).
//! Clés simulées par des fichiers image (`USBSAS_MOCK_IN_DEV` / `USBSAS_MOCK_OUT_DEV`).

use barbacane_parcours::{self as parcours, Analyzer, Event, proto};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use usbsas_comm::{ComRqUsbsas, ProtoReqCommon};
use usbsas_fsrw::FSRead;
use usbsas_process::{ChildMngt, UsbsasChild, UsbsasChildSpawner};

const EICAR: &str = r"X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*";

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn sh(cmd: &str) {
    let st = Command::new("bash").args(["-euc", cmd]).status().unwrap();
    assert!(st.success(), "échec : {cmd}");
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "attente de {what} trop longue"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Image disque MBR, une partition à 1 Mio, système de fichiers `fs` (vfat ou ntfs) contenant `src`.
fn make_disk(img: &Path, src: &Path, fs_kind: &str) {
    let part = img.with_extension("part");
    let (src, img, part) = (src.display(), img.display(), part.display());
    let populate = match fs_kind {
        "vfat" => format!(
            "truncate -s 60M {part}; mkfs.vfat -F 32 -n CLE {part} >/dev/null; \
             (cd {src} && find . -mindepth 1 -type d | sort | sed 's|^\\./||' | while read -r d; do mmd -i {part} \"::$d\"; done; \
              find . -type f | sed 's|^\\./||' | while read -r f; do mcopy -i {part} \"$f\" \"::$f\"; done)"
        ),
        "ntfs" => format!(
            "truncate -s 60M {part}; mkntfs -F -Q -q -L CLE {part}; \
             (cd {src} && for f in *; do ntfscp -f {part} \"$f\" \"$f\" >/dev/null; done)"
        ),
        _ => unreachable!(),
    };
    let ptype = if fs_kind == "vfat" { "c" } else { "7" };
    sh(&format!(
        "{populate}; truncate -s 64M {img}; echo 'start=2048, type={ptype}' | sfdisk -q {img}; \
         dd if={part} of={img} bs=1M seek=1 conv=notrunc status=none; rm {part}"
    ));
}

/// Lecteur de la première partition d'une image (pour relire la clé de sortie).
struct Part {
    f: fs::File,
    start: u64,
}
impl Read for Part {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.f.read(b)
    }
}
impl Seek for Part {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let p = match pos {
            SeekFrom::Start(o) => self.f.seek(SeekFrom::Start(self.start + o))?,
            other => self.f.seek(other)?,
        };
        Ok(p - self.start)
    }
}

/// Fichiers (chemin → contenu) de la clé de sortie, en exFAT.
fn read_exfat(img: &Path) -> Vec<(String, Vec<u8>)> {
    let mut mbr = vec![0u8; 512];
    fs::File::open(img).unwrap().read_exact(&mut mbr).unwrap();
    let parts = usbsas_mbr::parse_partition_table(&mbr).unwrap();
    let start = parts[0].start_in_lba as u64 * 512;
    let mut fs = usbsas_fsrw::ff::ExFatFsReader::new(
        Part {
            f: fs::File::open(img).unwrap(),
            start,
        },
        512,
    )
    .unwrap();
    let mut out = Vec::new();
    let mut todo = vec!["/".to_string()];
    while let Some(dir) = todo.pop() {
        for fi in fs.read_dir(&dir).unwrap() {
            if fi.ftype == proto::common::FileType::Directory as i32 {
                todo.push(fi.path.clone());
            } else {
                let mut buf = vec![0u8; fi.size as usize];
                fs.read_file(&fi.path, &mut buf, 0, fi.size).unwrap();
                out.push((fi.path.trim_start_matches('/').to_string(), buf));
            }
        }
    }
    // FatFs garde le volume monté dans un état global : démontage obligatoire.
    Box::new(fs).unmount_fs().unwrap();
    out.sort();
    out
}

struct Env {
    dir: PathBuf,
    conf: PathBuf,
    analyzer: Analyzer,
    _clamd: Kill,
    _an: Kill,
}

fn setup(dir: &Path) -> Env {
    let bin = repo().join("target/release/usbsas-usbsas");
    assert!(
        bin.exists(),
        "{} absent : lancer barbacane/tests/phase1-mock.sh d'abord",
        bin.display()
    );
    let st = Command::new(env!("CARGO"))
        .args(["build", "-q", "-p", "barbacane-analyzer"])
        .status()
        .unwrap();
    assert!(st.success());
    let fixtures = repo().join("barbacane/tests/fixtures");

    let sock = dir.join("clamd.sock");
    fs::write(
        dir.join("clamd.conf"),
        format!(
            "Foreground true\nLocalSocket {}\nDatabaseDirectory {}\nTemporaryDirectory {}\nUser root\n",
            sock.display(),
            fixtures.join("clamdb").display(),
            dir.display()
        ),
    )
    .unwrap();
    let clamd = Kill(
        Command::new("/usr/sbin/clamd")
            .arg("--config-file")
            .arg(dir.join("clamd.conf"))
            .spawn()
            .unwrap(),
    );
    wait_for("clamd", || {
        std::os::unix::net::UnixStream::connect(&sock).is_ok()
    });

    let stage = dir.join("sortie");
    fs::create_dir_all(&stage).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let seven = ["/usr/bin/7zz", "/usr/bin/7z"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap();
    fs::write(
        dir.join("analyzer.toml"),
        format!(
            "listen = \"127.0.0.1:{port}\"\nwork_dir = \"{}\"\nclamd_socket = \"{}\"\nseven_zip = \"{seven}\"\n\
             yara_rules_dir = \"{}\"\nstage_dir = \"{}\"\nreports_dir = \"{}\"\n[limits]\npassword_timeout = 20\n",
            dir.join("work").display(),
            sock.display(),
            fixtures.join("yara").display(),
            stage.display(),
            dir.join("rapports").display()
        ),
    )
    .unwrap();
    let an = Kill(
        Command::new(repo().join("barbacane/target/debug/barbacane-analyzer"))
            .args(["-c", dir.join("analyzer.toml").to_str().unwrap()])
            .spawn()
            .unwrap(),
    );
    let analyzer = Analyzer::new(&format!("http://127.0.0.1:{port}"));
    wait_for("analyseur", || analyzer.status().is_ok());

    let conf = dir.join("usbsas.toml");
    fs::write(
        &conf,
        format!(
            r#"out_directory = "{out}"
[analyzer]
url = "http://127.0.0.1:{port}/api/scanbundle"
analyze_usb = true
analyze_net = false
analyze_cmd = true
[command]
description = "Analyse"
longdescr = "Analyse seule"
command_bin = "/bin/true"
command_args = []
[[src_local_dirs]]
path = "{stage}"
description = "Sortie"
longdescr = "Fichiers sains"
[report]
write_dest = false
[[filters]]
contain = ["autorun.inf"]
"#,
            out = dir.join("usbsas").display(),
            port = port,
            stage = stage.display()
        ),
    )
    .unwrap();
    Env {
        dir: dir.to_path_buf(),
        conf,
        analyzer,
        _clamd: clamd,
        _an: an,
    }
}

fn session(env: &Env) -> UsbsasChild<ComRqUsbsas> {
    UsbsasChildSpawner::new("usbsas-usbsas")
        .args(&["-c", env.conf.to_str().unwrap()])
        .spawn::<ComRqUsbsas>()
        .unwrap()
}

fn end(mut s: UsbsasChild<ComRqUsbsas>) {
    let _ = s.comm.end();
    let _ = s.wait();
}

/// Répond aux demandes de mot de passe : « secret » pour les archives `*-bon*`.
fn events(
    analyzer: Analyzer,
    uid_hint: &'static str,
) -> (mpsc::Sender<Event>, std::thread::JoinHandle<Vec<Event>>) {
    let (tx, rx) = mpsc::channel();
    let h = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for ev in rx {
            if let Event::Password(req) = &ev {
                let id = analyzer.current(uid_hint).unwrap().unwrap().id;
                let pw = req.archive.contains("-bon").then_some("secret");
                analyzer.answer(&id, pw).unwrap();
            }
            seen.push(ev);
        }
        seen
    });
    (tx, h)
}

/// Parcours complet. `wipe_same` : Blanchir (la sortie est la clé source).
fn run(env: &Env, wipe_same: bool) -> parcours::Analysis {
    let mut s = session(env);
    let devs = parcours::devices(&mut s.comm).unwrap();
    let (src_id, src) = devs.sources().next().cloned().unwrap();
    let (tx, h) = events(env.analyzer.clone(), "Tartempion");
    let an =
        parcours::analyse(&mut s.comm, &env.analyzer, src_id, devs.sink.unwrap(), &tx).unwrap();
    end(s);
    env.analyzer
        .stage(&an.report.id, &parcours::describe(&src))
        .unwrap();

    if wipe_same {
        let mut s = session(env);
        let devs = parcours::devices(&mut s.comm).unwrap();
        let dst = devs
            .same_as_dst(&src)
            .expect("clé source absente des destinations");
        parcours::wipe(&mut s.comm, dst, &tx).unwrap();
        end(s);
    }
    let mut s = session(env);
    let devs = parcours::devices(&mut s.comm).unwrap();
    let dst = devs.destinations().next().unwrap().0;
    parcours::write(
        &mut s.comm,
        devs.local_dir(env.dir.join("sortie").to_str().unwrap())
            .unwrap(),
        dst,
        &tx,
    )
    .unwrap();
    end(s);
    drop(tx);
    let evs = h.join().unwrap();
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::Progress {
            status: proto::common::Status::AllDone,
            ..
        }
    )));
    an
}

fn names(files: &[(String, Vec<u8>)]) -> Vec<&str> {
    files
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| !n.starts_with("rapport-barbacane-"))
        .collect()
}

fn zip_mixte(dir: &Path) {
    let z = dir.join("z");
    fs::create_dir_all(&z).unwrap();
    fs::write(z.join("eicar.com"), EICAR).unwrap();
    fs::write(z.join("sain.txt"), "sain dans l'archive\n").unwrap();
    sh(&format!(
        "cd {} && zip -q -X mixte.zip eicar.com sain.txt && zip -q -X -P secret chiffre-bon.zip sain.txt",
        z.display()
    ));
}

#[test]
fn bout_en_bout() {
    let tmp = tempfile::tempdir().unwrap();
    let env = setup(tmp.path());
    let d = &env.dir;
    zip_mixte(d);
    let input = d.join("in.img");
    let output = d.join("out.img");
    unsafe {
        std::env::set_var("USBSAS_MOCK_IN_DEV", &input);
        std::env::set_var("USBSAS_MOCK_OUT_DEV", &output);
    }

    // 1. Copier, source FAT
    let src = d.join("cle1");
    fs::create_dir_all(src.join("dossier")).unwrap();
    fs::write(src.join("sain.txt"), "Bonjour\n").unwrap();
    fs::write(src.join("eicar.com"), EICAR).unwrap();
    fs::write(src.join("autorun.inf"), "[autorun]\n").unwrap();
    fs::write(src.join("dossier/doc.txt"), "doc\n").unwrap();
    fs::copy(d.join("z/mixte.zip"), src.join("mixte.zip")).unwrap();
    fs::copy(d.join("z/chiffre-bon.zip"), src.join("chiffre-bon.zip")).unwrap();
    make_disk(&input, &src, "vfat");
    sh(&format!("truncate -s 64M {}", output.display()));
    let an = run(&env, false);
    let refused: Vec<_> = an.report.refused().map(|f| f.path.as_str()).collect();
    assert_eq!(refused, ["eicar.com", "mixte.zip/eicar.com"]);
    assert!(
        an.filtered
            .iter()
            .any(|f| f.eq_ignore_ascii_case("/autorun.inf")),
        "{:?}",
        an.filtered
    );
    let out = read_exfat(&output);
    assert_eq!(
        names(&out),
        [
            "chiffre-bon.zip",
            "dossier/doc.txt",
            "mixte.zip.extrait/sain.txt",
            "sain.txt"
        ],
        "copier"
    );
    assert!(out.iter().any(|(n, c)| n.ends_with(".txt")
        && n.starts_with("rapport-barbacane-")
        && String::from_utf8_lossy(c).contains("[REFUSÉ] eicar.com")));
    assert!(out.iter().all(|(_, c)| c != EICAR.as_bytes()));

    // 2. Source exFAT (la clé produite à l'étape 1), archive seule et sale
    fs::copy(&output, &input).unwrap();
    sh(&format!("rm {0}; truncate -s 64M {0}", output.display()));
    let an = run(&env, false);
    assert!(
        an.report.refused().next().is_none(),
        "aucun refus attendu en relisant une clé déjà assainie"
    );
    assert_eq!(
        names(&read_exfat(&output)),
        [
            "chiffre-bon.zip",
            "dossier/doc.txt",
            "mixte.zip.extrait/sain.txt",
            "sain.txt"
        ],
        "exFAT"
    );

    // 3. Uniquement une archive mixte : usbsas n'a rien à copier, le contenu sain extrait passe quand même
    let src = d.join("cle3");
    fs::create_dir_all(&src).unwrap();
    fs::copy(d.join("z/mixte.zip"), src.join("mixte.zip")).unwrap();
    make_disk(&input, &src, "vfat");
    sh(&format!("rm {0}; truncate -s 64M {0}", output.display()));
    run(&env, false);
    let out = read_exfat(&output);
    assert_eq!(names(&out), ["mixte.zip.extrait/sain.txt"], "archive seule");
    assert_eq!(
        out.iter()
            .find(|(n, _)| n == "mixte.zip.extrait/sain.txt")
            .unwrap()
            .1,
        b"sain dans l'archive\n"
    );

    // 4. Source NTFS
    let src = d.join("cle4");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("rapport.txt"), "ntfs\n").unwrap();
    fs::write(src.join("eicar.com"), EICAR).unwrap();
    make_disk(&input, &src, "ntfs");
    sh(&format!("rm {0}; truncate -s 64M {0}", output.display()));
    run(&env, false);
    assert_eq!(names(&read_exfat(&output)), ["rapport.txt"], "NTFS");

    // 5. Blanchir : la clé source elle-même est effacée puis réécrite
    let src = d.join("cle5");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("garder.txt"), "à garder\n").unwrap();
    fs::write(src.join("eicar.com"), EICAR).unwrap();
    make_disk(&input, &src, "vfat");
    unsafe { std::env::set_var("USBSAS_MOCK_OUT_DEV", &input) };
    run(&env, true);
    assert_eq!(names(&read_exfat(&input)), ["garder.txt"], "blanchir");
    let raw = fs::read(&input).unwrap();
    assert!(
        !raw.windows(EICAR.len()).any(|w| w == EICAR.as_bytes()),
        "EICAR encore présent sur la clé blanchie"
    );
}
