//! Écrans et logique de l'interface Barbacane.
//!
//! Parcours : clé inconnue branchée → Copier / Blanchir → analyse (mots de
//! passe d'archives au clavier virtuel) → résumé → écriture → fin.

use crate::admin_ui::{self, AdminMsg, AdminUi};
use crate::config::Config;
use crate::keyboard::{Input, Key};
use crate::style;
use crate::worker::{self, Cmd, Ev};
use barbacane_parcours::{
    Analysis, Analyzer, Devices, Event, PasswordRequest,
    proto::common::{Status, UsbDevice},
};
use iced::widget::{Column, Space, button, column, container, progress_bar, row, scrollable, text};
use iced::{Alignment, Element, Length, Subscription, Task};
use std::sync::Arc;
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Copier,
    Blanchir,
}

#[derive(Debug)]
pub enum Screen {
    Connexion,
    Accueil,
    Choix,
    Analyse,
    MotDePasse(PasswordRequest),
    Resume,
    Sortie,
    ConfirmBlanchir,
    Ecriture,
    Fin(u64),
    Erreur { message: String, retry: bool },
    Admin,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Worker(Ev),
    Choisir(Mode),
    Key(Key),
    Valider,
    Ignorer,
    Repondu(Result<(), String>),
    VersSortie,
    Ecrire,
    Blanchir,
    Reessayer,
    Terminer,
    Etat(Result<serde_json::Value, String>),
    Tick,
    Admin(AdminMsg),
}

pub struct App {
    pub cfg: Config,
    pub screen: Screen,
    pub cmd: Option<Sender<Cmd>>,
    pub devices: Devices,
    src: Option<UsbDevice>,
    dst: Option<UsbDevice>,
    mode: Mode,
    progress: Option<(Status, u64, u64)>,
    analysis_progress: (u64, u64, u64),
    input: Input,
    analysis: Option<Arc<Analysis>>,
    pub analyzer: Analyzer,
    /// État renvoyé par barbacane-admind (clés enrôlées, âge des signatures…)
    pub etat: Option<serde_json::Value>,
    dst_refused: Option<String>,
    pub admin: AdminUi,
}

pub fn run(cfg: Config) -> iced::Result {
    iced::application(move || App::new(cfg.clone()), App::update, App::view)
        .title(|a: &App| a.cfg.title.clone())
        .subscription(App::subscription)
        .run()
}

/// Libellé d'une étape usbsas.
fn etape(s: Status) -> &'static str {
    match s {
        Status::ReadSrc => "Lecture de la clé",
        Status::UploadAv => "Envoi des fichiers à l'analyseur",
        Status::Analyze => "Analyse des fichiers",
        Status::MkFs => "Préparation de la clé de sortie",
        Status::MkArchive | Status::ExecCmd => "Fin de la lecture",
        Status::WriteDst => "Écriture sur la clé",
        Status::Wipe => "Effacement complet de la clé",
        Status::AllDone => "Terminé",
        _ => "Traitement en cours",
    }
}

impl App {
    fn new(cfg: Config) -> (Self, Task<Msg>) {
        let analyzer = Analyzer::new(&cfg.analyzer_url);
        let admin = AdminUi::new(cfg.admin_socket.clone());
        let app = App {
            cfg,
            screen: Screen::Connexion,
            cmd: None,
            devices: Devices::default(),
            src: None,
            dst: None,
            mode: Mode::Copier,
            progress: None,
            analysis_progress: (0, 0, 0),
            input: Input::text(128),
            analysis: None,
            analyzer,
            etat: None,
            dst_refused: None,
            admin,
        };
        let t = app.refresh_etat();
        (app, t)
    }

    pub fn refresh_etat(&self) -> Task<Msg> {
        let sock = self.cfg.admin_socket.clone();
        Task::perform(
            crate::admin::call(sock, serde_json::json!({"op": "status"})),
            Msg::Etat,
        )
    }

