//! Tests d'intégration de l'analyseur : jeu d'archives de
//! `barbacane/tests/fixtures/make.sh`, vrai clamd (base de test EICAR),
//! vrai 7-Zip dans bubblewrap.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures")
}

fn which(names: &[&str]) -> PathBuf {
    for n in names {
        for d in ["/usr/bin", "/usr/sbin", "/usr/local/bin", "/bin"] {
            let p = Path::new(d).join(n);
            if p.exists() {
                return p;
            }
        }
    }
    panic!("outil introuvable : {names:?}");
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

fn start_clamd(dir: &Path) -> (Kill, PathBuf) {
    let sock = dir.join("clamd.sock");
    let conf = dir.join("clamd.conf");
    std::fs::write(
        &conf,
        format!(
            "Foreground true\nLocalSocket {}\nDatabaseDirectory {}\nTemporaryDirectory {}\nStreamMaxLength 100M\nUser root\n",
            sock.display(),
            fixtures().join("clamdb").display(),
            dir.display()
        ),
    )
    .unwrap();
    let child = Command::new(which(&["clamd"]))
        .arg("--config-file")
        .arg(&conf)
        .spawn()
        .expect("clamd");
    wait_for("clamd", || {
        std::os::unix::net::UnixStream::connect(&sock).is_ok()
    });
    (Kill(child), sock)
}

struct Analyzer {
    _clamd: Kill,
    _proc: Kill,
    url: String,
    stage: PathBuf,
    http: reqwest::blocking::Client,
}

fn start_analyzer(dir: &Path, port: u16) -> Analyzer {
    let (clamd, sock) = start_clamd(dir);
    let stage = dir.join("sortie");
    let conf = dir.join("analyzer.toml");
    std::fs::write(
        &conf,
        format!(
            r#"listen = "127.0.0.1:{port}"
work_dir = "{w}"
clamd_socket = "{s}"
yara_rules_dir = "{y}"
seven_zip = "{z}"
bwrap = "{b}"
stage_dir = "{st}"
reports_dir = "{r}"
import_dir = "{i}"
[limits]
max_files = 50
max_total_size = 20000000
password_timeout = 2
"#,
            w = dir.join("work").display(),
            s = sock.display(),
            y = fixtures().join("yara").display(),
            z = which(&["7zz", "7z"]).display(),
            b = which(&["bwrap"]).display(),
            st = stage.display(),
            r = dir.join("rapports").display(),
            i = dir.join("import").display(),
        ),
    )
    .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_barbacane-analyzer"))
        .args(["-c", conf.to_str().unwrap()])
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .unwrap();
    wait_for("analyseur", || http.get(&url).send().is_ok());
    Analyzer {
        _clamd: clamd,
        _proc: Kill(child),
        url,
        stage,
        http,
    }
}

fn make_tar(src: &Path, dst: &Path) {
    let mut b = tar::Builder::new(std::fs::File::create(dst).unwrap());
    let mut names: Vec<_> = std::fs::read_dir(src)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    for n in names {
        b.append_path_with_name(src.join(&n), Path::new("data").join(&n))
            .unwrap();
    }
    // Fichier de métadonnées ajouté par usbsas, jamais analysé
    let mut h = tar::Header::new_gnu();
    h.set_size(2);
    h.set_cksum();
    b.append_data(&mut h, "config.json", &b"{}"[..]).unwrap();
    b.finish().unwrap();
}

/// Lance une analyse en répondant aux demandes de mot de passe selon `answers`.
fn scan(a: &Analyzer, tar: &Path, answers: &HashMap<&str, Value>) -> String {
    let rep: Value = a
        .http
        .post(format!("{}/api/scanbundle/borne1", a.url))
        .body(std::fs::read(tar).unwrap())
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(rep["status"], "uploaded");
    let id = rep["id"].as_str().unwrap().to_string();
    let mut asked: Vec<String> = Vec::new();
    wait_for("fin d'analyse", || {
        let p: Value = a
            .http
            .get(format!("{}/barbacane/v1/users/borne1/current", a.url))
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(p["id"], id.as_str());
        if let Some(req) = p["password_request"].as_object() {
            let archive = req["archive"].as_str().unwrap().to_string();
            let key = format!("{archive}#{}", req["attempt"]);
            if !asked.contains(&key) {
                asked.push(key);
                if let Some(ans) = answers.get(archive.as_str()) {
                    let r = a
                        .http
                        .post(format!("{}/barbacane/v1/scans/{id}/password", a.url))
                        .json(ans)
                        .send()
                        .unwrap();
                    assert!(r.status().is_success());
                }
            }
        }
        // Côté usbsas, rien ne change : « processing » jusqu'à la fin.
        let u: Value = a
            .http
            .get(format!("{}/api/scanbundle/borne1/{id}", a.url))
            .send()
            .unwrap()
            .json()
            .unwrap();
        u["status"] != "processing"
    });
    id
}

#[test]
fn archives() {
    let tmp = tempfile::tempdir().unwrap();
    let cle = tmp.path().join("cle");
    let st = Command::new(fixtures().join("make.sh"))
        .arg(&cle)
        .status()
        .unwrap();
    assert!(st.success());
    let tar = tmp.path().join("bundle.tar");
    make_tar(&cle, &tar);

    let a = start_analyzer(tmp.path(), 18042);
    let answers: HashMap<&str, Value> = HashMap::from([
        ("chiffre-bon.zip", json!({"password": "secret"})),
        ("chiffre-entetes.7z", json!({"password": "secret"})),
        ("chiffre-faux.zip", json!({"password": "mauvais"})),
        ("chiffre-abandon.zip", json!({"abandon": true})),
    ]);
    let id = scan(&a, &tar, &answers);

    // Rapport au format usbsas
    let u: Value = a
        .http
        .get(format!("{}/api/scanbundle/borne1/{id}", a.url))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(u["status"], "scanned");
    assert_eq!(u["version"], 2);
    assert!(
        u["antivirus"]["ClamAV"]["version"]
            .as_str()
            .unwrap()
            .starts_with("ClamAV")
    );
    let st = |f: &str| {
        u["files"][f]["status"]
            .as_str()
            .unwrap_or_else(|| panic!("{f} absent du rapport"))
            .to_string()
    };
    for f in [
        "sain.txt",
        "vide.txt",
        "propre.zip",
        "chiffre-bon.zip",
        "chiffre-entetes.7z",
    ] {
        assert_eq!(st(f), "CLEAN", "{f}");
    }
    for f in [
        "eicar.com",
        "yara.txt",
        "mixte.zip",
        "zip_dans_7z.7z",
        "eicar.iso",
        "mixte.tar.gz",
        "disque.vmdk",
        "mixte.cab",
        "paquet.msi",
        "chiffre-faux.zip",
        "chiffre-abandon.zip",
        "chiffre-sans.zip",
        "bombe-ratio.zip",
        "bombe-recursive.zip",
        "trop-de-fichiers.zip",
        "trop-gros.zip",
        "noms-pieges.zip",
    ] {
        assert_eq!(st(f), "DIRTY", "{f}");
    }
    assert_eq!(u["files"]["sain.txt"]["sha256"].as_str().unwrap().len(), 64);

    // Rapport détaillé
    let d: Value = a
        .http
        .get(format!("{}/barbacane/v1/scans/{id}/report", a.url))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let files = d["files"].as_array().unwrap();
    let entry = |p: &str| {
        files
            .iter()
            .find(|e| e["path"] == p)
            .unwrap_or_else(|| panic!("{p} absent du rapport détaillé"))
    };
    let reason = |p: &str| entry(p)["reason"].as_str().unwrap().to_string();
    assert!(reason("eicar.com").contains("ClamAV"));
    assert_eq!(entry("yara.txt")["engine"], "YARA");
    assert_eq!(
        entry("mixte.zip/sain.txt")["output"],
        "mixte.zip.extrait/sain.txt"
    );
    assert!(entry("mixte.zip/eicar.com")["output"].is_null());
    assert_eq!(
        entry("zip_dans_7z.7z/inner.zip/sain.txt")["output"],
        "zip_dans_7z.7z.extrait/inner.zip.extrait/sain.txt"
    );
    assert_eq!(
        entry("zip_dans_7z.7z/autre.txt")["output"],
        "zip_dans_7z.7z.extrait/autre.txt"
    );
    assert!(entry("eicar.iso/eicar.com")["verdict"] == "DIRTY");
    assert_eq!(entry("eicar.iso/sain.txt")["verdict"], "CLEAN");
    assert_eq!(entry("disque.vmdk/sain.txt")["verdict"], "CLEAN");
    assert_eq!(entry("mixte.cab/sain.txt")["verdict"], "CLEAN");
    assert_eq!(entry("chiffre-bon.zip")["output"], "chiffre-bon.zip");
    assert!(
        reason("chiffre-faux.zip").contains("incorrect"),
        "{}",
        reason("chiffre-faux.zip")
    );
    assert!(reason("chiffre-abandon.zip").contains("abandonnée"));
    assert!(reason("chiffre-sans.zip").contains("aucun mot de passe"));
    assert!(reason("bombe-ratio.zip").contains("compression"));
    assert!(
        reason("bombe-recursive.zip").contains("imbriquée"),
        "{}",
        reason("bombe-recursive.zip")
    );
    assert!(reason("trop-de-fichiers.zip").contains("trop de fichiers"));
    assert!(reason("trop-gros.zip").contains("taille"));
    // Conteneurs refusés en bloc : rien n'en est extrait.
    for c in [
        "bombe-ratio.zip",
        "trop-de-fichiers.zip",
        "trop-gros.zip",
        "chiffre-faux.zip",
    ] {
        assert!(
            !files.iter().any(|e| e["container"] == c),
            "{c} ne doit rien extraire"
        );
    }
    // Noms piégés : aucune sortie ne remonte d'un niveau.
    for e in files {
        if let Some(o) = e["output"].as_str() {
            assert!(
                !o.split('/').any(|c| c == ".." || c.is_empty()),
                "sortie dangereuse : {o}"
            );
        }
    }

    // Dossier de préparation
    let r: Value = a
        .http
        .post(format!("{}/barbacane/v1/scans/{id}/stage", a.url))
        .json(&json!({"device": "Test 1234"}))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert!(r["files"].as_u64().unwrap() > 10, "{r}");
    let s = &a.stage;
    assert!(s.join("sain.txt").is_file());
    assert!(s.join("mixte.zip.extrait/sain.txt").is_file());
    assert!(!s.join("eicar.com").exists());
    assert!(!s.join("mixte.zip").exists());
    assert!(
        s.join("noms-pieges.zip.extrait/dossier/Casse.txt")
            .is_file()
    );
    assert!(
        s.join("noms-pieges.zip.extrait/dossier/casse (2).txt")
            .is_file()
    );
    let txt: Vec<_> = std::fs::read_dir(s)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("rapport-barbacane-"))
        .collect();
    assert_eq!(txt.len(), 2, "{txt:?}");
    let rapport =
        std::fs::read_to_string(s.join(txt.iter().find(|n| n.ends_with(".txt")).unwrap())).unwrap();
    assert!(rapport.contains("[REFUSÉ] eicar.com"));
    assert!(
        !rapport.contains("[REFUSÉ] mixte.zip\n"),
        "une archive dont le contenu sain est récupéré n'est pas un refus"
    );
    // Le fichier d'EICAR n'a jamais été écrit hors du répertoire de travail.
    assert!(walk(s).iter().all(|p| std::fs::read(p).unwrap()
        != b"X5O!P%@AP[4\\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*"));
}

