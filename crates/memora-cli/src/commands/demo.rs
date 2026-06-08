//! `memora demo` — a zero-config, no-API-key, offline demonstration of the one
//! thing Memora does that nothing else does: re-read the source behind every
//! citation and reject what it can't prove.
//!
//! It builds an ephemeral vault with a known corpus, feeds the *real* validator a
//! pre-written "AI answer" that contains one of every kind of bad citation, and
//! renders the verdict. Nothing here is mocked: the rejections come from the same
//! `CitationValidator` the query pipeline uses.
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Duration, TimeZone, Utc};
use clap::Args;

use memora_core::{
    CitationStatus, CitationValidator, Claim, ClaimStore, Frontmatter, Index, Note, NoteSource,
    Privacy,
};

#[derive(Debug, Args)]
pub struct DemoArgs {
    /// Write a standalone HTML "Proof Report" and print its path.
    #[arg(long, default_value_t = false)]
    pub html: bool,
    /// Open the HTML Proof Report in your browser (implies --html).
    #[arg(long, default_value_t = false)]
    pub open: bool,
}

/// What each claim is, so we can author a deterministic, all-outcomes demo.
enum Seed {
    Faithful,
    /// Stored fingerprint won't match the source span (simulates a post-extraction edit).
    StaleFingerprint,
    /// Provenance intact, but the claim was retracted (valid_until in the past).
    Superseded,
}

pub async fn run(args: DemoArgs) -> Result<()> {
    let temp = tempfile::tempdir().context("create temp demo vault")?;
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault)?;
    let index = Index::open(&temp.path().join("index.db"))?;
    let store = ClaimStore::new(&index);

    let body = "We chose MessagePack for drift's serialization format. \
The launch is set for March 15. The API uses gRPC streaming. \
Auth uses short-lived JWTs.";
    seed_note(&vault, &index, "decisions.md", "decisions", body)?;

    seed_claim(
        &store,
        "0000000000000a01",
        "decisions",
        body,
        "We chose MessagePack for drift's serialization format",
        Seed::Faithful,
    )?;
    seed_claim(
        &store,
        "0000000000000a02",
        "decisions",
        body,
        "The launch is set for March 15",
        Seed::Faithful,
    )?;
    seed_claim(
        &store,
        "0000000000000a03",
        "decisions",
        body,
        "The API uses gRPC streaming",
        Seed::StaleFingerprint,
    )?;
    seed_claim(
        &store,
        "0000000000000a04",
        "decisions",
        body,
        "Auth uses short-lived JWTs",
        Seed::Superseded,
    )?;

    // The "AI answer" — exactly the kind of confident output you'd get from a model.
    // Each line cites a claim; together they exercise every verification outcome.
    let lines = [
        Line::new(
            "We chose \"MessagePack\" for drift's serialization format [claim:0000000000000a01].",
            "0000000000000a01",
        ),
        Line::new(
            "The launch is \"set for April 1\" [claim:0000000000000a02].",
            "0000000000000a02",
        ),
        Line::new(
            "The stack runs on PostgreSQL with three hot replicas [claim:0000000000dead01].",
            "0000000000dead01",
        ),
        Line::new(
            "The API \"uses gRPC streaming\" [claim:0000000000000a03].",
            "0000000000000a03",
        ),
        Line::new(
            "Auth \"uses short-lived JWTs\" [claim:0000000000000a04].",
            "0000000000000a04",
        ),
    ];

    let answer_text = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    let validator = CitationValidator {
        store: &store,
        index: &index,
        vault_root: &vault,
    };
    let result = validator.validate(&answer_text).await?;

    // Map each cited claim id to its verdict (each id appears once).
    let status_of = |claim_id: &str| -> CitationStatus {
        result
            .checks
            .iter()
            .find(|c| c.claim_id == claim_id)
            .map(|c| c.status)
            .unwrap_or(CitationStatus::Unverified)
    };
    let source_of = |claim_id: &str| -> Option<String> {
        result
            .checks
            .iter()
            .find(|c| c.claim_id == claim_id)
            .and_then(|c| c.source_text.clone())
    };

    let verdicts: Vec<Verdict> = lines
        .iter()
        .map(|l| Verdict {
            text: l.text.clone(),
            status: status_of(&l.claim_id),
            source: source_of(&l.claim_id),
        })
        .collect();

    render_terminal(&verdicts, &result.clean_text, &answer_text);

    if args.html || args.open {
        let path = temp
            .path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("memora-proof-report.html");
        // tempdir is removed on drop, so write the report outside it.
        let report_path = std::env::temp_dir().join("memora-proof-report.html");
        fs::write(
            &report_path,
            render_html(&verdicts, &result.clean_text, &answer_text),
        )?;
        println!("\nProof Report written to: {}", report_path.display());
        let _ = path; // keep intent clear; we use a stable temp path
        if args.open {
            open_in_browser(&report_path);
        }
    }

    Ok(())
}