    fn subscription(&self) -> Subscription<Msg> {
        let w = Subscription::run_with(
            (
                self.cfg.usbsas_socket.clone(),
                self.cfg.stage_dir.clone(),
                self.cfg.export_dir.clone(),
                self.cfg.analyzer_url.clone(),
            ),
            |(sock, stage, export, url)| {
                let (sock, stage, export, url) =
                    (sock.clone(), stage.clone(), export.clone(), url.clone());
                iced::stream::channel(
                    100,
                    move |mut out: iced::futures::channel::mpsc::Sender<Ev>| async move {
                        use iced::futures::{SinkExt, StreamExt};
                        let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
                        worker::spawn(
                            sock,
                            worker::Dirs { stage, export },
                            Analyzer::new(&url),
                            tx,
                        );
                        while let Some(ev) = rx.next().await {
                            if out.send(ev).await.is_err() {
                                break;
                            }
                        }
                    },
                )
            },
        )
        .map(Msg::Worker);
        let tick = iced::time::every(std::time::Duration::from_secs(60)).map(|_| Msg::Tick);
        Subscription::batch([w, tick])
    }

    fn send(&self, c: Cmd) {
        if let Some(tx) = &self.cmd {
            let _ = tx.send(c);
        }
    }

    /// Mode « clés enrôlées » : liste blanche des clés de sortie (None : toute clé acceptée).
    fn enrolled(&self) -> Option<Vec<serde_json::Value>> {
        let e = self.etat.as_ref()?;
        if e["enrolled_mode"].as_bool() != Some(true) {
            return None;
        }
        Some(e["enrolled"].as_array().cloned().unwrap_or_default())
    }

    fn is_enrolled(&self, d: &UsbDevice) -> bool {
        match self.enrolled() {
            None => true,
            Some(list) => list.iter().any(|k| {
                k["vendorid"].as_u64() == Some(d.vendorid as u64)
                    && k["productid"].as_u64() == Some(d.productid as u64)
                    && k["serial"].as_str() == Some(d.serial.as_str())
            }),
        }
    }

    fn same(a: &UsbDevice, b: &UsbDevice) -> bool {
        a.busnum == b.busnum && a.devnum == b.devnum
    }

    fn on_devices(&mut self, d: Devices) {
        self.devices = d;
        let first_src = self.devices.sources().next().map(|(_, u)| u.clone());
        match &self.screen {
            Screen::Connexion => self.screen = Screen::Accueil,
            Screen::Accueil => {
                if let Some(s) = first_src {
                    self.src = Some(s);
                    self.screen = Screen::Choix;
                }
            }
            Screen::Choix | Screen::Resume => {
                let still = self
                    .src
                    .as_ref()
                    .is_some_and(|s| self.devices.sources().any(|(_, u)| Self::same(u, s)));
                if !still {
                    self.reset();
                }
            }
            Screen::Sortie => {
                let src = self.src.clone();
                let cand = self
                    .devices
                    .destinations()
                    .map(|(_, u)| u.clone())
                    .find(|u| src.as_ref().is_none_or(|s| !Self::same(u, s)));
                self.dst_refused = None;
                self.dst = None;
                if let Some(c) = cand {
                    if self.is_enrolled(&c) {
                        self.dst = Some(c);
                    } else {
                        self.dst_refused = Some(format!(
                            "Cette clé ({}) n'est pas enrôlée : elle ne peut pas servir de clé de sortie.",
                            barbacane_parcours::describe(&c)
                        ));
                    }
                }
            }
            Screen::Fin(_) if self.devices.usb.is_empty() => {
                self.reset();
            }
            _ => (),
        }
    }

    fn reset(&mut self) {
        self.screen = Screen::Accueil;
        self.src = None;
        self.dst = None;
        self.analysis = None;
        self.progress = None;
        self.dst_refused = None;
        self.input = Input::text(128);
    }

