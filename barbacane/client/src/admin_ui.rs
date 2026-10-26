//! Écrans du mode administrateur (PIN au clavier virtuel). Toutes les
//! actions sont exécutées par barbacane-admind, qui vérifie le jeton.

use crate::app::{App, Msg, Screen, bouton, carte, petit, texte, texte_c, titre};
use crate::keyboard::{Input, Key};
use crate::style;
use crate::worker::Cmd;
use barbacane_parcours::proto::common::UsbDevice;
use iced::widget::{Column, Space, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length, Task};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub enum Page {
    Pin,
    Menu,
    Etat,
    MiseAJour,
    Export,
    Enrolement,
    Limites,
    NouveauPin,
    Confirmer(&'static str),
}

#[derive(Debug, Clone)]
pub enum AdminMsg {
    Ouvrir,
    Key(Key),
    Login,
    Aller(Page),
    Quitter,
    Resp(&'static str, Result<Value, String>),
    /// Lire la clé admin (via usbsas) puis appliquer la mise à jour
    LireMaj,
    Exporter,
    Enroler(UsbDevice),
    Retirer(Value),
    ModeEnrole(bool),
    Limite(&'static str, i64),
    EnregistrerLimites,
    EnregistrerPin,
    Executer(&'static str),
}

pub struct AdminUi {
    socket: String,
    pub page: Page,
    token: Option<String>,
    input: Input,
    first_pin: Option<zeroize::Zeroizing<String>>,
    message: Option<(bool, String)>,
    limits: Value,
    busy: bool,
}

impl AdminUi {
    pub fn new(socket: String) -> Self {
        AdminUi {
            socket,
            page: Page::Pin,
            token: None,
            input: Input::digits(12),
            first_pin: None,
            message: None,
            limits: json!({}),
            busy: false,
        }
    }

    fn call(&mut self, tag: &'static str, mut req: Value) -> Task<Msg> {
        if let Some(t) = &self.token {
            req["token"] = json!(t);
        }
        self.busy = true;
        Task::perform(crate::admin::call(self.socket.clone(), req), move |r| {
            Msg::Admin(AdminMsg::Resp(tag, r))
        })
    }
}

pub fn update(app: &mut App, m: AdminMsg) -> Task<Msg> {
    let a = &mut app.admin;
    match m {
        AdminMsg::Ouvrir => {
            a.page = Page::Pin;
            a.input = Input::digits(12);
            a.message = None;
            app.screen = Screen::Admin;
        }
        AdminMsg::Key(k) => a.input.press(k),
        AdminMsg::Login => {
            let pin = a.input.value.to_string();
            a.input = Input::digits(12);
            return a.call("login", json!({"op": "login", "pin": pin}));
        }
        AdminMsg::Aller(p) => {
            a.message = None;
            a.input = Input::digits(12);
            a.first_pin = None;
            if p == Page::Limites {
                a.limits = app
                    .etat
                    .as_ref()
                    .map(|e| e["limits"].clone())
                    .unwrap_or(json!({}));
            }
            a.page = p;
            return app.refresh_etat();
        }
        AdminMsg::Quitter => {
            let t = if a.token.is_some() {
                a.call("logout", json!({"op": "logout"}))
            } else {
                Task::none()
            };
            a.token = None;
            app.screen = Screen::Accueil;
            return Task::batch([t, app.refresh_etat()]);
        }
        AdminMsg::Resp(tag, r) => {
            a.busy = false;
            match (tag, r) {
                ("login", Ok(v)) => {
                    a.token = v["token"].as_str().map(str::to_string);
                    a.page = Page::Menu;
                }
                ("logout", _) => (),
                ("prepare_export", Ok(_)) => {
                    // Fichiers prêts : écriture sur la clé admin via usbsas.
                    match app.devices.destinations().next().map(|(_, d)| d.clone()) {
                        Some(dst) => {
                            app.admin.busy = true;
                            app.admin.message =
                                Some((true, "Écriture des journaux sur la clé…".into()));
                            if let Some(tx) = app_cmd(app) {
                                let _ = tx.send(Cmd::Export { dst });
                            }
                        }
                        None => {
                            app.admin.message = Some((
                                false,
                                "Branchez la clé administrateur sur le port de sortie.".into(),
                            ))
                        }
                    }
                }
                ("arm_import", Ok(_)) => match app.devices.sources().next().map(|(_, d)| d.clone())
                {
                    Some(src) => {
                        app.admin.busy = true;
                        app.admin.message =
                            Some((true, "Lecture de la clé administrateur…".into()));
                        if let Some(tx) = app_cmd(app) {
                            let _ = tx.send(Cmd::Analyse(src));
                        }
                    }
                    None => {
                        app.admin.message = Some((
                            false,
                            "Branchez la clé administrateur sur le port d'entrée.".into(),
                        ))
                    }
                },
                ("apply_update", Ok(v)) => {
                    let txt = v["report"]
                        .as_str()
                        .unwrap_or("Mise à jour appliquée.")
                        .to_string();
                    app.admin.message = Some((true, txt));
                    return app.refresh_etat();
                }
                (_, Ok(v)) => {
                    app.admin.message =
                        Some((true, v["message"].as_str().unwrap_or("Fait.").to_string()));
                    if tag == "set_pin" {
                        app.admin.page = Page::Menu;
                    }
                    return app.refresh_etat();
                }
                ("login", Err(e)) => a.message = Some((false, e)),
                (_, Err(e)) => {
                    if e.contains("session") {
                        a.token = None;
                        a.page = Page::Pin;
                    }
                    a.message = Some((false, e));
                }
            }
        }
        AdminMsg::LireMaj => {
            a.message = None;
            return a.call("arm_import", json!({"op": "arm_import"}));
        }
        AdminMsg::Exporter => {
            a.message = None;
            return a.call("prepare_export", json!({"op": "prepare_export"}));
        }
        AdminMsg::Enroler(d) => {
            return a.call(
                "enroll_add",
                json!({"op": "enroll_add", "vendorid": d.vendorid, "productid": d.productid, "serial": d.serial,
                       "description": barbacane_parcours::describe(&d)}),
            );
        }
        AdminMsg::Retirer(k) => {
            return a.call("enroll_remove", json!({"op": "enroll_remove", "key": k}));
        }
        AdminMsg::ModeEnrole(on) => {
            return a.call(
                "set_enrolled_mode",
                json!({"op": "set_enrolled_mode", "enabled": on}),
            );
        }
        AdminMsg::Limite(k, delta) => {
            let cur = a.limits[k].as_i64().unwrap_or(0);
            let step = match k {
                "max_depth" => 1,
                "max_files" => 1000,
                _ => 512 * 1024 * 1024,
            };
            a.limits[k] = json!((cur + delta * step).max(step));
        }
        AdminMsg::EnregistrerLimites => {
            let l = a.limits.clone();
            return a.call("set_limits", json!({"op": "set_limits", "limits": l}));
        }
        AdminMsg::EnregistrerPin => {
            let v = a.input.value.clone();
            a.input = Input::digits(12);
            match a.first_pin.take() {
                None => {
                    if v.len() < 6 {
                        a.message =
                            Some((false, "Le PIN doit comporter au moins 6 chiffres.".into()));
                    } else {
                        a.first_pin = Some(v);
                        a.message = Some((true, "Saisissez à nouveau le nouveau PIN.".into()));
                    }
                }
                Some(first) if *first == *v => {
                    return a.call("set_pin", json!({"op": "set_pin", "pin": v.as_str()}));
                }
                Some(_) => {
                    a.message = Some((false, "Les deux saisies diffèrent, recommencez.".into()))
                }
            }
        }
        AdminMsg::Executer(op) => return a.call(op, json!({"op": op})),
    }
    Task::none()
}

fn app_cmd(app: &App) -> Option<std::sync::mpsc::Sender<Cmd>> {
    app.cmd.clone()
}

/// Fin de lecture de la clé admin (transfert usbsas) : demande l'application.
pub fn analysed(app: &mut App, r: Result<(), String>) -> Task<Msg> {
    match r {
        Ok(()) => {
            app.admin.message = Some((
                true,
                "Vérification et application de la mise à jour…".into(),
            ));
            app.admin
                .call("apply_update", json!({"op": "apply_update"}))
        }
        Err(e) => {
            app.admin.busy = false;
            app.admin.message = Some((false, format!("Lecture de la clé impossible : {e}")));
            Task::none()
        }
    }
}

pub fn exported(app: &mut App, r: Result<(), String>) -> Task<Msg> {
    app.admin.busy = false;
    app.admin.message = Some(match r {
        Ok(()) => (
            true,
            "Journaux et rapports exportés. Vous pouvez retirer la clé.".into(),
        ),
        Err(e) => (false, format!("Export impossible : {e}")),
    });
    Task::none()
}

fn am(m: AdminMsg) -> Msg {
    Msg::Admin(m)
}

fn menu_btn<'a>(t: &str, p: Page) -> Element<'a, Msg> {
    bouton(t, am(AdminMsg::Aller(p)), style::secondaire)
}

pub fn view(app: &App) -> Element<'_, Msg> {
    let a = &app.admin;
    let etat = app.etat.clone().unwrap_or(json!({}));
    let msg: Element<Msg> = match &a.message {
        Some((true, m)) => container(texte(m)).padding(12).style(style::carte).into(),
        Some((false, m)) => container(texte_c(m, style::ROUGE))
            .padding(12)
            .style(style::alerte)
            .into(),
        None => Space::new().height(0).into(),
    };
    let retour = bouton(
        "Retour au menu",
        am(AdminMsg::Aller(Page::Menu)),
        style::secondaire,
    );
    let body: Element<Msg> = match &a.page {
        Page::Pin => column![
            titre("Mode administrateur"),
            texte("Saisissez le PIN administrateur."),
            container(text(a.input.shown()).size(34)).padding(12).width(Length::Fixed(360.0)).style(style::carte),
            a.input.view(|k| am(AdminMsg::Key(k))),
            msg,
            row![bouton("Annuler", am(AdminMsg::Quitter), style::secondaire), bouton("Valider", am(AdminMsg::Login), style::primaire)].spacing(30),
        ]
        .spacing(20)
        .align_x(Alignment::Center)
        .into(),
        Page::Menu => column![
            titre("Administration"),
            row![menu_btn("État de la borne", Page::Etat), menu_btn("Mettre à jour", Page::MiseAJour), menu_btn("Exporter les journaux", Page::Export)].spacing(20),
            row![menu_btn("Clés de sortie enrôlées", Page::Enrolement), menu_btn("Limites d'analyse", Page::Limites), menu_btn("Changer le PIN", Page::NouveauPin)].spacing(20),
            row![
                bouton("Redémarrer", am(AdminMsg::Aller(Page::Confirmer("reboot"))), style::danger),
                bouton("Éteindre", am(AdminMsg::Aller(Page::Confirmer("poweroff"))), style::danger),
            ]
            .spacing(20),
            msg,
            bouton("Quitter le mode administrateur", am(AdminMsg::Quitter), style::primaire),
        ]
        .spacing(20)
        .align_x(Alignment::Center)
        .into(),
        Page::Etat => {
            let mut c = Column::new().spacing(6);
            let line = |k: &str, v: String| row![text(format!("{k} :")).size(style::TEXTE_TAILLE).width(Length::Fixed(380.0)), texte(&v)];
            c = c.push(line("Version Barbacane", etat["versions"]["barbacane"].as_str().unwrap_or("?").into()));
            c = c.push(line("Version usbsas", etat["versions"]["usbsas"].as_str().unwrap_or("?").into()));
            c = c.push(line("ClamAV", etat["versions"]["clamav"].as_str().unwrap_or("?").into()));
            c = c.push(line("Âge des signatures", format!("{} jour(s)", etat["signatures_age_days"].as_u64().map(|d| d.to_string()).unwrap_or("?".into()))));
            c = c.push(line("Règles YARA", etat["yara_rules"].as_u64().map(|n| format!("{n} fichier(s)")).unwrap_or("?".into())));
            c = c.push(line("Version du dernier paquet de mise à jour", etat["update_version"].as_u64().map(|n| n.to_string()).unwrap_or("aucun".into())));
            c = c.push(line("Espace disque libre", etat["disk_free"].as_str().unwrap_or("?").into()));
            c = c.push(line("Transferts journalisés", etat["reports"].as_u64().map(|n| n.to_string()).unwrap_or("?".into())));
            column![titre("État de la borne"), carte(c), retour].spacing(20).align_x(Alignment::Center).into()
        }
        Page::MiseAJour => column![
            titre("Mise à jour hors ligne"),
            carte(column![
                texte("1. Branchez la clé administrateur contenant le paquet de mise à jour (*.bcu et *.bcu.minisig) sur le port d'entrée."),
                texte("2. Lancez la lecture : le paquet est lu par usbsas, sa signature et sa version sont vérifiées, puis il est appliqué."),
                petit("Un paquet non signé, corrompu ou plus ancien que la version installée est refusé."),
            ]),
            msg,
            row![retour, bouton_busy("Lire et appliquer", am(AdminMsg::LireMaj), a.busy)].spacing(30),
        ]
        .spacing(20)
        .align_x(Alignment::Center)
        .into(),
        Page::Export => column![
            titre("Exporter les journaux et rapports"),
            carte(column![
                texte("Branchez la clé administrateur sur le port de sortie."),
                texte_c("Elle sera effacée et formatée en exFAT.", style::ROUGE),
            ]),
            msg,
            row![retour, bouton_busy("Exporter", am(AdminMsg::Exporter), a.busy)].spacing(30),
        ]
        .spacing(20)
        .align_x(Alignment::Center)
        .into(),
        Page::Enrolement => {
            let on = etat["enrolled_mode"].as_bool() == Some(true);
            let mut list = Column::new().spacing(8);
            for k in etat["enrolled"].as_array().cloned().unwrap_or_default() {
                list = list.push(
                    row![
                        container(texte(k["description"].as_str().unwrap_or("?"))).width(Length::Fill),
                        bouton("Retirer", am(AdminMsg::Retirer(k.clone())), style::secondaire)
                    ]
                    .spacing(20)
                    .align_y(Alignment::Center),
                );
            }
            let plugged: Vec<UsbDevice> = app.devices.usb.iter().map(|(_, d)| d.clone()).collect();
            let mut add = Column::new().spacing(8);
            for d in plugged {
                add = add.push(bouton(&format!("Enrôler : {}", barbacane_parcours::describe(&d)), am(AdminMsg::Enroler(d)), style::primaire));
            }
            column![
                titre("Clés de sortie enrôlées"),
                row![
                    texte(if on { "Mode clés enrôlées : ACTIVÉ (seules ces clés sont acceptées en sortie)" } else { "Mode clés enrôlées : désactivé (toute clé acceptée en sortie)" }),
                    bouton(if on { "Désactiver" } else { "Activer" }, am(AdminMsg::ModeEnrole(!on)), style::secondaire),
                ]
                .spacing(20)
                .align_y(Alignment::Center),
                container(scrollable(list).height(Length::Fixed(220.0)).width(Length::Fixed(1000.0))).padding(12).style(style::carte),
                petit("Pour enrôler une clé, branchez-la : elle apparaît ci-dessous."),
                add,
                msg,
                retour,
            ]
            .spacing(20)
            .align_x(Alignment::Center)
            .into()
        }
        Page::Limites => {
            let l = &a.limits;
            let ligne = |lib: &str, k: &'static str, v: String| {
                row![
                    text(lib.to_string()).size(style::TEXTE_TAILLE).width(Length::Fixed(420.0)),
                    bouton("−", am(AdminMsg::Limite(k, -1)), style::secondaire),
                    text(v).size(style::TEXTE_TAILLE).width(Length::Fixed(160.0)).center(),
                    bouton("+", am(AdminMsg::Limite(k, 1)), style::secondaire),
                ]
                .spacing(14)
                .align_y(Alignment::Center)
            };
            column![
                titre("Limites d'analyse des archives"),
                carte(column![
                    ligne("Profondeur d'imbrication", "max_depth", l["max_depth"].to_string()),
                    ligne("Nombre de fichiers par transfert", "max_files", l["max_files"].to_string()),
                    ligne(
                        "Taille décompressée par transfert",
                        "max_total_size",
                        format!("{:.1} Go", l["max_total_size"].as_f64().unwrap_or(0.0) / 1073741824.0)
                    ),
                ]),
                msg,
                row![retour, bouton_busy("Enregistrer", am(AdminMsg::EnregistrerLimites), a.busy)].spacing(30),
            ]
            .spacing(20)
            .align_x(Alignment::Center)
            .into()
        }
        Page::NouveauPin => column![
            titre("Changer le PIN"),
            texte(if a.first_pin.is_some() { "Confirmez le nouveau PIN." } else { "Nouveau PIN (6 chiffres minimum)." }),
            container(text(a.input.shown()).size(34)).padding(12).width(Length::Fixed(360.0)).style(style::carte),
            a.input.view(|k| am(AdminMsg::Key(k))),
            msg,
            row![retour, bouton("Valider", am(AdminMsg::EnregistrerPin), style::primaire)].spacing(30),
        ]
        .spacing(20)
        .align_x(Alignment::Center)
        .into(),
        Page::Confirmer(op) => column![
            titre(if *op == "reboot" { "Redémarrer la borne ?" } else { "Éteindre la borne ?" }),
            msg,
            row![retour, bouton("Confirmer", am(AdminMsg::Executer(op)), style::danger)].spacing(30),
        ]
        .spacing(20)
        .align_x(Alignment::Center)
        .into(),
    };
    container(body)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn bouton_busy<'a>(t: &str, m: Msg, busy: bool) -> Element<'a, Msg> {
    if busy {
        iced::widget::button(text("Patientez…").size(style::TEXTE_TAILLE))
            .padding([18, 30])
            .style(style::secondaire)
            .into()
    } else {
        bouton(t, m, style::primaire)
    }
}