struct Line {
    text: String,
    claim_id: String,
}

impl Line {
    fn new(text: &str, claim_id: &str) -> Self {
        Self {
            text: text.to_string(),
            claim_id: claim_id.to_string(),
        }
    }
}

struct Verdict {
    text: String,
    status: CitationStatus,
    source: Option<String>,
}

fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

fn c(code: &str, s: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn strike(s: &str) -> String {
    if color_enabled() {
        format!("\x1b[9m\x1b[31m{s}\x1b[0m")
    } else {
        format!("[REJECTED] {s}")
    }
}

/// Plain-English reason a citation was rejected (or flagged).
fn reason(status: CitationStatus) -> &'static str {
    match status {
        CitationStatus::Verified => "source verbatim contains this; hash matches",
        CitationStatus::Unverified => "cited a claim id that does not exist (hallucinated)",
        CitationStatus::FingerprintMismatch => {
            "source span changed since extraction — hash mismatch"
        }
        CitationStatus::QuoteMismatch => "quoted text is not in the cited source span",
        CitationStatus::Superseded => "claim was superseded/retracted (valid_until has passed)",
    }
}

fn badge(status: CitationStatus) -> String {
    match status {
        CitationStatus::Verified => c("1;32", "✓ VERIFIED  "),
        CitationStatus::Superseded => c("1;33", "⚠ SUPERSEDED"),
        _ => c("1;31", "✗ REJECTED  "),
    }
}