    fn update(&mut self, m: Msg) -> Task<Msg> {
        match m {
            Msg::Worker(ev) => self.on_worker(ev),
            Msg::Choisir(mode) => {
                if let Some(src) = self.src.clone() {
                    self.mode = mode;
                    self.progress = None;
                    self.analysis_progress = (0, 0, 0);
                    self.screen = Screen::Analyse;
                    self.send(Cmd::Analyse(src));
                }
                Task::none()
            }
            Msg::Key(k) => {
                self.input.press(k);
                Task::none()
            }
            Msg::Valider | Msg::Ignorer => {
                let Screen::MotDePasse(req) = &self.screen else {
                    return Task::none();
                };
                let pw = matches!(m, Msg::Valider).then(|| self.input.value.to_string());
                let (an, job) = (self.analyzer.clone(), req.job.clone());
                self.input = Input::text(128);
                self.screen = Screen::Analyse;
                Task::perform(
                    smol::unblock(move || {
                        an.answer(&job, pw.as_deref()).map_err(|e| e.to_string())
                    }),
                    Msg::Repondu,
                )
            }
            Msg::Repondu(r) => {
                if let Err(e) = r {
                    log::warn!("réponse mot de passe : {e}");
                }
                Task::none()
            }
            Msg::VersSortie => {
                self.dst = None;
                self.screen = Screen::Sortie;
                Task::none()
            }
            Msg::Ecrire => self.start_write(false),
            Msg::Blanchir => match self.screen {
                Screen::Resume => {
                    self.screen = Screen::ConfirmBlanchir;
                    Task::none()
                }
                _ => self.start_write(true),
            },
            Msg::Reessayer => self.start_write(false),
            Msg::Terminer => {
                self.reset();
                Task::none()
            }
            Msg::Etat(r) => {
                match r {
                    Ok(v) => self.etat = Some(v),
                    Err(e) => log::warn!("état admin : {e}"),
                }
                Task::none()
            }
            Msg::Tick => self.refresh_etat(),
            Msg::Admin(am) => admin_ui::update(self, am),
        }
    }

    fn start_write(&mut self, wipe: bool) -> Task<Msg> {
        let (Some(src), Some(an)) = (self.src.clone(), self.analysis.clone()) else {
            return Task::none();
        };
        let dst = match self.mode {
            Mode::Blanchir => src.clone(),
            Mode::Copier => match self.dst.clone() {
                Some(d) => d,
                None => return Task::none(),
            },
        };
        self.progress = None;
        self.screen = Screen::Ecriture;
        self.send(Cmd::Write {
            src,
            dst,
            wipe,
            job: an.report.id.clone(),
        });
        Task::none()
    }

    fn on_worker(&mut self, ev: Ev) -> Task<Msg> {
        match ev {
            Ev::Ready(tx) => self.cmd = Some(tx),
            Ev::Connected(false) => {
                if matches!(self.screen, Screen::Accueil | Screen::Choix) {
                    self.screen = Screen::Connexion;
                }
            }
            Ev::Connected(true) => (),
            Ev::Devices(d) => self.on_devices(d),
            Ev::Op(Event::Progress {
                status,
                current,
                total,
            }) => self.progress = Some((status, current, total)),
            Ev::Op(Event::Analysis {
                done,
                total,
                scanned,
            }) => self.analysis_progress = (done, total, scanned),
            Ev::Op(Event::Password(req)) => {
                if matches!(self.screen, Screen::Analyse | Screen::MotDePasse(_)) {
                    self.input = Input::text(128);
                    self.screen = Screen::MotDePasse(req);
                }
            }
            Ev::Op(Event::PasswordDone) => {
                if matches!(self.screen, Screen::MotDePasse(_)) {
                    self.screen = Screen::Analyse;
                }
            }
            Ev::Analysed(r) if matches!(self.screen, Screen::Admin) => {
                return admin_ui::analysed(self, r.map(|_| ()));
            }
            Ev::Analysed(Ok(a)) => {
                self.analysis = Some(a);
                self.screen = Screen::Resume;
            }
            Ev::Analysed(Err(e)) => {
                self.screen = Screen::Erreur {
                    message: format!(
                        "L'analyse n'a pas pu aboutir : {e}. Aucun fichier n'a été copié."
                    ),
                    retry: false,
                }
            }
            Ev::Written(Ok(n)) => self.screen = Screen::Fin(n),
            Ev::Written(Err(e)) => {
                self.screen = Screen::Erreur {
                    message: match self.mode {
                        Mode::Blanchir => format!(
                            "Le blanchiment a échoué : {e}.\nLa clé a peut-être été effacée sans être réécrite : ne l'utilisez pas en l'état. \
                             Laissez-la branchée et réessayez l'écriture."
                        ),
                        Mode::Copier => format!(
                            "L'écriture sur la clé de sortie a échoué : {e}.\nNe l'utilisez pas en l'état ; réessayez."
                        ),
                    },
                    retry: true,
                }
            }
            Ev::Exported(r) => return admin_ui::exported(self, r),
        }
        Task::none()
    }

