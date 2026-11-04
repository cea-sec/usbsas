//! Configuration du client (`/etc/barbacane/client.toml`).

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Dossier de la socket du service usbsas-server
    pub usbsas_socket: String,
    pub analyzer_url: String,
    /// Socket du service d'administration (barbacane-admind)
    pub admin_socket: String,
    /// Dossier de préparation (même chemin que dans `[[src_local_dirs]]` d'usbsas)
    pub stage_dir: String,
    /// Dossier d'export des journaux (même chemin que dans `[[src_local_dirs]]` d'usbsas)
    pub export_dir: String,
    pub title: String,
    /// Âge des signatures antivirales au-delà duquel une alerte s'affiche (jours)
    pub signatures_max_age_days: u64,
    /// Identification par badge (prévu, non disponible)
    pub badge: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            usbsas_socket: "/run/usbsas".into(),
            analyzer_url: "http://127.0.0.1:8042".into(),
            admin_socket: "/run/barbacane/admin.sock".into(),
            stage_dir: "/var/lib/barbacane/sortie".into(),
            export_dir: "/var/lib/barbacane/export".into(),
            title: "Barbacane".into(),
            signatures_max_age_days: 7,
            badge: false,
        }
    }
}

impl Config {
    pub fn load(path: &str) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| panic!("{path} : {e}")),
            Err(_) => {
                log::warn!("{path} absent, configuration par défaut");
                Config::default()
            }
        }
    }
}
