//! Mode serveur : un analyseur central (TLS + jetons) et deux bornes en mode
//! relais, utilisées exactement comme un analyseur local (protocole usbsas).

use serde_json::{Value, json};
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

const EICAR: &str = r"X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures")
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "attente de {what} trop longue"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn analyzer(conf: &Path) -> Kill {
    Kill(
        Command::new(env!("CARGO_BIN_EXE_barbacane-analyzer"))
            .args(["-c", conf.to_str().unwrap()])
            .spawn()
            .unwrap(),
    )
}

fn tar_of(files: &[(&str, &str)], dst: &Path) {
    let mut b = tar::Builder::new(std::fs::File::create(dst).unwrap());
    for (name, content) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(content.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, format!("data/{name}"), content.as_bytes())
            .unwrap();
    }
    b.finish().unwrap();
}

struct Borne {
    _p: Kill,
    url: String,
    stage: PathBuf,
}

fn borne(dir: &Path, central: &str, token: &str, ca: &Path) -> Borne {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("jeton"), token).unwrap();
    let p = port();
    let stage = dir.join("sortie");
    std::fs::write(
        dir.join("a.toml"),
        format!(
            "listen = \"127.0.0.1:{p}\"\nwork_dir = \"{w}\"\nstage_dir = \"{s}\"\nimport_dir = \"{i}\"\n\
             [relay]\nupstream = \"{central}\"\ntoken_file = \"{t}\"\nca_cert = \"{ca}\"\n",
            w = dir.join("work").display(),
            s = stage.display(),
            i = dir.join("import").display(),
            t = dir.join("jeton").display(),
            ca = ca.display()
        ),
    )
    .unwrap();
    let proc = analyzer(&dir.join("a.toml"));
    let url = format!("http://127.0.0.1:{p}");
    let http = reqwest::blocking::Client::new();
    wait_for("relais", || http.get(&url).send().is_ok());
    Borne {
        _p: proc,
        url,
        stage,
    }
}

/// Comme usbsas : POST du tar puis interrogation jusqu'à « scanned ».
fn scan(http: &reqwest::blocking::Client, b: &Borne, tar: &Path) -> (String, Value) {
    let r = http
        .post(format!("{}/api/scanbundle/Tartempion", b.url))
        .body(std::fs::read(tar).unwrap())
        .send()
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let id = r.json::<Value>().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut last = Value::Null;
    wait_for("analyse", || {
        last = http
            .get(format!("{}/api/scanbundle/Tartempion/{id}", b.url))
            .send()
            .unwrap()
            .json()
            .unwrap();
        last["status"] != "processing"
    });
    (id, last)
}