    // ---------------------------------------------------------------- vue

    fn view(&self) -> Element<'_, Msg> {
        let body: Element<Msg> = match &self.screen {
            Screen::Connexion => centre(column![
                titre("Démarrage…"),
                texte("Connexion au service de lecture des clés USB."),
            ]),
            Screen::Accueil => centre(column![
                text("🛡").size(110).shaping(text::Shaping::Advanced),
                titre("Branchez la clé USB à analyser"),
                texte("sur le port d'entrée de la borne."),
                Space::new().height(30),
                petit(
                    "Tous les fichiers seront analysés, y compris le contenu des archives.\nSeuls les fichiers sains seront copiés."
                ),
            ]),
            Screen::Choix => self.view_choix(),
            Screen::Analyse => self.view_progress("Analyse en cours", "Ne retirez pas la clé."),
            Screen::MotDePasse(req) => self.view_password(req),
            Screen::Resume => self.view_resume(),
            Screen::Sortie => self.view_sortie(),
            Screen::ConfirmBlanchir => centre(column![
                titre("Blanchir cette clé ?"),
                carte(column![
                    texte_c(
                        "La clé va être ENTIÈREMENT EFFACÉE, puis seuls les fichiers sains y seront réécrits.",
                        style::ROUGE
                    ),
                    texte(&format!(
                        "{} fichier(s) seront réécrits.",
                        self.analysis
                            .as_ref()
                            .map(|a| a.report.copied().count())
                            .unwrap_or(0)
                    )),
                    petit("Les fichiers refusés seront définitivement supprimés de la clé."),
                ]),
                Space::new().height(20),
                row![
                    bouton("Annuler", Msg::Terminer, style::secondaire),
                    bouton("Effacer et réécrire", Msg::Blanchir, style::danger),
                ]
                .spacing(30),
            ]),
            Screen::Ecriture => self.view_progress(
                match self.mode {
                    Mode::Copier => "Écriture sur la clé de sortie",
                    Mode::Blanchir => "Blanchiment de la clé",
                },
                "Ne retirez aucune clé.",
            ),
            Screen::Fin(n) => centre(column![
                text("✔")
                    .size(100)
                    .color(style::VERT)
                    .shaping(text::Shaping::Advanced),
                titre("Terminé"),
                texte(&format!(
                    "{n} fichier(s) sain(s) écrit(s), avec le rapport (rapport-barbacane-…txt / .html)."
                )),
                texte("Vous pouvez retirer les clés."),
                Space::new().height(20),
                bouton("Nouveau transfert", Msg::Terminer, style::primaire),
            ]),
            Screen::Erreur { message, retry } => {
                let mut b = row![bouton(
                    "Recommencer depuis le début",
                    Msg::Terminer,
                    style::secondaire
                )]
                .spacing(30);
                if *retry {
                    b = b.push(bouton(
                        "Réessayer l'écriture",
                        Msg::Reessayer,
                        style::primaire,
                    ));
                }
                centre(column![
                    text("⚠")
                        .size(90)
                        .color(style::ROUGE)
                        .shaping(text::Shaping::Advanced),
                    titre("Une erreur est survenue"),
                    carte(column![texte(message)]),
                    Space::new().height(20),
                    b,
                ])
            }
            Screen::Admin => admin_ui::view(self),
        };
        let page = column![
            self.view_header(),
            container(body)
                .padding(30)
                .width(Length::Fill)
                .height(Length::Fill),
            self.view_footer()
        ];
        container(page)
            .style(style::fond)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_header(&self) -> Element<'_, Msg> {
        let mut r = row![
            text(&self.cfg.title)
                .size(style::TITRE)
                .color(iced::Color::WHITE),
            Space::new().width(Length::Fill)
        ]
        .align_y(Alignment::Center);
        let age = self
            .etat
            .as_ref()
            .and_then(|e| e["signatures_age_days"].as_u64());
        match age {
            Some(a) if a > self.cfg.signatures_max_age_days => r = r.push(
                container(
                    text(format!(
                        "⚠ Signatures antivirales anciennes ({a} jours) : prévenez l'administrateur"
                    ))
                    .size(style::PETIT)
                    .shaping(text::Shaping::Advanced),
                )
                .padding(8)
                .style(style::alerte),
            ),
            Some(a) => {
                r = r.push(
                    text(format!("Signatures antivirales : {a} jour(s)"))
                        .size(style::PETIT)
                        .color(iced::Color::WHITE),
                )
            }
            None if self.etat.is_some() => {
                r = r.push(
                    container(
                        text("⚠ Base antivirale absente ou illisible : prévenez l'administrateur")
                            .size(style::PETIT)
                            .shaping(text::Shaping::Advanced),
                    )
                    .padding(8)
                    .style(style::alerte),
                )
            }
            None => (),
        }
        container(r)
            .padding([14, 30])
            .style(style::bandeau)
            .width(Length::Fill)
            .into()
    }

    fn view_footer(&self) -> Element<'_, Msg> {
        let admin_ok = matches!(
            self.screen,
            Screen::Accueil | Screen::Connexion | Screen::Choix
        );
        let mut adm = button(text("Administration").size(style::PETIT)).style(style::discret);
        if admin_ok {
            adm = adm.on_press(Msg::Admin(AdminMsg::Ouvrir));
        }
        let mut r = row![
            petit(&format!(
                "Barbacane {} — station blanche hors ligne",
                env!("CARGO_PKG_VERSION")
            )),
            Space::new().width(Length::Fill),
        ]
        .align_y(Alignment::Center);
        if self.enrolled().is_some() {
            r = r
                .push(petit("Mode clés enrôlées"))
                .push(Space::new().width(20));
        }
        container(r.push(adm))
            .padding([6, 30])
            .width(Length::Fill)
            .into()
    }

    fn view_choix(&self) -> Element<'_, Msg> {
        let d = self
            .src
            .as_ref()
            .map(barbacane_parcours::describe)
            .unwrap_or_default();
        centre(column![
            titre("Clé détectée"),
            texte(&d),
            Space::new().height(30),
            row![
                grand_choix("Copier vers une autre clé", "Les fichiers sains sont copiés sur une clé de sortie,\nqui sera effacée et formatée.", Msg::Choisir(Mode::Copier)),
                grand_choix("Blanchir cette clé", "La clé est analysée, entièrement effacée,\npuis seuls ses fichiers sains y sont réécrits.", Msg::Choisir(Mode::Blanchir)),
            ]
            .spacing(40),
        ])
    }

    fn view_progress(&self, t: &str, sub: &str) -> Element<'_, Msg> {
        let mut c = column![titre(t), texte(sub), Space::new().height(30)]
            .spacing(10)
            .align_x(Alignment::Center);
        if let Some((s, cur, tot)) = self.progress {
            c = c.push(texte(etape(s)));
            if tot > 0 {
                c = c.push(
                    progress_bar(0.0..=tot as f32, cur as f32)
                        .length(Length::Fixed(700.0))
                        .girth(28),
                );
            }
        }
        let (done, total, scanned) = self.analysis_progress;
        if matches!(self.screen, Screen::Analyse) && total > 0 {
            c = c.push(texte(&format!("Fichiers de la clé analysés : {done} / {total} ({scanned} au total avec le contenu des archives)")));
        }
        centre(c)
    }

    fn view_password(&self, req: &PasswordRequest) -> Element<'_, Msg> {
        centre(column![
            titre("Archive protégée par mot de passe"),
            texte(&format!("« {} »", req.archive)),
            petit(&format!(
                "Saisissez son mot de passe pour en analyser le contenu (essai {} sur {}). Sans mot de passe, l'archive sera refusée.",
                req.attempt, req.max_attempts
            )),
            container(
                text(self.input.shown())
                    .size(30)
                    .shaping(text::Shaping::Advanced)
            )
            .padding(14)
            .width(Length::Fixed(700.0))
            .style(style::carte),
            self.input.view(Msg::Key),
            row![
                bouton("Ignorer cette archive", Msg::Ignorer, style::secondaire),
                bouton("Valider", Msg::Valider, style::primaire),
            ]
            .spacing(30),
        ])
    }

    fn view_resume(&self) -> Element<'_, Msg> {
        let Some(a) = &self.analysis else {
            return centre(column![titre("…")]);
        };
        let copied = a.report.copied().count();
        let refused: Vec<_> = a.report.refused().collect();
        let mut list = Column::new().spacing(8);
        for f in &refused {
            list = list.push(
                column![
                    text(&f.path)
                        .size(style::TEXTE_TAILLE)
                        .color(style::ROUGE)
                        .shaping(text::Shaping::Advanced),
                    petit(&f.reason)
                ]
                .spacing(2),
            );
        }
        for f in &a.filtered {
            list = list.push(
                column![
                    text(f.trim_start_matches('/'))
                        .size(style::TEXTE_TAILLE)
                        .color(style::GRIS)
                        .shaping(text::Shaping::Advanced),
                    petit("fichier système écarté")
                ]
                .spacing(2),
            );
        }
        let unpacked: Vec<_> = a
            .report
            .files
            .iter()
            .filter(|f| a.report.is_unpacked(f))
            .collect();
        let mut c = column![
            titre("Résultat de l'analyse"),
            row![
                pastille(
                    &format!("{copied}"),
                    "fichier(s) sain(s) à copier",
                    style::VERT
                ),
                pastille(
                    &format!("{}", refused.len() + a.filtered.len()),
                    "fichier(s) refusé(s)",
                    style::ROUGE
                ),
            ]
            .spacing(30),
        ]
        .spacing(16)
        .align_x(Alignment::Center);
        if !unpacked.is_empty() {
            c = c.push(petit(&format!(
                "{} archive(s) contenant des fichiers refusés : leurs fichiers sains seront copiés dans un dossier « <archive>.extrait ».",
                unpacked.len()
            )));
        }
        if !refused.is_empty() || !a.filtered.is_empty() {
            c = c.push(texte("Ce qui ne passe pas, et pourquoi :")).push(
                container(
                    scrollable(list)
                        .height(Length::Fixed(300.0))
                        .width(Length::Fixed(900.0)),
                )
                .padding(16)
                .style(style::carte),
            );
        }
        let action = match self.mode {
            Mode::Copier => bouton("Choisir la clé de sortie", Msg::VersSortie, style::primaire),
            Mode::Blanchir => bouton("Blanchir la clé", Msg::Blanchir, style::danger),
        };
        c = c.push(Space::new().height(10)).push(
            row![
                bouton("Abandonner", Msg::Terminer, style::secondaire),
                action
            ]
            .spacing(30),
        );
        centre(c)
    }

    fn view_sortie(&self) -> Element<'_, Msg> {
        let mut c = column![
            titre("Branchez la clé de sortie"),
            texte("sur le port de sortie de la borne.")
        ]
        .spacing(14)
        .align_x(Alignment::Center);
        if let Some(r) = &self.dst_refused {
            c = c.push(container(texte(r)).padding(12).style(style::alerte));
        }
        let mut go =
            button(text("Effacer cette clé et y copier les fichiers").size(style::TEXTE_TAILLE))
                .padding([18, 30])
                .style(style::primaire);
        if let Some(d) = &self.dst {
            c = c.push(carte(column![
                texte(&format!(
                    "Clé de sortie : {}",
                    barbacane_parcours::describe(d)
                )),
                texte_c("Son contenu actuel sera entièrement effacé.", style::ROUGE),
            ]));
            go = go.on_press(Msg::Ecrire);
        }
        c = c
            .push(Space::new().height(20))
            .push(row![bouton("Abandonner", Msg::Terminer, style::secondaire), go].spacing(30));
        centre(c)
    }
}