#[test]
fn clamd_absent_refuse_tout() {
    let tmp = tempfile::tempdir().unwrap();
    let cle = tmp.path().join("cle");
    std::fs::create_dir(&cle).unwrap();
    std::fs::write(cle.join("a.txt"), "x").unwrap();
    let tar = tmp.path().join("b.tar");
    make_tar(&cle, &tar);
    let mut a = start_analyzer(tmp.path(), 18043);
    drop(std::mem::replace(
        &mut a._clamd,
        Kill(Command::new("true").spawn().unwrap()),
    ));
    let id = scan(&a, &tar, &HashMap::new());
    let u: Value = a
        .http
        .get(format!("{}/api/scanbundle/borne1/{id}", a.url))
        .send()
        .unwrap()
        .json()
        .unwrap();
    // usbsas avorte le transfert sur tout statut autre que « scanned ».
    assert_eq!(u["status"], "error");
}

#[test]
fn import_cle_admin() {
    let tmp = tempfile::tempdir().unwrap();
    let cle = tmp.path().join("cle");
    std::fs::create_dir(&cle).unwrap();
    std::fs::write(cle.join("barbacane-maj-000002.bcu"), b"(\xb5/\xfd paquet").unwrap();
    std::fs::write(cle.join("barbacane-maj-000002.bcu.minisig"), "sig").unwrap();
    std::fs::write(cle.join("autre.txt"), "x").unwrap();
    let tar = tmp.path().join("b.tar");
    make_tar(&cle, &tar);
    let a = start_analyzer(tmp.path(), 18044);
    // Sans armement : rien n'est déposé.
    scan(&a, &tar, &HashMap::new());
    assert!(!tmp.path().join("import").exists());
    let r = a
        .http
        .post(format!("{}/barbacane/v1/import/arm", a.url))
        .send()
        .unwrap();
    assert!(r.status().is_success());
    let id = scan(&a, &tar, &HashMap::new());
    let imp = tmp.path().join("import");
    assert!(
        imp.join("barbacane-maj-000002.bcu").is_file()
            && imp.join("barbacane-maj-000002.bcu.minisig").is_file()
    );
    assert!(!imp.join("autre.txt").exists());
    let u: Value = a
        .http
        .get(format!("{}/api/scanbundle/borne1/{id}", a.url))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        u["files"]["barbacane-maj-000002.bcu"]["status"], "DIRTY",
        "jamais copié vers une clé de sortie"
    );
    assert_eq!(u["files"]["autre.txt"]["status"], "CLEAN");
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(d).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            v.extend(walk(&p));
        } else {
            v.push(p);
        }
    }
    v
}
