//! `memora verify` — verify the citations in an AI answer against a vault, and
//! exit non-zero if any cannot be proven. This is the building block for "CI for
//! hallucinations": run it in a pipeline and the build fails when a model cites
//! something the source does not contain.
use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

use memora_core::{CitationStatus, Entailment, Memora, Privacy};

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
    /// Also run an optional, LLM-judged entailment check on verified citations:
    /// does the source actually support the assertion? Best-effort, not hash-proven.
    /// Needs an LLM (cloud providers require MEMORA_ENABLE_NETWORK_LLM=1).
    #[arg(long, default_value_t = false)]
    pub entailment: bool,
    /// With --entailment, treat unsupported citations as failures (exit non-zero).
    #[arg(long, default_value_t = false)]
    pub fail_unsupported: bool,
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

    // Optional entailment pass (best-effort, LLM-judged): for citations whose
    // provenance is intact, ask whether the source actually supports the
    // assertion. Maps claim_id -> verdict.
    let mut entailment: std::collections::HashMap<String, Entailment> =
        std::collections::HashMap::new();
    let mut unsupported = 0usize;
    if args.entailment {
        let checker = memora.entailment_checker()?;
        for check in &answer.checks {
            if !matches!(
                check.status,
                CitationStatus::Verified | CitationStatus::Superseded
            ) {
                continue;
            }
            let premise = check.source_text.clone().unwrap_or_default();
            let hypothesis = match &check.quote {
                Some(q) if !q.trim().is_empty() => q.clone(),
                _ => claim_assertion(&memora, &check.claim_id)?,
            };
            let privacy = memora
                .claim(&check.claim_id)?
                .map(|c| c.privacy)
                .unwrap_or(Privacy::Private);
            let verdict = checker.check(&premise, &hypothesis, privacy).await?;
            if verdict == Entailment::Unsupported {
                unsupported += 1;
            }
            entailment.insert(check.claim_id.clone(), verdict);
        }
    }

    if args.json {
        let checks: Vec<_> = answer
            .checks
            .iter()
            .map(|check| {
                let mut obj = serde_json::json!({
                    "claim_id": check.claim_id,
                    "status": status_str(check.status),
                    "reason": verdict::reason(check.status),
                });
                if let Some(verdict) = entailment.get(&check.claim_id) {
                    obj["entailment"] = serde_json::Value::String(verdict.as_str().to_string());
                }
                obj
            })
            .collect();
        let mut out = serde_json::json!({
            "verified": answer.verified_count,
            "unverified": answer.unverified_count,
            "mismatch": answer.mismatch_count,
            "superseded": answer.superseded_count,
            "problems": problems,
            "clean_text": answer.clean_text,
            "checks": checks,
        });
        if args.entailment {
            out["entailment_checked"] = serde_json::Value::Bool(true);
            out["unsupported"] = serde_json::Value::from(unsupported);
        }
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

    if args.entailment && !args.json && !entailment.is_empty() {
        let entailed = entailment
            .values()
            .filter(|v| **v == Entailment::Entailed)
            .count();
        let unchecked = entailment
            .values()
            .filter(|v| **v == Entailment::Unchecked)
            .count();
        println!();
        println!("Entailment (LLM-judged, best-effort — the provenance above is hash-proven):");
        for check in &answer.checks {
            if let Some(verdict) = entailment.get(&check.claim_id) {
                let (mark, label) = match verdict {
                    Entailment::Entailed => ("✓", "entailed   "),
                    Entailment::Unsupported => ("✗", "unsupported"),
                    Entailment::Unchecked => ("·", "unchecked  "),
                };
                println!("  {mark} {label}  [claim:{}]", check.claim_id);
            }
        }
        println!("{entailed} entailed, {unsupported} unsupported, {unchecked} unchecked");
    }

    // Non-zero exit so a CI step fails when a citation cannot be proven. With
    // --fail-unsupported, an LLM-judged "unsupported" verdict also fails the run.
    let fail = problems > 0 || (args.fail_unsupported && unsupported > 0);
    if fail {
        let _ = std::io::stdout().flush();
        std::process::exit(1);
    }
    Ok(())
}

/// The claim's `subject predicate object` as a single assertion string, used as
/// the entailment hypothesis when the answer has no explicit quote.
fn claim_assertion(memora: &Memora, claim_id: &str) -> Result<String> {
    Ok(match memora.claim(claim_id)? {
        Some(claim) => format!(
            "{} {} {}",
            claim.subject,
            claim.predicate,
            claim.object.as_deref().unwrap_or("")
        )
        .trim()
        .to_string(),
        None => String::new(),
    })
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
