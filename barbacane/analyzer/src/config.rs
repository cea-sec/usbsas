//! Configuration de l'analyseur (`/etc/barbacane/analyzer.toml`).

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Adresse d'écoute (127.0.0.1:8042 par défaut ; autre adresse = mode serveur)
    pub listen: String,
    /// Répertoire de travail (tmpfs de taille limitée en production)
    pub work_dir: PathBuf,
    /// Socket clamd
    pub clamd_socket: PathBuf,
    /// Répertoire des règles YARA (*.yar, *.yara), facultatif
    pub yara_rules_dir: Option<PathBuf>,
    /// Exécutable 7-Zip
    pub seven_zip: PathBuf,
    /// Exécutable bubblewrap ; vide = pas de bac à sable (tests uniquement)
    pub bwrap: Option<PathBuf>,
    /// Dossier de préparation lu par usbsas (`[[src_local_dirs]]`), mode local uniquement
    pub stage_dir: PathBuf,
    /// Rapports JSON détaillés conservés localement
    pub reports_dir: Option<PathBuf>,
    /// Paquets de mise à jour lus sur la clé admin (après armement par barbacane-admind)
    pub import_dir: PathBuf,
    /// Nombre de rapports locaux conservés (rotation)
    pub max_reports: usize,
    pub limits: Limits,
    pub server: Option<Server>,
    pub relay: Option<Relay>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Profondeur d'imbrication maximale des archives
    pub max_depth: u32,
    /// Nombre maximal de fichiers extraits par transfert
    pub max_files: u64,
    /// Taille totale décompressée maximale par transfert (octets)
    pub max_total_size: u64,
    /// Rapport taille décompressée / taille du conteneur maximal
    pub max_ratio: u64,
    /// Durée maximale d'une extraction (secondes)
    pub extract_timeout: u64,
    /// Mémoire maximale d'un processus d'extraction (octets)
    pub extract_memory: u64,
    /// Délai d'attente d'un mot de passe (secondes)
    pub password_timeout: u64,
    /// Nombre d'essais de mot de passe par archive
    pub password_attempts: u32,
}

/// Mode serveur partagé entre plusieurs SAS (désactivé si absent).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    /// Fichier listant les jetons acceptés, un par ligne : `<jeton> <nom de la borne>`
    pub tokens_file: PathBuf,
}

/// Borne reliée à un analyseur central (désactivé si absent).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relay {
    /// URL du serveur central, ex. `https://analyse.interne:8443`
    pub upstream: String,
    /// Jeton de la borne (une ligne)
    pub token_file: PathBuf,
    /// Certificat du serveur (ou de son AC) à épingler
    pub ca_cert: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            listen: "127.0.0.1:8042".into(),
            work_dir: "/var/lib/barbacane/analyzer".into(),
            clamd_socket: "/run/clamav/clamd.ctl".into(),
            yara_rules_dir: Some("/var/lib/barbacane/yara".into()),
            seven_zip: "/usr/bin/7z".into(),
            bwrap: Some("/usr/bin/bwrap".into()),
            stage_dir: "/var/lib/barbacane/sortie".into(),
            reports_dir: Some("/var/lib/barbacane/rapports".into()),
            import_dir: "/var/lib/barbacane/import".into(),
            max_reports: 5000,
            limits: Limits::default(),
            server: None,
            relay: None,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_depth: 5,
            max_files: 10_000,
            max_total_size: 4 * 1024 * 1024 * 1024,
            max_ratio: 100,
            extract_timeout: 300,
            extract_memory: 2 * 1024 * 1024 * 1024,
            password_timeout: 300,
            password_attempts: 3,
        }
    }
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(toml::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::warn!("{path} absent, configuration par défaut");
                Ok(Config::default())
            }
            Err(e) => Err(e.into()),
        }
    }
}