// ---------------------------------------------------------------- briques

pub fn centre<'a>(c: Column<'a, Msg>) -> Element<'a, Msg> {
    container(c.spacing(16).align_x(Alignment::Center))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

pub fn titre<'a>(t: &str) -> Element<'a, Msg> {
    text(t.to_string())
        .size(style::TITRE)
        .color(style::BLEU)
        .shaping(text::Shaping::Advanced)
        .into()
}

pub fn texte<'a>(t: &str) -> Element<'a, Msg> {
    text(t.to_string())
        .size(style::TEXTE_TAILLE)
        .shaping(text::Shaping::Advanced)
        .into()
}

pub fn texte_c<'a>(t: &str, c: iced::Color) -> Element<'a, Msg> {
    text(t.to_string())
        .size(style::TEXTE_TAILLE)
        .color(c)
        .shaping(text::Shaping::Advanced)
        .into()
}

pub fn petit<'a>(t: &str) -> Element<'a, Msg> {
    text(t.to_string())
        .size(style::PETIT)
        .color(style::GRIS)
        .shaping(text::Shaping::Advanced)
        .into()
}

pub fn carte<'a>(c: Column<'a, Msg>) -> Element<'a, Msg> {
    container(c.spacing(10))
        .padding(20)
        .style(style::carte)
        .into()
}

