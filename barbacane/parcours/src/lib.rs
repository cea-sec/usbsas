//! Parcours Barbacane au-dessus d'usbsas, sans interface graphique :
//!
//! - **analyse** : transfert usbsas « clé inconnue → commande » avec analyse,
//!   suivi des demandes de mot de passe de l'analyseur ;
//! - **préparation** : l'analyseur remplit le dossier lu par usbsas ;
//! - **écriture** : transfert usbsas « dossier de préparation → clé » (exFAT) ;
//! - **effacement** : wipe usbsas (option « Blanchir »).
//!
//! Chaque opération usbsas se fait dans une session neuve (usbsas relance ses
//! processus entre deux opérations). Le client graphique exécute ces fonctions
//! dans un fil dédié et reçoit des [`Event`].

use serde::Deserialize;
use std::io;
use std::sync::mpsc::Sender;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use usbsas_comm::ProtoReqUsbsas;
pub use usbsas_proto as proto;
use usbsas_proto::common::{FsType, PartitionInfo, Status, UsbDevice, device::Device};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Usbsas(io::Error),
    Analyzer(String),
    /// Erreur à présenter telle quelle à l'utilisateur
    User(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Usbsas(e) => write!(f, "usbsas : {e}"),
            Error::Analyzer(e) => write!(f, "analyseur : {e}"),
            Error::User(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Usbsas(e)
    }
}
impl From<ureq::Error> for Error {
    fn from(e: ureq::Error) -> Self {
        Error::Analyzer(e.to_string())
    }
}

/// Événements envoyés à l'interface pendant une opération.
#[derive(Debug, Clone)]
pub enum Event {
    /// Étape usbsas et avancement (`current`/`total`, en octets ou fichiers)
    Progress {
        status: Status,
        current: u64,
        total: u64,
    },
    /// Avancement de l'analyse (fichiers de la clé analysés / total, fichiers analysés au total)
    Analysis { done: u64, total: u64, scanned: u64 },
    /// L'analyseur attend un mot de passe (répondre avec [`Analyzer::answer`])
    Password(PasswordRequest),
    /// Plus de mot de passe attendu
    PasswordDone,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PasswordRequest {
    /// Analyse concernée (rempli par le suivi, absent de la réponse de l'analyseur)
    #[serde(default)]
    pub job: String,
    pub archive: String,
    pub attempt: u32,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Progress {
    pub id: String,
    pub status: String,
    pub error: Option<String>,
    pub done: u64,
    pub total: u64,
    pub scanned: u64,
    pub password_request: Option<PasswordRequest>,
}

/// Ligne du rapport détaillé de l'analyseur.
#[derive(Debug, Clone, Deserialize)]
pub struct FileLine {
    pub path: String,
    pub container: Option<String>,
    pub size: u64,
    pub verdict: String,
    pub reason: String,
    pub output: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Report {
    pub id: String,
    pub status: String,
    pub files: Vec<FileLine>,
}

impl Report {
    /// Fichiers qui seront écrits sur la clé de sortie.
    pub fn copied(&self) -> impl Iterator<Item = &FileLine> {
        self.files.iter().filter(|f| f.output.is_some())
    }

    /// Archive refusée dont le contenu a été extrait (pas un refus pour l'utilisateur).
    pub fn is_unpacked(&self, f: &FileLine) -> bool {
        f.verdict == "DIRTY"
            && self
                .files
                .iter()
                .any(|c| c.container.as_deref() == Some(&f.path))
    }

    /// Fichiers refusés, avec leur raison.
    pub fn refused(&self) -> impl Iterator<Item = &FileLine> {
        self.files
            .iter()
            .filter(|f| f.verdict == "DIRTY" && !self.is_unpacked(f))
    }
}

/// Client de l'API de barbacane-analyzer.
#[derive(Clone)]
pub struct Analyzer {
    base: String,
    http: ureq::Agent,
}

impl Analyzer {
    /// `base` : par exemple `http://127.0.0.1:8042`
    pub fn new(base: &str) -> Self {
        let http = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(600)))
            .http_status_as_error(false)
            .build()
            .into();
        Analyzer {
            base: base.trim_end_matches('/').to_string(),
            http,
        }
    }

    fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<Option<T>> {
        let mut r = self.http.get(format!("{}{path}", self.base)).call()?;
        match r.status().as_u16() {
            404 => Ok(None),
            200 => Ok(Some(r.body_mut().read_json()?)),
            c => Err(Error::Analyzer(format!("{path} : HTTP {c}"))),
        }
    }

    fn post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        let mut r = self
            .http
            .post(format!("{}{path}", self.base))
            .send_json(body)?;
        match r.status().as_u16() {
            200 => Ok(r.body_mut().read_json()?),
            204 => Ok(serde_json::Value::Null),
            c => Err(Error::Analyzer(format!(
                "{path} : HTTP {c} {}",
                r.body_mut().read_to_string().unwrap_or_default()
            ))),
        }
    }

    /// Analyse en cours (ou dernière) de la borne `uid`.
    pub fn current(&self, uid: &str) -> Result<Option<Progress>> {
        self.get(&format!("/barbacane/v1/users/{uid}/current"))
    }

