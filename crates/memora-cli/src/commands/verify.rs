//! `memora verify` — verify the citations in an AI answer against a vault, and
//! exit non-zero if any cannot be proven. This is the building block for "CI for
//! hallucinations": run it in a pipeline and the build fails when a model cites
//! something the source does not contain.
use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

use memora_core::{CitationStatus, Memora};

use super::verdict::{self, RenderOpts, VerdictLine};

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// File containing the AI answer (with [claim:ID] markers). Reads stdin if omitted.
    #[arg(value_name = "file")]
    pub file: Option<PathBuf>,
    /// Vault to verify against.
    #[arg(long, default_value = "vault")]
    pub vault: PathBuf,
    /// Emit machine-readable JSON instead of the human verdict.
    #[arg(long, default_value_t = false)]
    pub json: bool,
    /// Do not fail on superseded citations (provenance intact, claim retracted).
    #[arg(long, default_value_t = false)]
    pub allow_superseded: bool,
}

pub async fn run(args: VerifyArgs) -> Result<()> {
    let text = match &args.file {
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
        }
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("read answer from stdin")?;
            buf
        }
    };

    let memora = Memora::open(&args.vault)?;
    let answer = memora.validate(&text).await?;

    let problems = answer
        .checks
        .iter()
        .filter(|check| match check.status {
            CitationStatus::Verified => false,
            CitationStatus::Superseded => !args.allow_superseded,
            _ => true,
        })
        .count();

    if args.json {
        let checks: Vec<_> = answer
            .checks
            .iter()
            .map(|check| {
                serde_json::json!({
                    "claim_id": check.claim_id,
                    "status": status_str(check.status),
                    "reason": verdict::reason(check.status),
                })
            })
            .collect();
        let out = serde_json::json!({
            "verified": answer.verified_count,
            "unverified": answer.unverified_count,
            "mismatch": answer.mismatch_count,
            "superseded": answer.superseded_count,
            "problems": problems,
            "clean_text": answer.clean_text,
            "checks": checks,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else if answer.checks.is_empty() {
        println!("No [claim:...] citations found in the input. Nothing to verify.");
    } else {
        let lines: Vec<VerdictLine> = answer
            .checks
            .iter()
            .map(|check| VerdictLine {
                text: verdict::sentence_for_marker(&text, &check.claim_id),
                status: check.status,
            })
            .collect();
        verdict::render_terminal(
            &lines,
            &answer.clean_text,
            &text,
            &RenderOpts {
                color: verdict::color_enabled(),
                show_naive_contrast: false,
            },
        );
    }

    if problems > 0 {
        let _ = std::io::stdout().flush();
        // Non-zero exit so a CI step fails when a citation cannot be proven.
        std::process::exit(1);
    }
    Ok(())
}

fn status_str(status: CitationStatus) -> &'static str {
    match status {
        CitationStatus::Verified => "verified",
        CitationStatus::Unverified => "unverified",
        CitationStatus::FingerprintMismatch => "fingerprint_mismatch",
        CitationStatus::QuoteMismatch => "quote_mismatch",
        CitationStatus::Superseded => "superseded",
    }
}