pub fn bouton<'a>(
    t: &str,
    m: Msg,
    s: fn(&iced::Theme, button::Status) -> button::Style,
) -> Element<'a, Msg> {
    button(
        text(t.to_string())
            .size(style::TEXTE_TAILLE)
            .shaping(text::Shaping::Advanced),
    )
    .padding([18, 30])
    .style(s)
    .on_press(m)
    .into()
}

fn grand_choix<'a>(t: &str, d: &str, m: Msg) -> Element<'a, Msg> {
    button(
        column![
            text(t.to_string()).size(style::SOUS_TITRE),
            text(d.to_string()).size(style::PETIT)
        ]
        .spacing(14)
        .align_x(Alignment::Center),
    )
    .padding(30)
    .width(Length::Fixed(460.0))
    .style(style::primaire)
    .on_press(m)
    .into()
}

fn pastille<'a>(n: &str, l: &str, c: iced::Color) -> Element<'a, Msg> {
    container(
        column![
            text(n.to_string()).size(54).color(c),
            text(l.to_string()).size(style::TEXTE_TAILLE)
        ]
        .align_x(Alignment::Center),
    )
    .center_x(Length::Fixed(320.0))
    .padding(20)
    .width(Length::Fixed(320.0))
    .style(style::carte)
    .into()
}