    /// Mot de passe (`Some`) ou abandon (`None`) pour l'archive en attente.
    pub fn answer(&self, id: &str, password: Option<&str>) -> Result<()> {
        let body = match password {
            Some(p) => serde_json::json!({"password": p}),
            None => serde_json::json!({"abandon": true}),
        };
        self.post(&format!("/barbacane/v1/scans/{id}/password"), body)
            .map(|_| ())
    }

    pub fn report(&self, id: &str) -> Result<Report> {
        self.get(&format!("/barbacane/v1/scans/{id}/report"))?
            .ok_or_else(|| Error::Analyzer("rapport introuvable".into()))
    }

    /// Remplit le dossier de préparation ; renvoie le nombre de fichiers.
    pub fn stage(&self, id: &str, device: &str) -> Result<u64> {
        let r = self.post(
            &format!("/barbacane/v1/scans/{id}/stage"),
            serde_json::json!({"device": device}),
        )?;
        Ok(r["files"].as_u64().unwrap_or(0))
    }

    /// État de l'analyseur (versions, âge de la base antivirale).
    pub fn status(&self) -> Result<serde_json::Value> {
        self.get("/barbacane/v1/status")?
            .ok_or_else(|| Error::Analyzer("état indisponible".into()))
    }
}

/// Périphériques et destinations connus d'usbsas.
#[derive(Debug, Clone, Default)]
pub struct Devices {
    pub usb: Vec<(u64, UsbDevice)>,
    /// Dossiers locaux (`[[src_local_dirs]]`) : identifiant, chemin
    pub local_dirs: Vec<(u64, String)>,
    /// Destination « commande » utilisée pour l'analyse seule
    pub sink: Option<u64>,
}

impl Devices {
    pub fn sources(&self) -> impl Iterator<Item = &(u64, UsbDevice)> {
        self.usb.iter().filter(|(_, d)| d.is_src)
    }

    pub fn destinations(&self) -> impl Iterator<Item = &(u64, UsbDevice)> {
        self.usb.iter().filter(|(_, d)| d.is_dst)
    }

    /// La même clé physique vue comme destination (pour « Blanchir »).
    pub fn same_as_dst(&self, src: &UsbDevice) -> Option<u64> {
        self.destinations()
            .find(|(_, d)| {
                d.serial == src.serial && d.vendorid == src.vendorid && d.productid == src.productid
            })
            .map(|(id, _)| *id)
    }

    /// Dossier local déclaré dans la configuration usbsas.
    pub fn local_dir(&self, path: &str) -> Option<u64> {
        self.local_dirs
            .iter()
            .find(|(_, p)| p.trim_end_matches('/') == path.trim_end_matches('/'))
            .map(|(id, _)| *id)
    }

    pub fn find(&self, dev: &UsbDevice, dst: bool) -> Option<u64> {
        self.usb
            .iter()
            .find(|(_, d)| {
                d.busnum == dev.busnum
                    && d.devnum == dev.devnum
                    && d.serial == dev.serial
                    && if dst { d.is_dst } else { d.is_src }
            })
            .map(|(id, _)| *id)
    }
}

/// Description d'une clé pour les rapports (horodatage + n° de série).
pub fn describe(d: &UsbDevice) -> String {
    format!(
        "{} {} (n° de série {}, {:04x}:{:04x})",
        d.manufacturer, d.description, d.serial, d.vendorid, d.productid
    )
}

pub fn devices(comm: &mut impl ProtoReqUsbsas) -> Result<Devices> {
    let list = comm
        .devices(proto::usbsas::RequestDevices { include_alt: true })?
        .devices;
    let mut out = Devices::default();
    for d in list {
        match d.device {
            Some(Device::Usb(u)) => out.usb.push((d.id, u)),
            Some(Device::LocalDir(l)) => out.local_dirs.push((d.id, l.path)),
            Some(Device::Command(_)) => out.sink = Some(d.id),
            _ => (),
        }
    }
    Ok(out)
}

/// Choix de partition par défaut : la plus grande dont le système de fichiers est reconnu.
// shortcut: une seule partition analysée par transfert ; proposer le choix à l'utilisateur si des clés multi-partitions se présentent
pub fn default_partition(parts: &[PartitionInfo]) -> Option<u32> {
    parts
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.type_str.is_empty() && !p.type_str.eq_ignore_ascii_case("unknown"))
        .max_by_key(|(_, p)| p.size)
        .map(|(i, _)| i as u32)
}

/// Résultat de l'analyse d'une clé.
#[derive(Debug)]
pub struct Analysis {
    pub uid: String,
    pub report: Report,
    /// Fichiers écartés par les filtres usbsas (fichiers système)
    pub filtered: Vec<String>,
}

fn wait_done(comm: &mut impl ProtoReqUsbsas, ev: &Sender<Event>) -> io::Result<()> {
    loop {
        let st = comm.recv_status()?;
        let status = Status::try_from(st.status).unwrap_or(Status::Unknown);
        let _ = ev.send(Event::Progress {
            status,
            current: st.current,
            total: st.total,
        });
        if status == Status::AllDone {
            return Ok(());
        }
    }
}

