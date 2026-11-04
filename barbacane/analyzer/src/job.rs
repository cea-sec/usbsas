//! État partagé d'une analyse : progression, question de mot de passe en
//! attente, rapport final.

use crate::scan::Entry;
use serde::Serialize;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Processing,
    Scanned,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct PasswordRequest {
    /// Archive concernée (chemin affiché)
    pub archive: String,
    pub attempt: u32,
    pub max_attempts: u32,
}

pub enum Answer {
    Password(Zeroizing<String>),
    Abandon,
    Timeout,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Antivirus {
    pub version: String,
    pub database_version: String,
    pub database_timestamp: String,
}

pub struct State {
    pub status: Status,
    pub error: Option<String>,
    /// Fichiers de la clé analysés / total
    pub done: usize,
    pub total: usize,
    /// Fichiers analysés au total (archives comprises)
    pub scanned: usize,
    pub pending: Option<PasswordRequest>,
    answer: Option<Answer>,
    pub entries: Vec<Entry>,
    pub antivirus: Antivirus,
    pub yara: bool,
    pub started: jiff::Zoned,
    pub finished: Option<jiff::Zoned>,
}

pub struct Job {
    pub id: String,
    pub uid: String,
    pub state: Mutex<State>,
    cond: Condvar,
    pub work: tempfile::TempDir,
}

impl Job {
    pub fn new(uid: &str, work: tempfile::TempDir) -> Self {
        Job {
            id: uuid::Uuid::new_v4().to_string(),
            uid: uid.to_string(),
            state: Mutex::new(State {
                status: Status::Processing,
                error: None,
                done: 0,
                total: 0,
                scanned: 0,
                pending: None,
                answer: None,
                entries: Vec::new(),
                antivirus: Antivirus::default(),
                yara: false,
                started: jiff::Zoned::now(),
                finished: None,
            }),
            cond: Condvar::new(),
            work,
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_progress(&self, done: usize, scanned: usize) {
        let mut s = self.lock();
        if done == 0 {
            s.total = scanned;
        } else {
            s.done = done;
            s.scanned = scanned;
        }
    }

    /// Bloque le fil d'analyse jusqu'à la réponse de l'utilisateur ou l'expiration du délai.
    pub fn ask_password(
        &self,
        archive: &str,
        attempt: u32,
        max_attempts: u32,
        timeout: u64,
    ) -> Answer {
        let mut s = self.lock();
        s.answer = None;
        s.pending = Some(PasswordRequest {
            archive: archive.to_string(),
            attempt,
            max_attempts,
        });
        let deadline = Instant::now() + Duration::from_secs(timeout);
        while s.answer.is_none() {
            let now = Instant::now();
            if now >= deadline {
                s.pending = None;
                return Answer::Timeout;
            }
            s = self
                .cond
                .wait_timeout(s, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        s.pending = None;
        s.answer.take().unwrap_or(Answer::Timeout)
    }

    /// Réponse du client ; `false` si aucune question n'est en attente.
    pub fn answer(&self, a: Answer) -> bool {
        let mut s = self.lock();
        if s.pending.is_none() {
            return false;
        }
        s.answer = Some(a);
        self.cond.notify_all();
        true
    }
}
