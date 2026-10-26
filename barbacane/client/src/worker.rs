//! Fil de travail : tient la session usbsas (socket du service
//! usbsas-server), surveille les clés branchées et exécute les parcours de
//! `barbacane-parcours`. Il ne bloque jamais l'interface.

use barbacane_parcours::{self as parcours, Analyzer, Devices, Event, proto::common::UsbDevice};
use iced::futures::channel::mpsc::UnboundedSender;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;
use usbsas_comm::Comm;

type Usbsas = Comm<barbacane_parcours::proto::usbsas::Request, UnixStream, UnixStream>;

/// Ordres de l'interface.
#[derive(Debug, Clone)]
pub enum Cmd {
    /// Lire et analyser la clé
    Analyse(UsbDevice),
    /// Préparer puis écrire sur `dst` ; `wipe` : effacer `dst` d'abord (Blanchir)
    Write {
        src: UsbDevice,
        dst: UsbDevice,
        wipe: bool,
        job: String,
    },
    /// Écrire un dossier local (export admin) sur une clé
    Export { dst: UsbDevice },
}

/// Nouvelles pour l'interface.
#[derive(Debug, Clone)]
pub enum Ev {
    /// Canal d'ordres du fil de travail
    Ready(Sender<Cmd>),
    Connected(bool),
    Devices(Devices),
    Op(Event),
    Analysed(Result<std::sync::Arc<parcours::Analysis>, String>),
    Written(Result<u64, String>),
    Exported(Result<(), String>),
}

/// Chemins des dossiers locaux déclarés dans `[[src_local_dirs]]` d'usbsas.
#[derive(Clone)]
pub struct Dirs {
    pub stage: String,
    pub export: String,
}

pub struct Worker {
    pub socket: String,
    dirs: Dirs,
    pub analyzer: Analyzer,
    out: UnboundedSender<Ev>,
    session: Option<Usbsas>,
    devices: Devices,
}

/// Lance le fil de travail ; les nouvelles arrivent sur `out`.
pub fn spawn(socket: String, dirs: Dirs, analyzer: Analyzer, out: UnboundedSender<Ev>) {
    let (tx, rx) = channel();
    let _ = out.unbounded_send(Ev::Ready(tx));
    std::thread::spawn(move || {
        Worker {
            socket,
            dirs,
            analyzer,
            out,
            session: None,
            devices: Devices::default(),
        }
        .run(rx)
    });
}

impl Worker {
    fn send(&self, ev: Ev) {
        let _ = self.out.unbounded_send(ev);
    }

    fn connect(&self) -> Option<Usbsas> {
        // usbsas-server se relance après chaque session : la socket réapparaît en quelques instants.
        let s = UnixStream::connect(format!("{}/usbsas.sock", self.socket.trim_end_matches('/')))
            .ok()?;
        let s2 = s.try_clone().ok()?;
        Some(Comm::new(s, s2))
    }

    /// Session usbsas prête (déjà ouverte ou nouvelle).
    fn session(&mut self) -> Option<&mut Usbsas> {
        if self.session.is_none() {
            self.session = self.connect();
            self.send(Ev::Connected(self.session.is_some()));
        }
        self.session.as_mut()
    }

    /// Session neuve pour une opération (l'ancienne est terminée par l'opération précédente).
    fn fresh(&mut self) -> Result<Usbsas, String> {
        if let Some(s) = self.session.take() {
            return Ok(s);
        }
        for _ in 0..50 {
            if let Some(s) = self.connect() {
                return Ok(s);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err("le service usbsas ne répond pas".into())
    }

    fn poll(&mut self) {
        let Some(s) = self.session() else { return };
        match parcours::devices(s) {
            Ok(d) => {
                self.devices = d.clone();
                self.send(Ev::Devices(d));
            }
            Err(e) => {
                log::warn!("liste des clés : {e}");
                self.session = None;
                self.send(Ev::Connected(false));
            }
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => self.poll(),
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        let (etx, erx) = channel::<Event>();
        // Relais des événements de l'opération vers l'interface.
        let out = self.out.clone();
        let relay = std::thread::spawn(move || {
            for e in erx {
                let _ = out.unbounded_send(Ev::Op(e));
            }
        });
        match cmd {
            Cmd::Analyse(src) => {
                let r = self.analyse(&src, &etx);
                self.send(Ev::Analysed(
                    r.map(std::sync::Arc::new).map_err(|e| e.to_string()),
                ));
            }
            Cmd::Write {
                src,
                dst,
                wipe,
                job,
            } => {
                let r = self.write(&src, &dst, wipe, &job, &etx);
                self.send(Ev::Written(r.map_err(|e| e.to_string())));
            }
            Cmd::Export { dst } => {
                let r = self.export(&dst, &etx);
                self.send(Ev::Exported(r.map_err(|e| e.to_string())));
            }
        }
        drop(etx);
        let _ = relay.join();
    }

    fn analyse(
        &mut self,
        src: &UsbDevice,
        ev: &Sender<Event>,
    ) -> Result<parcours::Analysis, String> {
        let mut s = self.fresh()?;
        let devs = parcours::devices(&mut s).map_err(|e| e.to_string())?;
        let id = devs.find(src, false).ok_or("la clé a été retirée")?;
        let sink = devs
            .sink
            .ok_or("configuration usbsas incomplète ([command] absent)")?;
        parcours::analyse(&mut s, &self.analyzer, id, sink, ev).map_err(|e| e.to_string())
    }

    fn write(
        &mut self,
        src: &UsbDevice,
        dst: &UsbDevice,
        wipe: bool,
        job: &str,
        ev: &Sender<Event>,
    ) -> Result<u64, String> {
        let n = self
            .analyzer
            .stage(job, &parcours::describe(src))
            .map_err(|e| e.to_string())?;
        if wipe {
            let mut s = self.fresh()?;
            let devs = parcours::devices(&mut s).map_err(|e| e.to_string())?;
            let id = devs.same_as_dst(dst).ok_or("la clé a été retirée")?;
            parcours::wipe(&mut s, id, ev).map_err(|e| format!("effacement : {e}"))?;
        }
        let stage = self.dirs.stage.clone();
        self.write_stage(dst, ev, &stage)?;
        Ok(n)
    }

    /// Écrit un dossier local déclaré dans `[[src_local_dirs]]` sur `dst`.
    fn write_stage(
        &mut self,
        dst: &UsbDevice,
        ev: &Sender<Event>,
        dir: &str,
    ) -> Result<(), String> {
        let mut s = self.fresh()?;
        let devs = parcours::devices(&mut s).map_err(|e| e.to_string())?;
        let id = devs
            .find(dst, true)
            .or_else(|| devs.same_as_dst(dst))
            .ok_or("la clé de sortie a été retirée")?;
        let stage = devs
            .local_dir(dir)
            .ok_or_else(|| format!("configuration usbsas incomplète ([[src_local_dirs]] {dir})"))?;
        parcours::write(&mut s, stage, id, ev)
            .map(|_| ())
            .map_err(|e| format!("écriture : {e}"))
    }

    fn export(&mut self, dst: &UsbDevice, ev: &Sender<Event>) -> Result<(), String> {
        let export = self.dirs.export.clone();
        self.write_stage(dst, ev, &export)
    }
}
