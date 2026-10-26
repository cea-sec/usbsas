//! barbacane-admind : service d'administration de la borne (root).
//!
//! Socket Unix, une requête JSON par ligne. Le PIN (haché Argon2id) est
//! vérifié ici ; après connexion, un jeton de session (15 min d'inactivité)
//! autorise les opérations. Verrouillage progressif après 5 échecs.
//!
//! `barbacane-admind --set-pin` lit un PIN sur l'entrée standard (installation).

mod update;

use anyhow::{Context, Result, anyhow, bail};
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub socket: PathBuf,
    /// Groupe autorisé à se connecter (session kiosque)
    pub socket_group: String,
    pub state_dir: PathBuf,
    pub analyzer_config: PathBuf,
    pub analyzer_url: String,
    pub import_dir: PathBuf,
    pub export_dir: PathBuf,
    pub reports_dir: PathBuf,
    pub clamav_dir: PathBuf,
    pub yara_dir: PathBuf,
    pub update_pubkey: PathBuf,
    /// Tests : n'exécute aucune commande système (systemctl, apt-get, reboot…)
    pub dry_run: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            socket: "/run/barbacane/admin.sock".into(),
            socket_group: "barbacane-kiosk".into(),
            state_dir: "/var/lib/barbacane/admin".into(),
            analyzer_config: "/etc/barbacane/analyzer.toml".into(),
            analyzer_url: "http://127.0.0.1:8042".into(),
            import_dir: "/var/lib/barbacane/import".into(),
            export_dir: "/var/lib/barbacane/export".into(),
            reports_dir: "/var/lib/barbacane/rapports".into(),
            clamav_dir: "/var/lib/clamav".into(),
            yara_dir: "/var/lib/barbacane/yara".into(),
            update_pubkey: "/etc/barbacane/maj.pub".into(),
            dry_run: false,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Lock {
    failures: u32,
    /// Fin du verrouillage (horodatage Unix)
    until: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Enrolled {
    mode: bool,
    keys: Vec<Value>,
}

pub struct Ctx {
    pub conf: Config,
    session: Mutex<Option<(String, Instant)>>,
    /// Une seule opération à la fois
    op: Mutex<()>,
}

const SESSION: Duration = Duration::from_secs(15 * 60);
const MAX_FAILURES: u32 = 5;

fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

impl Ctx {
    fn path(&self, name: &str) -> PathBuf {
        self.conf.state_dir.join(name)
    }

    fn read_json<T: for<'de> Deserialize<'de> + Default>(&self, name: &str) -> T {
        std::fs::read(self.path(name))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Écriture atomique (fichier temporaire puis renommage).
    fn write_json(&self, name: &str, v: &impl Serialize) -> Result<()> {
        std::fs::create_dir_all(&self.conf.state_dir)?;
        let tmp = self.path(&format!(".{name}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
        std::fs::rename(tmp, self.path(name))?;
        Ok(())
    }

    pub fn update_version(&self) -> u64 {
        std::fs::read_to_string(self.path("version-maj"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn set_update_version(&self, v: u64) -> Result<()> {
        std::fs::write(self.path("version-maj"), v.to_string())?;
        Ok(())
    }

    pub fn run(&self, argv: &[&str]) -> Result<String> {
        log::info!("commande : {}", argv.join(" "));
        if self.conf.dry_run {
            return Ok(String::new());
        }
        let out = std::process::Command::new(argv[0])
            .args(&argv[1..])
            .env("DEBIAN_FRONTEND", "noninteractive")
            .output()?;
        if !out.status.success() {
            bail!(
                "{} : {}",
                argv[0],
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn set_pin(&self, pin: &str) -> Result<()> {
        if pin.len() < 6 || !pin.chars().all(|c| c.is_ascii_digit()) {
            bail!("le PIN doit comporter au moins 6 chiffres");
        }
        let h = argon2::Argon2::default()
            .hash_password(pin.as_bytes())
            .map_err(|e| anyhow!("{e}"))?
            .to_string();
        std::fs::create_dir_all(&self.conf.state_dir)?;
        let tmp = self.path(".pin.tmp");
        std::fs::write(&tmp, h)?;
        std::fs::rename(tmp, self.path("pin"))?;
        self.write_json("verrou.json", &Lock::default())
    }

    fn login(&self, pin: &str) -> Result<String> {
        let mut lock: Lock = self.read_json("verrou.json");
        if lock.until > now() {
            bail!(
                "trop d'échecs : mode administrateur verrouillé pendant encore {} min",
                (lock.until - now()) / 60 + 1
            );
        }
        let stored = std::fs::read_to_string(self.path("pin"))
            .context("aucun PIN configuré (barbacane-admind --set-pin)")?;
        let ok = PasswordHash::new(stored.trim())
            .map(|h| {
                argon2::Argon2::default()
                    .verify_password(pin.as_bytes(), &h)
                    .is_ok()
            })
            .unwrap_or(false);
        if !ok {
            lock.failures += 1;
            let msg = if lock.failures >= MAX_FAILURES {
                // Verrouillage croissant : 15 min, 30 min, 60 min…
                let minutes = 15i64 << (lock.failures - MAX_FAILURES).min(6);
                lock.until = now() + minutes * 60;
                format!("PIN incorrect : mode administrateur verrouillé {minutes} min")
            } else {
                format!(
                    "PIN incorrect ({} essai(s) restant(s) avant verrouillage)",
                    MAX_FAILURES - lock.failures
                )
            };
            self.write_json("verrou.json", &lock)?;
            log::warn!("échec de connexion administrateur ({})", lock.failures);
            bail!(msg);
        }
        self.write_json("verrou.json", &Lock::default())?;
        let mut b = [0u8; 32];
        getrandom::fill(&mut b).map_err(|e| anyhow!("{e}"))?;
        let token = hex::encode(b);
        *self.session.lock().unwrap() = Some((token.clone(), Instant::now()));
        log::info!("connexion administrateur");
        Ok(token)
    }

    fn check(&self, req: &Value) -> Result<()> {
        let mut s = self.session.lock().unwrap();
        match (&*s, req["token"].as_str()) {
            (Some((t, at)), Some(given)) if t == given && at.elapsed() < SESSION => {
                *s = Some((t.clone(), Instant::now()));
                Ok(())
            }
            _ => bail!("session administrateur expirée : saisissez à nouveau le PIN"),
        }
    }

    fn signatures_age_days(&self) -> Option<u64> {
        // Date de la base `daily` (en-tête « ClamAV-VDB:14 Oct 2026 07-35 +0000:… »), sinon date du fichier.
        let mut best: Option<i64> = None;
        for name in ["daily.cld", "daily.cvd", "main.cld", "main.cvd"] {
            let p = self.conf.clamav_dir.join(name);
            let Ok(mut f) = std::fs::File::open(&p) else {
                continue;
            };
            let mut head = [0u8; 512];
            let _ = std::io::Read::read(&mut f, &mut head);
            let h = String::from_utf8_lossy(&head);
            let t = h
                .strip_prefix("ClamAV-VDB:")
                .and_then(|r| r.split(':').next())
                .and_then(|d| jiff::fmt::strtime::parse("%d %b %Y %H-%M %z", d).ok())
                .and_then(|t| t.to_timestamp().ok())
                .map(|t| t.as_second())
                .or_else(|| {
                    std::fs::metadata(&p)
                        .ok()?
                        .modified()
                        .ok()?
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs() as i64)
                });
            if let Some(t) = t {
                best = Some(best.map_or(t, |b: i64| b.max(t)));
            }
            if name.starts_with("daily") && best.is_some() {
                break;
            }
        }
        best.map(|t| ((now() - t).max(0) / 86400) as u64)
    }

    fn limits(&self) -> Value {
        let t: toml::Table = std::fs::read_to_string(&self.conf.analyzer_config)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        let l = t
            .get("limits")
            .and_then(|v| v.as_table())
            .cloned()
            .unwrap_or_default();
        let get = |k: &str, d: i64| l.get(k).and_then(|v| v.as_integer()).unwrap_or(d);
        json!({
            "max_depth": get("max_depth", 5),
            "max_files": get("max_files", 10000),
            "max_total_size": get("max_total_size", 4 * 1024 * 1024 * 1024),
        })
    }

    fn status(&self) -> Value {
        let en: Enrolled = self.read_json("enroles.json");
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let dir = std::ffi::CString::new(self.conf.state_dir.as_os_str().as_encoded_bytes())
            .unwrap_or_default();
        let free = if unsafe { libc::statvfs(dir.as_ptr(), &mut st) } == 0 {
            format!(
                "{:.1} Go",
                (st.f_bavail as f64 * st.f_frsize as f64) / 1073741824.0
            )
        } else {
            "?".into()
        };
        let count = |d: &Path| std::fs::read_dir(d).map(|r| r.count()).unwrap_or(0);
        let clamav = ureq::get(format!("{}/barbacane/v1/status", self.conf.analyzer_url))
            .call()
            .ok()
            .and_then(|mut r| r.body_mut().read_json::<Value>().ok())
            .and_then(|v| v["clamav"]["version"].as_str().map(str::to_string))
            .unwrap_or_else(|| "indisponible".into());
        let usbsas = if self.conf.dry_run {
            "?".into()
        } else {
            self.run(&["dpkg-query", "-W", "-f=${Version}", "usbsas-server"])
                .unwrap_or_else(|_| "?".into())
        };
        json!({
            "enrolled_mode": en.mode,
            "enrolled": en.keys,
            "signatures_age_days": self.signatures_age_days(),
            "versions": {"barbacane": env!("CARGO_PKG_VERSION"), "usbsas": usbsas, "clamav": clamav},
            "yara_rules": count(&self.conf.yara_dir),
            "update_version": self.update_version(),
            "disk_free": free,
            "reports": count(&self.conf.reports_dir),
            "limits": self.limits(),
        })
    }

    fn set_limits(&self, l: &Value) -> Result<()> {
        let get = |k: &str, min: i64, max: i64| -> Result<i64> {
            let v = l[k].as_i64().with_context(|| format!("{k} manquant"))?;
            if !(min..=max).contains(&v) {
                bail!("{k} hors limites ({min} à {max})");
            }
            Ok(v)
        };
        let (d, f, s) = (
            get("max_depth", 1, 20)?,
            get("max_files", 100, 1_000_000)?,
            get("max_total_size", 64 << 20, 64 << 30)?,
        );
        let mut t: toml::Table = toml::from_str(
            &std::fs::read_to_string(&self.conf.analyzer_config).unwrap_or_default(),
        )?;
        let limits = t
            .entry("limits")
            .or_insert(toml::Value::Table(Default::default()));
        let lt = limits.as_table_mut().context("[limits] invalide")?;
        lt.insert("max_depth".into(), d.into());
        lt.insert("max_files".into(), f.into());
        lt.insert("max_total_size".into(), s.into());
        let tmp = self.conf.analyzer_config.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string(&t)?)?;
        std::fs::rename(tmp, &self.conf.analyzer_config)?;
        self.run(&["systemctl", "restart", "barbacane-analyzer"])?;
        log::info!("limites modifiées : profondeur {d}, fichiers {f}, taille {s}");
        Ok(())
    }

    fn prepare_export(&self) -> Result<usize> {
        let dir = &self.conf.export_dir;
        std::fs::create_dir_all(dir)?;
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                std::fs::remove_dir_all(&p)?
            } else {
                std::fs::remove_file(&p)?
            }
        }
        let stamp = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string();
        let out = dir.join(format!("barbacane-export-{stamp}"));
        let rapports = out.join("rapports");
        std::fs::create_dir_all(&rapports)?;
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(&self.conf.reports_dir) {
            for e in rd {
                let p = e?.path();
                if p.is_file() {
                    std::fs::copy(&p, rapports.join(p.file_name().unwrap()))?;
                    n += 1;
                }
            }
        }
        let journal = if self.conf.dry_run {
            "(journal non disponible en mode test)\n".to_string()
        } else {
            self.run(&[
                "journalctl",
                "--no-pager",
                "-o",
                "short-iso",
                "--since",
                "-90d",
            ])
            .unwrap_or_else(|e| format!("journal indisponible : {e}\n"))
        };
        std::fs::write(out.join("journal.txt"), journal)?;
        std::fs::write(
            out.join("etat.json"),
            serde_json::to_vec_pretty(&self.status())?,
        )?;
        // État des paquets Debian : sert à calculer le prochain paquet de mise à jour.
        if !self.conf.dry_run {
            let _ = std::fs::copy("/var/lib/dpkg/status", out.join("dpkg-status"));
        }
        // Lisible par usbsas (source « dossier local »).
        let _ = self.run(&["chmod", "-R", "a+rX", &dir.display().to_string()]);
        Ok(n + 2)
    }

    fn arm_import(&self) -> Result<()> {
        let dir = &self.conf.import_dir;
        std::fs::create_dir_all(dir)?;
        for e in std::fs::read_dir(dir)? {
            let _ = std::fs::remove_file(e?.path());
        }
        let r = ureq::post(format!(
            "{}/barbacane/v1/import/arm",
            self.conf.analyzer_url
        ))
        .send_json(json!({}));
        r.map_err(|e| anyhow!("analyseur : {e}"))?;
        Ok(())
    }

    fn handle(&self, req: Value) -> Result<Value> {
        let op = req["op"].as_str().unwrap_or_default().to_string();
        if op == "status" {
            return Ok(self.status());
        }
        let _g = self.op.lock().unwrap();
        if op == "login" {
            return Ok(json!({"token": self.login(req["pin"].as_str().unwrap_or_default())?}));
        }
        self.check(&req)?;
        let msg = match op.as_str() {
            "logout" => {
                *self.session.lock().unwrap() = None;
                "Session fermée.".to_string()
            }
            "set_pin" => {
                self.set_pin(req["pin"].as_str().unwrap_or_default())?;
                log::info!("PIN administrateur modifié");
                "PIN modifié.".into()
            }
            "enroll_add" => {
                let mut e: Enrolled = self.read_json("enroles.json");
                let key = json!({
                    "vendorid": req["vendorid"], "productid": req["productid"], "serial": req["serial"],
                    "description": req["description"], "added": jiff::Zoned::now().strftime("%d/%m/%Y %H:%M").to_string(),
                });
                if req["serial"].as_str().unwrap_or_default().is_empty() {
                    bail!("cette clé n'a pas de numéro de série : elle ne peut pas être enrôlée");
                }
                e.keys.retain(|k| {
                    !(k["serial"] == key["serial"]
                        && k["vendorid"] == key["vendorid"]
                        && k["productid"] == key["productid"])
                });
                e.keys.push(key);
                self.write_json("enroles.json", &e)?;
                log::info!("clé enrôlée : {}", req["description"]);
                "Clé enrôlée.".into()
            }
            "enroll_remove" => {
                let mut e: Enrolled = self.read_json("enroles.json");
                let k = &req["key"];
                e.keys.retain(|x| {
                    !(x["serial"] == k["serial"]
                        && x["vendorid"] == k["vendorid"]
                        && x["productid"] == k["productid"])
                });
                self.write_json("enroles.json", &e)?;
                log::info!("clé retirée : {}", k["description"]);
                "Clé retirée.".into()
            }
            "set_enrolled_mode" => {
                let mut e: Enrolled = self.read_json("enroles.json");
                e.mode = req["enabled"].as_bool().unwrap_or(false);
                if e.mode && e.keys.is_empty() {
                    bail!("enrôlez au moins une clé avant d'activer ce mode");
                }
                self.write_json("enroles.json", &e)?;
                log::info!("mode clés enrôlées : {}", e.mode);
                if e.mode {
                    "Mode clés enrôlées activé."
                } else {
                    "Mode clés enrôlées désactivé."
                }
                .into()
            }
            "set_limits" => {
                self.set_limits(&req["limits"])?;
                "Limites enregistrées.".into()
            }
            "prepare_export" => format!("{} fichier(s) prêts", self.prepare_export()?),
            "arm_import" => {
                self.arm_import()?;
                "Prêt à lire la clé.".into()
            }
            "apply_update" => {
                let o = update::apply(self)?;
                log::info!("mise à jour version {} appliquée", o.version);
                return Ok(json!({"report": o.report, "reboot": o.reboot, "version": o.version}));
            }
            "reboot" => {
                self.run(&["systemctl", "reboot"])?;
                "Redémarrage…".into()
            }
            "poweroff" => {
                self.run(&["systemctl", "poweroff"])?;
                "Arrêt…".into()
            }
            _ => bail!("opération inconnue"),
        };
        Ok(json!({"message": msg}))
    }
}

fn serve_conn(ctx: &Ctx, s: UnixStream) -> Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut line = String::new();
    // Requêtes bornées à 64 Kio.
    BufReader::new(std::io::Read::take(&s, 64 * 1024)).read_line(&mut line)?;
    let rep = match serde_json::from_str::<Value>(&line) {
        Ok(req) => ctx
            .handle(req)
            .unwrap_or_else(|e| json!({"error": format!("{e:#}")})),
        Err(e) => json!({"error": format!("requête invalide : {e}")}),
    };
    let mut out = serde_json::to_vec(&rep)?;
    out.push(b'\n');
    (&s).write_all(&out)?;
    Ok(())
}

fn gid_of(group: &str) -> Option<u32> {
    std::fs::read_to_string("/etc/group")
        .ok()?
        .lines()
        .find_map(|l| {
            let mut it = l.split(':');
            (it.next()? == group).then(|| it.nth(1)?.parse().ok())?
        })
}

fn main() -> Result<()> {
    env_logger::init_from_env(env_logger::Env::default().filter_or("RUST_LOG", "info"));
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!(
            "barbacane-admind [-c FICHIER.toml] [--set-pin]\n  --set-pin : lit un nouveau PIN (6 chiffres minimum) sur l'entrée standard\n  sans option : service d'administration (socket Unix)"
        );
        return Ok(());
    }
    let conf_path = args
        .iter()
        .skip_while(|a| *a != "-c")
        .nth(1)
        .cloned()
        .unwrap_or_else(|| "/etc/barbacane/admind.toml".into());
    let conf: Config = match std::fs::read_to_string(&conf_path) {
        Ok(s) => toml::from_str(&s)?,
        Err(_) => Config::default(),
    };
    let ctx = Ctx {
        conf,
        session: Mutex::new(None),
        op: Mutex::new(()),
    };
    if args.iter().any(|a| a == "--set-pin") {
        let mut pin = String::new();
        std::io::stdin().read_line(&mut pin)?;
        ctx.set_pin(pin.trim())?;
        println!("PIN enregistré.");
        return Ok(());
    }
    let sock = &ctx.conf.socket;
    if let Some(p) = sock.parent() {
        std::fs::create_dir_all(p)?;
    }
    let _ = std::fs::remove_file(sock);
    let l = UnixListener::bind(sock)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o660))?;
    if let Some(g) = gid_of(&ctx.conf.socket_group) {
        if let Err(e) = std::os::unix::fs::chown(sock, None, Some(g)) {
            log::warn!("groupe de la socket non appliqué : {e}");
        }
    } else {
        log::warn!(
            "groupe {} introuvable : socket réservée à root",
            ctx.conf.socket_group
        );
    }
    log::info!("écoute sur {}", sock.display());
    std::thread::scope(|sc| {
        for s in l.incoming().flatten() {
            let ctx = &ctx;
            sc.spawn(move || {
                if let Err(e) = serve_conn(ctx, s) {
                    log::warn!("connexion : {e:#}");
                }
            });
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests;