fn render_terminal(verdicts: &[Verdict], clean_text: &str, raw_text: &str) {
    let mut verified = 0usize;
    let mut rejected = 0usize;
    let mut superseded = 0usize;

    println!();
    println!(
        "{}",
        c(
            "1",
            "memora demo — an AI answer, re-checked against the source"
        )
    );
    println!(
        "{}",
        c(
            "2",
            "no API key · no network · nothing mocked (the real validator runs)"
        )
    );
    println!();

    for v in verdicts {
        match v.status {
            CitationStatus::Verified => verified += 1,
            CitationStatus::Superseded => superseded += 1,
            _ => rejected += 1,
        }
        println!("{}  {}", badge(v.status), c("2", reason(v.status)));
        let shown = match v.status {
            CitationStatus::Verified => c("32", &v.text),
            CitationStatus::Superseded => c("33", &v.text),
            _ => strike(&v.text),
        };
        println!("   {shown}");
        if v.status == CitationStatus::Verified {
            if let Some(src) = &v.source {
                println!("   {} {}", c("2", "source:"), c("2", src));
            }
        }
        println!();
    }

    println!("{}", c("1", "Verdict"));
    println!(
        "  {}   {}   {}",
        c("1;32", &format!("{verified} verified")),
        c("1;31", &format!("{rejected} rejected")),
        c("1;33", &format!("{superseded} superseded")),
    );
    println!();
    println!(
        "{}",
        c(
            "2",
            "A naive RAG/prompt-cite tool would have shown you all of this as fact:"
        )
    );
    println!("   {}", c("2", &one_line(raw_text)));
    println!();
    println!(
        "{}",
        c(
            "1",
            "What memora actually returns (only what it could prove):"
        )
    );
    let clean = if clean_text.trim().is_empty() {
        "(nothing — none of the citations could be verified)".to_string()
    } else {
        one_line(clean_text)
    };
    println!("   {}", c("32", &clean));
    println!();
    println!(
        "{}",
        c("2", "Run `memora demo --open` for an HTML Proof Report. This is the same check `memora query` runs on every answer.")
    );
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn render_html(verdicts: &[Verdict], clean_text: &str, raw_text: &str) -> String {
    let mut cards = String::new();
    for v in verdicts {
        let (cls, label) = match v.status {
            CitationStatus::Verified => ("ok", "VERIFIED"),
            CitationStatus::Superseded => ("warn", "SUPERSEDED"),
            CitationStatus::Unverified => ("bad", "REJECTED — hallucinated id"),
            CitationStatus::FingerprintMismatch => ("bad", "REJECTED — hash mismatch"),
            CitationStatus::QuoteMismatch => ("bad", "REJECTED — quote not in source"),
        };
        let text = html_escape(&v.text);
        let text = if matches!(
            v.status,
            CitationStatus::Verified | CitationStatus::Superseded
        ) {
            text
        } else {
            format!("<s>{text}</s>")
        };
        let src = v
            .source
            .as_deref()
            .map(|s| format!("<div class=\"src\">source: {}</div>", html_escape(s)))
            .unwrap_or_default();
        cards.push_str(&format!(
            "<div class=\"card {cls}\"><span class=\"label\">{label}</span><div class=\"sentence\">{text}</div><div class=\"reason\">{}</div>{src}</div>\n",
            html_escape(reason(v.status))
        ));
    }
    let clean = if clean_text.trim().is_empty() {
        "(nothing — none of the citations could be verified)".to_string()
    } else {
        html_escape(&one_line(clean_text))
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Memora Proof Report</title>\
<style>\
:root{{color-scheme:dark}}body{{font:16px/1.5 -apple-system,Segoe UI,Roboto,sans-serif;background:#0b0f14;color:#e6edf3;max-width:760px;margin:40px auto;padding:0 20px}}\
h1{{font-size:22px}}.sub{{color:#8b949e;margin-bottom:24px}}\
.card{{border-left:4px solid #30363d;background:#11161d;border-radius:8px;padding:14px 16px;margin:12px 0}}\
.card.ok{{border-color:#2ea043}}.card.bad{{border-color:#f85149}}.card.warn{{border-color:#d29922}}\
.label{{font-size:12px;font-weight:700;letter-spacing:.04em}}.card.ok .label{{color:#3fb950}}.card.bad .label{{color:#ff7b72}}.card.warn .label{{color:#e3b341}}\
.sentence{{margin:6px 0;font-size:17px}}.sentence s{{color:#ff7b72;text-decoration-color:#f85149}}\
.reason{{color:#8b949e;font-size:14px}}.src{{color:#6e7681;font-size:13px;margin-top:6px;font-family:ui-monospace,monospace}}\
.final{{margin-top:28px;padding:16px;background:#0d1117;border:1px solid #30363d;border-radius:8px}}\
.final .k{{color:#8b949e;font-size:13px}}.final .v{{color:#3fb950;margin-top:6px}}\
</style></head><body>\
<h1>Memora Proof Report</h1>\
<div class=\"sub\">Every citation in an AI answer, re-read against the source span and re-hashed. No API key, nothing mocked.</div>\
{cards}\
<div class=\"final\"><div class=\"k\">A naive tool returns all of the above as fact. memora returns only what it could prove:</div><div class=\"v\">{clean}</div></div>\
<p class=\"sub\" style=\"margin-top:24px\">Raw model answer: {}</p>\
</body></html>",
        html_escape(&one_line(raw_text))
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn open_in_browser(path: &Path) {
    let cmd = if cfg!(target_os = "macos") {
        Some(("open", vec![path.as_os_str().to_owned()]))
    } else if cfg!(target_os = "windows") {
        Some((
            "cmd",
            vec!["/C".into(), "start".into(), path.as_os_str().to_owned()],
        ))
    } else {
        Some(("xdg-open", vec![path.as_os_str().to_owned()]))
    };
    if let Some((bin, cmd_args)) = cmd {
        let _ = std::process::Command::new(bin).args(cmd_args).status();
    }
}

fn seed_note(vault: &Path, index: &Index, rel: &str, id: &str, body: &str) -> Result<()> {
    let rel_path = PathBuf::from(rel);
    let note = Note {
        path: rel_path.clone(),
        fm: Frontmatter {
            id: id.to_string(),
            region: "demo/decisions".to_string(),
            source: NoteSource::Personal,
            privacy: Privacy::Private,
            created: Utc
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("date"),
            updated: Utc
                .with_ymd_and_hms(2026, 1, 2, 0, 0, 0)
                .single()
                .expect("date"),
            summary: "memora demo".to_string(),
            tags: Vec::new(),
            refs: Vec::new(),
        },
        body: body.to_string(),
        wikilinks: Vec::new(),
    };
    let content = format!(
        "---\nid: {id}\nregion: demo/decisions\nsource: personal\nprivacy: private\ncreated: 2026-01-01T00:00:00Z\nupdated: 2026-01-02T00:00:00Z\nsummary: \"memora demo\"\ntags: []\nrefs: []\n---\n{body}\n"
    );
    fs::write(vault.join(&rel_path), content)?;
    index.upsert_note(&note, body)?;
    Ok(())
}

fn seed_claim(
    store: &ClaimStore,
    id: &str,
    note_id: &str,
    body: &str,
    span_text: &str,
    seed: Seed,
) -> Result<()> {
    let span_start = body
        .find(span_text)
        .with_context(|| format!("span '{span_text}' not found in demo body"))?;
    let span_end = span_start + span_text.len();
    let fingerprint = match seed {
        Seed::StaleFingerprint => Claim::compute_fingerprint(&format!("{span_text} (old)")),
        _ => Claim::compute_fingerprint(span_text),
    };
    let valid_until = match seed {
        Seed::Superseded => Some(Utc::now() - Duration::days(30)),
        _ => None,
    };
    let claim = Claim {
        id: id.to_string(),
        subject: "demo".to_string(),
        predicate: "states".to_string(),
        object: Some("fact".to_string()),
        note_id: note_id.to_string(),
        span_start,
        span_end,
        span_fingerprint: fingerprint,
        valid_from: Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("date"),
        valid_until,
        confidence: 1.0,
        privacy: Privacy::Private,
        extracted_by: "demo".to_string(),
        extracted_at: Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("date"),
    };
    store.upsert(&claim)?;
    Ok(())
}
