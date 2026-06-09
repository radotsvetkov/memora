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

use super::verdict::{self, RenderOpts, VerdictLine};

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

    // The "AI answer": confident output, with one of every kind of bad citation.
    let lines = [
        (
            "We chose \"MessagePack\" for drift's serialization format [claim:0000000000000a01].",
            "0000000000000a01",
        ),
        (
            "The launch is \"set for April 1\" [claim:0000000000000a02].",
            "0000000000000a02",
        ),
        (
            "The stack runs on PostgreSQL with three hot replicas [claim:0000000000dead01].",
            "0000000000dead01",
        ),
        (
            "The API \"uses gRPC streaming\" [claim:0000000000000a03].",
            "0000000000000a03",
        ),
        (
            "Auth \"uses short-lived JWTs\" [claim:0000000000000a04].",
            "0000000000000a04",
        ),
    ];
    let answer_text = lines.iter().map(|(t, _)| *t).collect::<Vec<_>>().join(" ");

    let validator = CitationValidator {
        store: &store,
        index: &index,
        vault_root: &vault,
    };
    let result = validator.validate(&answer_text).await?;

    let status_of = |claim_id: &str| -> CitationStatus {
        result
            .checks
            .iter()
            .find(|c| c.claim_id == claim_id)
            .map(|c| c.status)
            .unwrap_or(CitationStatus::Unverified)
    };
    let verdict_lines: Vec<VerdictLine> = lines
        .iter()
        .map(|(text, id)| VerdictLine {
            text: text.to_string(),
            status: status_of(id),
        })
        .collect();

    println!();
    println!("memora demo — an AI answer, re-checked against the source");
    println!("no API key · no network · nothing mocked (the real validator runs)\n");
    verdict::render_terminal(
        &verdict_lines,
        &result.clean_text,
        &answer_text,
        &RenderOpts {
            color: verdict::color_enabled(),
            show_naive_contrast: true,
        },
    );

    if args.html || args.open {
        let report_path = std::env::temp_dir().join("memora-proof-report.html");
        fs::write(
            &report_path,
            verdict::render_html(&verdict_lines, &result.clean_text),
        )?;
        println!("\nProof Report written to: {}", report_path.display());
        if args.open {
            super::verdict::open_in_browser(&report_path);
        }
    }

    Ok(())
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
    let fingerprint = match seed {
        Seed::StaleFingerprint => Claim::compute_fingerprint(&format!("{span_text} (old)")),
        _ => Claim::compute_fingerprint(span_text),
    };
    let valid_until = match seed {
        Seed::Superseded => Some(Utc::now() - Duration::days(30)),
        _ => None,
    };
    store.upsert(&Claim {
        id: id.to_string(),
        subject: "demo".to_string(),
        predicate: "states".to_string(),
        object: Some("fact".to_string()),
        note_id: note_id.to_string(),
        span_start,
        span_end: span_start + span_text.len(),
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
    })?;
    Ok(())
}
