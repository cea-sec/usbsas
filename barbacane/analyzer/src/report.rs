//! Rapports : JSON pour usbsas (protocole v2), JSON détaillé, texte et HTML
//! lisibles écrits sur la clé de sortie.

use crate::job::{Job, State, Status};
use crate::scan::{Entry, Verdict, human};
use serde_json::{Value, json};

/// Réponse attendue par usbsas (`usbsas-net/src/analyzer.rs`, `AnalyzeReport`).
/// Seuls les fichiers de la clé source y figurent, clés sans `/` initial.
pub fn usbsas(job: &Job, s: &State) -> Value {
    if s.status != Status::Scanned {
        return json!({"id": job.id, "status": s.status});
    }
    let files: serde_json::Map<String, Value> = s
        .entries
        .iter()
        .filter(|e| e.container.is_none())
        .map(|e| {
            let st = if e.verdict == Verdict::Clean {
                "CLEAN"
            } else {
                "DIRTY"
            };
            (e.path.clone(), json!({"status": st, "sha256": e.sha256}))
        })
        .collect();
    json!({
        "id": job.id,
        "status": "scanned",
        "version": 2,
        "files": files,
        "antivirus": {"ClamAV": s.antivirus},
    })
}

pub fn detailed(job: &Job, s: &State) -> Value {
    let copied = s.entries.iter().filter(|e| e.output.is_some()).count();
    let refused = s
        .entries
        .iter()
        .filter(|e| is_refusal(e, &s.entries))
        .count();
    json!({
        "id": job.id,
        "user_id": job.uid,
        "status": s.status,
        "error": s.error,
        "started": s.started.to_string(),
        "finished": s.finished.as_ref().map(|t| t.to_string()),
        "antivirus": {"ClamAV": s.antivirus, "YARA": s.yara},
        "summary": {"files": s.entries.len(), "copied": copied, "refused": refused},
        "files": s.entries,
    })
}

/// Archive refusée dont le contenu a été extrait (chaque fichier a son propre verdict).
pub fn is_unpacked(e: &Entry, all: &[Entry]) -> bool {
    e.verdict == Verdict::Dirty && all.iter().any(|c| c.container.as_deref() == Some(&e.path))
}

/// Un refus à signaler à l'utilisateur.
pub fn is_refusal(e: &Entry, all: &[Entry]) -> bool {
    e.verdict == Verdict::Dirty && !is_unpacked(e, all)
}

pub fn text(job: &Job, s: &State, device: &str) -> String {
    let mut t = String::new();
    t += "BARBACANE — RAPPORT DE TRANSFERT\n================================\n\n";
    t += &format!(
        "Date            : {}\n",
        s.started.strftime("%d/%m/%Y %H:%M:%S")
    );
    t += &format!("Clé source      : {device}\n");
    t += &format!("Identifiant     : {}\n", job.id);
    t += &format!(
        "Antivirus       : ClamAV {} (base {} du {}){}\n\n",
        s.antivirus.version,
        s.antivirus.database_version,
        s.antivirus.database_timestamp,
        if s.yara { " + YARA" } else { "" }
    );
    let copied: Vec<_> = s.entries.iter().filter(|e| e.output.is_some()).collect();
    let refused: Vec<_> = s
        .entries
        .iter()
        .filter(|e| is_refusal(e, &s.entries))
        .collect();
    t += &format!("FICHIERS COPIÉS ({})\n", copied.len());
    for e in &copied {
        t += &format!(
            "  [OK]     {}  ({})\n",
            e.output.as_deref().unwrap_or_default(),
            human(e.size)
        );
    }
    t += &format!("\nFICHIERS REFUSÉS ({})\n", refused.len());
    for e in &refused {
        t += &format!("  [REFUSÉ] {}\n           raison : {}\n", e.path, e.reason);
    }
    let unpacked: Vec<_> = s
        .entries
        .iter()
        .filter(|e| is_unpacked(e, &s.entries))
        .collect();
    if !unpacked.is_empty() {
        t += &format!(
            "\nARCHIVES NON COPIÉES, REMPLACÉES PAR LEUR CONTENU SAIN ({})\n",
            unpacked.len()
        );
        for e in &unpacked {
            t += &format!(
                "  [ARCHIVE] {}  →  {}.extrait/\n            raison : {}\n",
                e.path, e.path, e.reason
            );
        }
    }
    t += "\nLes fichiers refusés ne sont pas copiés. En cas de doute, contactez votre administrateur.\n";
    t
}

pub fn html(job: &Job, s: &State, device: &str) -> String {
    let esc = |x: &str| {
        x.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let mut rows_ok = String::new();
    let mut rows_ko = String::new();
    let mut rows_ar = String::new();
    for e in &s.entries {
        if let Some(o) = &e.output {
            rows_ok += &format!(
                "<tr><td>{}</td><td>{}</td><td><code>{}</code></td></tr>\n",
                esc(o),
                human(e.size),
                e.sha256
            );
        } else if is_unpacked(e, &s.entries) {
            rows_ar += &format!(
                "<tr><td>{}</td><td>{}</td></tr>\n",
                esc(&e.path),
                esc(&e.reason)
            );
        } else if is_refusal(e, &s.entries) {
            rows_ko += &format!(
                "<tr><td>{}</td><td>{}</td></tr>\n",
                esc(&e.path),
                esc(&e.reason)
            );
        }
    }
    format!(
        r#"<!doctype html><html lang="fr"><head><meta charset="utf-8"><title>Rapport Barbacane</title>
<style>body{{font-family:sans-serif;margin:2em;color:#1b1b1b}}h1{{font-size:1.4em}}table{{border-collapse:collapse;width:100%;margin-bottom:2em}}
td,th{{border:1px solid #ccc;padding:.3em .6em;text-align:left;vertical-align:top}}th{{background:#eee}}.ok{{color:#0a6b2d}}.ko{{color:#b00020}}code{{font-size:.8em}}</style></head>
<body><h1>Barbacane — rapport de transfert</h1>
<p>Date : {date}<br>Clé source : {dev}<br>Identifiant : {id}<br>Antivirus : ClamAV {av} (base {db} du {dbd}){yara}</p>
<h2 class="ok">Fichiers copiés</h2><table><tr><th>Fichier</th><th>Taille</th><th>SHA-256</th></tr>
{rows_ok}</table>
<h2 class="ko">Fichiers refusés</h2><table><tr><th>Fichier</th><th>Raison</th></tr>
{rows_ko}</table>
<h2>Archives non copiées, remplacées par leur contenu sain</h2><table><tr><th>Archive</th><th>Raison</th></tr>
{rows_ar}</table></body></html>
"#,
        date = s.started.strftime("%d/%m/%Y %H:%M:%S"),
        dev = esc(device),
        id = job.id,
        av = esc(&s.antivirus.version),
        db = esc(&s.antivirus.database_version),
        dbd = esc(&s.antivirus.database_timestamp),
        yara = if s.yara { " + YARA" } else { "" },
    )
}