/// Lit la clé inconnue et la fait analyser. La session usbsas est terminée au retour.
pub fn analyse(
    comm: &mut impl ProtoReqUsbsas,
    analyzer: &Analyzer,
    src: u64,
    sink: u64,
    ev: &Sender<Event>,
) -> Result<Analysis> {
    let uid = comm.userid(proto::usbsas::RequestUserId {})?.userid;
    comm.inittransfer(proto::usbsas::RequestInitTransfer {
        source: src,
        destination: sink,
        fstype: None,
        pin: None,
    })?;
    let parts = comm
        .partitions(proto::usbsas::RequestPartitions {})?
        .partitions;
    let Some(index) = default_partition(&parts) else {
        let _ = comm.end();
        return Err(Error::User(
            "aucun système de fichiers lisible sur la clé".into(),
        ));
    };
    comm.openpartition(proto::usbsas::RequestOpenPartition { index })?;
    comm.selectfiles(proto::usbsas::RequestSelectFiles {
        selected: vec!["/".into()],
    })?;

    // Suivi de l'analyseur en parallèle : usbsas reste bloqué tant qu'un mot de passe est attendu.
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (stop, analyzer, ev, uid) = (stop.clone(), analyzer.clone(), ev.clone(), uid.clone());
        std::thread::spawn(move || {
            let mut asked = None;
            while !stop.load(Ordering::Relaxed) {
                if let Ok(Some(p)) = analyzer.current(&uid) {
                    let _ = ev.send(Event::Analysis {
                        done: p.done,
                        total: p.total,
                        scanned: p.scanned,
                    });
                    let mut req = p.password_request;
                    if let Some(r) = req.as_mut() {
                        r.job = p.id.clone();
                    }
                    if req != asked {
                        let _ = ev.send(match &req {
                            Some(r) => Event::Password(r.clone()),
                            None => Event::PasswordDone,
                        });
                        asked = req;
                    }
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        })
    };
    let res = wait_done(comm, ev);
    stop.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    let filtered = match res {
        Ok(()) => comm
            .report(proto::usbsas::RequestReport {})
            .ok()
            .and_then(|r| r.report)
            .map(|r| r.filtered)
            .unwrap_or_default(),
        // Rien de sain parmi les fichiers de la clé : le contenu extrait des archives peut l'être.
        Err(e) if e.to_string().contains("Nothing to copy after analyzer") => Vec::new(),
        Err(e) => {
            let _ = comm.end();
            return Err(e.into());
        }
    };
    let _ = comm.end();
    let p = analyzer
        .current(&uid)?
        .ok_or_else(|| Error::Analyzer("analyse introuvable".into()))?;
    if p.status != "scanned" {
        return Err(Error::Analyzer(
            p.error.unwrap_or_else(|| format!("statut {}", p.status)),
        ));
    }
    let report = analyzer.report(&p.id)?;
    Ok(Analysis {
        uid,
        report,
        filtered,
    })
}

/// Écrit le dossier de préparation sur la clé `dst` (effacée et formatée en exFAT par usbsas).
pub fn write(
    comm: &mut impl ProtoReqUsbsas,
    stage: u64,
    dst: u64,
    ev: &Sender<Event>,
) -> Result<proto::common::TransferReport> {
    comm.userid(proto::usbsas::RequestUserId {})?;
    comm.inittransfer(proto::usbsas::RequestInitTransfer {
        source: stage,
        destination: dst,
        fstype: Some(FsType::Exfat.into()),
        pin: None,
    })?;
    comm.selectfiles(proto::usbsas::RequestSelectFiles {
        selected: vec!["/".into()],
    })?;
    let res = wait_done(comm, ev);
    let report = res.and_then(|_| comm.report(proto::usbsas::RequestReport {}));
    let _ = comm.end();
    let report = report?
        .report
        .ok_or_else(|| Error::User("rapport d'écriture absent".into()))?;
    if !report.errors.is_empty() {
        return Err(Error::User(format!(
            "{} fichier(s) n'ont pas pu être écrits : {}",
            report.errors.len(),
            report.errors.join(", ")
        )));
    }
    Ok(report)
}

/// Efface entièrement la clé (zéros) et crée un système de fichiers exFAT vide.
pub fn wipe(comm: &mut impl ProtoReqUsbsas, dev: u64, ev: &Sender<Event>) -> Result<()> {
    comm.wipe(proto::usbsas::RequestWipe {
        id: dev,
        quick: false,
        fstype: FsType::Exfat.into(),
    })?;
    let res = wait_done(comm, ev);
    let _ = comm.end();
    Ok(res?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_choice() {
        let p = |size, t: &str| PartitionInfo {
            size,
            type_str: t.into(),
            ..Default::default()
        };
        assert_eq!(
            default_partition(&[p(10, "Unknown"), p(5, "FAT"), p(8, "NTFS")]),
            Some(2)
        );
        assert_eq!(default_partition(&[p(10, "Unknown")]), None);
    }
}
