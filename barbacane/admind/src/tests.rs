use super::*;
use sha2::{Digest, Sha256};
use std::process::Command;

fn ctx(dir: &Path) -> Ctx {
    let conf = Config {
        socket: dir.join("admin.sock"),
        state_dir: dir.join("etat"),
        analyzer_config: dir.join("analyzer.toml"),
        analyzer_url: "http://127.0.0.1:9".into(),
        import_dir: dir.join("import"),
        export_dir: dir.join("export"),
        reports_dir: dir.join("rapports"),
        clamav_dir: dir.join("clamav"),
        yara_dir: dir.join("yara"),
        update_pubkey: dir.join("maj.pub"),
        dry_run: true,
        ..Config::default()
    };
    for d in [
        &conf.state_dir,
        &conf.import_dir,
        &conf.reports_dir,
        &conf.clamav_dir,
        &conf.yara_dir,
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(
        &conf.analyzer_config,
        "listen = \"127.0.0.1:8042\"\n[limits]\nmax_depth = 5\n",
    )
    .unwrap();
    Ctx {
        conf,
        session: Mutex::new(None),
        op: Mutex::new(()),
    }
}

fn req(c: &Ctx, v: Value) -> Result<Value> {
    c.handle(v)
}

#[test]
fn pin_et_verrouillage() {
    let t = tempfile::tempdir().unwrap();
    let c = ctx(t.path());
    assert!(c.set_pin("123").is_err(), "PIN trop court");
    c.set_pin("246813").unwrap();
    assert!(
        !std::fs::read_to_string(c.path("pin"))
            .unwrap()
            .contains("246813"),
        "PIN stocké en clair"
    );
    assert!(
        req(&c, json!({"op": "set_pin", "pin": "111111"})).is_err(),
        "opération sans jeton"
    );
    let tok = req(&c, json!({"op": "login", "pin": "246813"})).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    req(&c, json!({"op": "set_pin", "pin": "135790", "token": tok})).unwrap();
    assert!(
        req(
            &c,
            json!({"op": "set_pin", "pin": "135790", "token": "faux"})
        )
        .is_err()
    );
    for i in 1..MAX_FAILURES {
        let e = req(&c, json!({"op": "login", "pin": "000000"}))
            .unwrap_err()
            .to_string();
        assert!(e.contains(&format!("{} essai", MAX_FAILURES - i)), "{e}");
    }
    let e = req(&c, json!({"op": "login", "pin": "000000"}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("verrouillé 15 min"), "{e}");
    // Même le bon PIN est refusé pendant le verrouillage.
    let e = req(&c, json!({"op": "login", "pin": "135790"}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("verrouillé"), "{e}");
}

fn login(c: &Ctx) -> String {
    c.set_pin("246813").unwrap();
    req(c, json!({"op": "login", "pin": "246813"})).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn enrolement_et_limites() {
    let t = tempfile::tempdir().unwrap();
    let c = ctx(t.path());
    let tok = login(&c);
    assert!(
        req(
            &c,
            json!({"op": "set_enrolled_mode", "enabled": true, "token": tok})
        )
        .is_err(),
        "aucune clé enrôlée"
    );
    let key = json!({"op": "enroll_add", "vendorid": 4660, "productid": 22136, "serial": "ABC123", "description": "Clé A", "token": tok});
    req(&c, key.clone()).unwrap();
    req(&c, key).unwrap();
    req(
        &c,
        json!({"op": "set_enrolled_mode", "enabled": true, "token": tok}),
    )
    .unwrap();
    let st = c.status();
    assert_eq!(st["enrolled_mode"], true);
    assert_eq!(
        st["enrolled"].as_array().unwrap().len(),
        1,
        "pas de doublon"
    );
    assert!(
        req(
            &c,
            json!({"op": "enroll_add", "vendorid": 1, "productid": 1, "serial": "", "token": tok})
        )
        .is_err()
    );
    req(&c, json!({"op": "enroll_remove", "key": {"vendorid": 4660, "productid": 22136, "serial": "ABC123"}, "token": tok})).unwrap();
    assert_eq!(c.status()["enrolled"].as_array().unwrap().len(), 0);

    assert!(req(&c, json!({"op": "set_limits", "limits": {"max_depth": 0, "max_files": 10, "max_total_size": 1}, "token": tok})).is_err());
    req(&c, json!({"op": "set_limits", "limits": {"max_depth": 7, "max_files": 20000, "max_total_size": 8589934592u64}, "token": tok})).unwrap();
    let l = c.limits();
    assert_eq!(
        (l["max_depth"].as_i64(), l["max_files"].as_i64()),
        (Some(7), Some(20000))
    );
    // Le reste de la configuration de l'analyseur est conservé.
    assert!(
        std::fs::read_to_string(&c.conf.analyzer_config)
            .unwrap()
            .contains("listen")
    );
}

#[test]
fn export() {
    let t = tempfile::tempdir().unwrap();
    let c = ctx(t.path());
    std::fs::write(c.conf.reports_dir.join("r1.json"), "{}").unwrap();
    let tok = login(&c);
    req(&c, json!({"op": "prepare_export", "token": tok})).unwrap();
    let d: Vec<_> = std::fs::read_dir(&c.conf.export_dir).unwrap().collect();
    assert_eq!(d.len(), 1);
    let root = d.into_iter().next().unwrap().unwrap().path();
    assert!(
        root.join("rapports/r1.json").is_file()
            && root.join("journal.txt").is_file()
            && root.join("etat.json").is_file()
    );
}

/// Construit un paquet signé ; `tamper` modifie le contenu après signature.
fn paquet(
    dir: &Path,
    key: &Path,
    version: u64,
    extra: Option<(&str, &str)>,
    bad_hash: bool,
) -> PathBuf {
    paquet_x(dir, key, version, extra, bad_hash, None)
}

/// `cle` : nouvelle clé publique embarquée (rotation), listée dans le manifeste.
fn paquet_x(
    dir: &Path,
    key: &Path,
    version: u64,
    extra: Option<(&str, &str)>,
    bad_hash: bool,
    cle: Option<&Path>,
) -> PathBuf {
    let src = dir.join(format!(
        "src-{version}-{}",
        key.file_name().unwrap().to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(&src);
    std::fs::create_dir_all(src.join("clamav")).unwrap();
    std::fs::create_dir_all(src.join("yara")).unwrap();
    std::fs::write(
        src.join("clamav/daily.cld"),
        format!("ClamAV-VDB:01 Jan 2026 00-00 +0000:{version}:"),
    )
    .unwrap();
    std::fs::write(src.join("yara/regles.yar"), "rule r { condition: false }").unwrap();
    let mut rels = vec!["clamav/daily.cld", "yara/regles.yar"];
    if let Some(c) = cle {
        std::fs::create_dir_all(src.join("cles")).unwrap();
        std::fs::copy(c, src.join("cles/maj.pub")).unwrap();
        rels.push("cles/maj.pub");
    }
    let mut files = serde_json::Map::new();
    for rel in rels {
        let mut h = Sha256::new();
        h.update(std::fs::read(src.join(rel)).unwrap());
        let mut hx = hex::encode(h.finalize());
        if bad_hash {
            hx = "0".repeat(64);
        }
        files.insert(rel.into(), json!(hx));
    }
    if let Some((rel, content)) = extra {
        std::fs::write(src.join(rel), content).unwrap();
    }
    std::fs::write(
        src.join("manifest.json"),
        serde_json::to_vec(
            &json!({"format": 1, "version": version, "date": "2026-10-26", "files": files}),
        )
        .unwrap(),
    )
    .unwrap();
    let pkg = dir.join(format!("barbacane-maj-{version:06}.bcu"));
    let f = std::fs::File::create(&pkg).unwrap();
    let mut b = tar::Builder::new(zstd::Encoder::new(f, 3).unwrap().auto_finish());
    b.append_dir_all(".", &src).unwrap();
    b.into_inner().unwrap();
    let st = Command::new("minisign")
        .args(["-S", "-s"])
        .arg(key)
        .arg("-m")
        .arg(&pkg)
        .args(["-t", "barbacane"])
        .status()
        .unwrap();
    assert!(st.success());
    pkg
}

fn deposer(c: &Ctx, pkg: &Path) {
    for e in std::fs::read_dir(&c.conf.import_dir).unwrap() {
        std::fs::remove_file(e.unwrap().path()).unwrap();
    }
    let name = pkg.file_name().unwrap().to_string_lossy().to_string();
    std::fs::copy(pkg, c.conf.import_dir.join(&name)).unwrap();
    std::fs::copy(
        format!("{}.minisig", pkg.display()),
        c.conf.import_dir.join(format!("{name}.minisig")),
    )
    .unwrap();
}

#[test]
fn mises_a_jour() {
    let t = tempfile::tempdir().unwrap();
    let c = ctx(t.path());
    let tok = login(&c);
    let k = t.path().join("k");
    std::fs::create_dir_all(&k).unwrap();
    let st = Command::new("minisign")
        .args(["-G", "-W", "-p"])
        .arg(&c.conf.update_pubkey)
        .arg("-s")
        .arg(k.join("maj.key"))
        .status()
        .unwrap();
    assert!(st.success());
    let st = Command::new("minisign")
        .args(["-G", "-W", "-p"])
        .arg(k.join("autre.pub"))
        .arg("-s")
        .arg(k.join("autre.key"))
        .status()
        .unwrap();
    assert!(st.success());
    let apply = |c: &Ctx| req(c, json!({"op": "apply_update", "token": tok}));
    std::fs::write(c.conf.clamav_dir.join("daily.cld"), "ancienne base").unwrap();

    // Valide
    let p2 = paquet(t.path(), &k.join("maj.key"), 2, None, false);
    deposer(&c, &p2);
    let r = apply(&c).unwrap();
    assert_eq!(r["version"], 2);
    assert!(
        std::fs::read_to_string(c.conf.clamav_dir.join("daily.cld"))
            .unwrap()
            .contains(":2:")
    );
    assert!(c.conf.yara_dir.join("regles.yar").is_file());
    assert_eq!(c.update_version(), 2);
    assert!(c.status()["signatures_age_days"].as_u64().is_some());

    // Version plus ancienne ou identique : refusée
    let p1 = paquet(t.path(), &k.join("maj.key"), 1, None, false);
    deposer(&c, &p1);
    assert!(
        apply(&c)
            .unwrap_err()
            .to_string()
            .contains("retour arrière")
    );
    deposer(&c, &p2);
    assert!(
        apply(&c)
            .unwrap_err()
            .to_string()
            .contains("retour arrière")
    );

    // Signature d'une autre clé : refusée
    let pa = paquet(t.path(), &k.join("autre.key"), 3, None, false);
    deposer(&c, &pa);
    assert!(
        apply(&c)
            .unwrap_err()
            .to_string()
            .contains("signature invalide")
    );

    // Paquet modifié après signature : refusé
    let p3 = paquet(t.path(), &k.join("maj.key"), 3, None, false);
    deposer(&c, &p3);
    let target = c.conf.import_dir.join(p3.file_name().unwrap());
    let mut bytes = std::fs::read(&target).unwrap();
    let n = bytes.len();
    bytes[n / 2] ^= 0xff;
    std::fs::write(&target, bytes).unwrap();
    assert!(
        apply(&c)
            .unwrap_err()
            .to_string()
            .contains("signature invalide")
    );

    // Signé mais manifeste incohérent (empreinte ou fichier non listé) : refusé
    let p4 = paquet(t.path(), &k.join("maj.key"), 4, None, true);
    deposer(&c, &p4);
    assert!(
        apply(&c)
            .unwrap_err()
            .to_string()
            .contains("empreinte incorrecte")
    );
    let p5 = paquet(
        t.path(),
        &k.join("maj.key"),
        5,
        Some(("clamav/intrus.cvd", "x")),
        false,
    );
    deposer(&c, &p5);
    assert!(apply(&c).unwrap_err().to_string().contains("non listé"));

    // Signature absente
    deposer(&c, &p3);
    std::fs::remove_file(c.conf.import_dir.join(format!(
        "{}.minisig",
        p3.file_name().unwrap().to_string_lossy()
    )))
    .unwrap();
    assert!(apply(&c).unwrap_err().to_string().contains("signature"));

    // Rien n'a bougé depuis la version 2.
    assert_eq!(c.update_version(), 2);
    assert!(
        std::fs::read_to_string(c.conf.clamav_dir.join("daily.cld"))
            .unwrap()
            .contains(":2:")
    );
}

#[test]
fn retour_arriere_si_echec() {
    let t = tempfile::tempdir().unwrap();
    let mut c = ctx(t.path());
    let tok = login(&c);
    let k = t.path().join("k");
    std::fs::create_dir_all(&k).unwrap();
    Command::new("minisign")
        .args(["-G", "-W", "-p"])
        .arg(&c.conf.update_pubkey)
        .arg("-s")
        .arg(k.join("maj.key"))
        .status()
        .unwrap();
    std::fs::write(c.conf.clamav_dir.join("daily.cld"), "ancienne base").unwrap();
    let p = paquet(t.path(), &k.join("maj.key"), 2, None, false);
    deposer(&c, &p);
    // systemctl échoue (exécution réelle d'une commande absente)
    c.conf.dry_run = false;
    let e = req(&c, json!({"op": "apply_update", "token": tok}));
    // systemctl peut exister sur la machine de test : on n'exige le retour arrière que s'il a échoué.
    if let Err(e) = e {
        assert!(e.to_string().contains("restaurées"), "{e}");
        assert_eq!(
            std::fs::read_to_string(c.conf.clamav_dir.join("daily.cld")).unwrap(),
            "ancienne base"
        );
        assert_eq!(c.update_version(), 0);
    }
}

#[test]
fn rotation_de_cle() {
    let t = tempfile::tempdir().unwrap();
    let c = ctx(t.path());
    let tok = login(&c);
    let k = t.path().join("k");
    std::fs::create_dir_all(&k).unwrap();
    for n in ["maj", "nouvelle"] {
        let pubk = if n == "maj" {
            c.conf.update_pubkey.clone()
        } else {
            k.join("nouvelle.pub")
        };
        let st = Command::new("minisign")
            .args(["-G", "-W", "-p"])
            .arg(pubk)
            .arg("-s")
            .arg(k.join(format!("{n}.key")))
            .status()
            .unwrap();
        assert!(st.success());
    }
    let apply = |c: &Ctx| req(c, json!({"op": "apply_update", "token": tok}));
    // Paquet signé par l'ancienne clé, apportant la nouvelle clé publique.
    let p = paquet_x(
        t.path(),
        &k.join("maj.key"),
        2,
        None,
        false,
        Some(&k.join("nouvelle.pub")),
    );
    deposer(&c, &p);
    assert!(
        apply(&c).unwrap()["report"]
            .as_str()
            .unwrap()
            .contains("clé de signature")
    );
    assert_eq!(
        std::fs::read(&c.conf.update_pubkey).unwrap(),
        std::fs::read(k.join("nouvelle.pub")).unwrap()
    );
    // L'ancienne clé n'est plus acceptée, la nouvelle l'est.
    deposer(&c, &paquet(t.path(), &k.join("maj.key"), 3, None, false));
    assert!(
        apply(&c)
            .unwrap_err()
            .to_string()
            .contains("signature invalide")
    );
    deposer(
        &c,
        &paquet(t.path(), &k.join("nouvelle.key"), 3, None, false),
    );
    apply(&c).unwrap();
    assert_eq!(c.update_version(), 3);
}