#[test]
fn analyseur_central_deux_bornes() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();

    // Certificat du serveur central (auto-signé, épinglé par les bornes)
    let ck =
        rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    std::fs::write(t.join("central.pem"), ck.cert.pem()).unwrap();
    std::fs::write(t.join("central.key"), ck.signing_key.serialize_pem()).unwrap();
    let (ta, tb) = ("a".repeat(40), "b".repeat(40));
    std::fs::write(
        t.join("jetons"),
        format!("# jeton borne\n{ta} borne-a\n{tb} borne-b\n"),
    )
    .unwrap();

    // clamd + serveur central
    let sock = t.join("clamd.sock");
    std::fs::write(
        t.join("clamd.conf"),
        format!(
            "Foreground true\nLocalSocket {}\nDatabaseDirectory {}\nUser root\n",
            sock.display(),
            fixtures().join("clamdb").display()
        ),
    )
    .unwrap();
    let _clamd = Kill(
        Command::new("/usr/sbin/clamd")
            .arg("--config-file")
            .arg(t.join("clamd.conf"))
            .spawn()
            .unwrap(),
    );
    wait_for("clamd", || {
        std::os::unix::net::UnixStream::connect(&sock).is_ok()
    });
    let seven = ["/usr/bin/7zz", "/usr/bin/7z"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap();
    let cp = port();
    std::fs::write(
        t.join("central.toml"),
        format!(
            "listen = \"127.0.0.1:{cp}\"\nwork_dir = \"{w}\"\nclamd_socket = \"{s}\"\nseven_zip = \"{seven}\"\n\
             stage_dir = \"{w}/inutile\"\nreports_dir = \"{w}/rapports\"\n\
             [server]\ntls_cert = \"{c}\"\ntls_key = \"{k}\"\ntokens_file = \"{j}\"\n",
            w = t.join("central").display(),
            s = sock.display(),
            c = t.join("central.pem").display(),
            k = t.join("central.key").display(),
            j = t.join("jetons").display()
        ),
    )
    .unwrap();
    let _central = analyzer(&t.join("central.toml"));
    let central = format!("https://localhost:{cp}");
    let pin = reqwest::Certificate::from_pem(ck.cert.pem().as_bytes()).unwrap();
    let https = reqwest::blocking::Client::builder()
        .tls_certs_only([pin])
        .build()
        .unwrap();
    wait_for("serveur central", || https.get(&central).send().is_ok());

    // Sans jeton (ou mauvais jeton) : refusé
    assert_eq!(
        https
            .get(format!("{central}/barbacane/v1/status"))
            .send()
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        https
            .get(format!("{central}/barbacane/v1/status"))
            .bearer_auth("x".repeat(40))
            .send()
            .unwrap()
            .status(),
        401
    );

    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap();
    let a = borne(&t.join("borne-a"), &central, &ta, &t.join("central.pem"));
    let b = borne(&t.join("borne-b"), &central, &tb, &t.join("central.pem"));
    let intrus = borne(
        &t.join("intrus"),
        &central,
        &"c".repeat(40),
        &t.join("central.pem"),
    );

    // Les deux bornes analysent en même temps, avec le même identifiant usbsas.
    let ta_tar = t.join("a.tar");
    let tb_tar = t.join("b.tar");
    tar_of(
        &[("rapport-a.txt", "borne A"), ("eicar.com", EICAR)],
        &ta_tar,
    );
    tar_of(&[("rapport-b.txt", "borne B")], &tb_tar);
    let (ida, ra) = scan(&http, &a, &ta_tar);
    let (idb, rb) = scan(&http, &b, &tb_tar);
    assert_eq!(ra["files"]["rapport-a.txt"]["status"], "CLEAN");
    assert_eq!(ra["files"]["eicar.com"]["status"], "DIRTY");
    assert_eq!(rb["files"]["rapport-b.txt"]["status"], "CLEAN");
    assert!(rb["files"].get("eicar.com").is_none());

    // Chaque borne voit sa propre analyse en cours, pas celle de l'autre.
    let cur_a: Value = http
        .get(format!("{}/barbacane/v1/users/Tartempion/current", a.url))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(cur_a["id"], ida.as_str());
    assert_eq!(
        http.get(format!("{}/barbacane/v1/scans/{ida}/report", b.url))
            .send()
            .unwrap()
            .status(),
        404
    );

    // Dossier de préparation de chaque borne, rempli depuis le serveur central.
    for (borne, id, fichier) in [(&a, &ida, "rapport-a.txt"), (&b, &idb, "rapport-b.txt")] {
        let r: Value = http
            .post(format!("{}/barbacane/v1/scans/{id}/stage", borne.url))
            .json(&json!({"device": "Clé test"}))
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(r["files"], 1, "{r}");
        assert!(borne.stage.join(fichier).is_file());
        assert!(!borne.stage.join("eicar.com").exists());
        let rapports = std::fs::read_dir(&borne.stage)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("rapport-barbacane-")
            })
            .count();
        assert_eq!(rapports, 2);
    }

    // Borne au jeton inconnu : l'analyse échoue (usbsas avorte le transfert).
    let r = http
        .post(format!("{}/api/scanbundle/Tartempion", intrus.url))
        .body(std::fs::read(&tb_tar).unwrap())
        .send()
        .unwrap();
    assert_eq!(r.status(), 401);
}
