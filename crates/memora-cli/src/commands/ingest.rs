//! `memora ingest` — normalize an external document into a vault note so it flows
//! through the existing index -> extract -> verify pipeline. memora's whole model
//! is byte-span verification over markdown, so ingestion is just: turn the source
//! into clean markdown text, write it as a note with valid frontmatter, and let
//! `memora index` pick it up. Supported: PDF (with the `pdf` feature), plain text,
//! markdown, and VTT/SRT transcripts.
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use clap::Args;

use memora_core::note::{rewrite_with_frontmatter, Frontmatter, NoteSource, Privacy};
use memora_core::vault_path::validate_region;

#[derive(Debug, Args)]
pub struct IngestArgs {
    /// File to ingest: .pdf (needs the `pdf` feature), .txt, .md, .markdown, .vtt, .srt.
    #[arg(value_name = "file")]
    pub file: PathBuf,
    /// Vault to ingest into.
    #[arg(long, default_value = "vault")]
    pub vault: PathBuf,
    /// Region (folder) to place the note under.
    #[arg(long, default_value = "ingested")]
    pub region: String,
    /// Privacy band for the note: public | private | secret.
    #[arg(long, default_value = "private")]
    pub privacy: String,
}

pub fn run(args: IngestArgs) -> Result<()> {
    if !args.file.is_file() {
        bail!("file not found: {}", args.file.display());
    }
    let privacy: Privacy = args.privacy.parse().map_err(|_| {
        anyhow!(
            "invalid --privacy '{}' (use public|private|secret)",
            args.privacy
        )
    })?;

    let ext = args
        .file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let body = clean_text(&extract_text(&args.file, &ext)?);
    if body.trim().is_empty() {
        bail!("no text could be extracted from {}", args.file.display());
    }

    let stem = args
        .file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let id = format!("{}-{}", slugify(stem), short_hash(&args.file));
    let summary = derive_summary(&body, stem);

    let region_dir =
        validate_region(&args.vault, &args.region).map_err(|e| anyhow!("invalid --region: {e}"))?;
    std::fs::create_dir_all(&region_dir)
        .with_context(|| format!("create region dir {}", region_dir.display()))?;
    let note_path = region_dir.join(format!("{id}.md"));

    let now = Utc::now();
    let frontmatter = Frontmatter {
        id: id.clone(),
        region: args.region.clone(),
        source: NoteSource::Reference,
        privacy,
        created: now,
        updated: now,
        summary,
        tags: vec!["ingested".to_string(), ext.clone()],
        refs: Vec::new(),
    };
    rewrite_with_frontmatter(&note_path, &frontmatter, &body)
        .map_err(|e| anyhow!("write note: {e}"))?;

    println!(
        "Ingested {} -> {}",
        args.file.display(),
        note_path.display()
    );
    println!(
        "Next: memora index --vault {}   (extracts claims and makes it verifiable)",
        args.vault.display()
    );
    Ok(())
}

fn extract_text(path: &Path, ext: &str) -> Result<String> {
    match ext {
        "txt" | "text" | "md" | "markdown" => {
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
        }
        "vtt" | "srt" => Ok(strip_subtitles(
            &std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?,
        )),
        "pdf" => extract_pdf(path),
        other => bail!(
            "unsupported file type '.{other}'. Supported: pdf (with the `pdf` feature), \
             txt, md, markdown, vtt, srt"
        ),
    }
}

#[cfg(feature = "pdf")]
fn extract_pdf(path: &Path) -> Result<String> {
    pdf_extract::extract_text(path)
        .with_context(|| format!("extract text from PDF {}", path.display()))
}

#[cfg(not(feature = "pdf"))]
fn extract_pdf(_path: &Path) -> Result<String> {
    bail!(
        "PDF ingestion needs memora built with the `pdf` feature \
         (e.g. `cargo install memora-cli --features pdf`). Or convert the PDF to \
         text/markdown first and ingest that."
    )
}

/// Normalize extracted text: drop carriage returns and form feeds, and collapse
/// runs of blank lines into a single paragraph break.
fn clean_text(raw: &str) -> String {
    let unified = raw.replace('\u{c}', "\n").replace('\r', "");
    let mut out = String::with_capacity(unified.len());
    let mut blanks = 0usize;
    for line in unified.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blanks += 1;
            if blanks == 1 {
                out.push('\n');
            }
        } else {
            blanks = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

/// A filesystem-safe, readable id stem from the source filename.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let slug: String = out.trim_matches('-').chars().take(48).collect();
    if slug.is_empty() {
        "document".to_string()
    } else {
        slug
    }
}

/// Stable short id suffix derived from the source path, so re-ingesting the same
/// file updates the same note rather than creating a duplicate.
fn short_hash(path: &Path) -> String {
    blake3::hash(path.to_string_lossy().as_bytes())
        .to_hex()
        .to_string()
        .chars()
        .take(6)
        .collect()
}

fn derive_summary(body: &str, stem: &str) -> String {
    let first = body.lines().map(str::trim).find(|l| !l.is_empty());
    let base = match first {
        Some(line) if !line.is_empty() => line,
        _ => stem,
    };
    base.chars()
        .take(120)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Strip VTT/SRT cue numbers, timestamps, and headers, keeping only spoken text.
fn strip_subtitles(raw: &str) -> String {
    let mut out = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty()
            || t.eq_ignore_ascii_case("WEBVTT")
            || t.contains("-->")
            || t.chars().all(|c| c.is_ascii_digit())
        {
            continue;
        }
        out.push(t.to_string());
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_makes_readable_filesystem_safe_stems() {
        assert_eq!(
            slugify("My Contract v2 (final).pdf"),
            "my-contract-v2-final-pdf"
        );
        assert_eq!(slugify("  spaced  out  "), "spaced-out");
        assert_eq!(slugify("***"), "document");
        assert_eq!(slugify(""), "document");
    }

    #[test]
    fn clean_text_strips_control_chars_and_collapses_blank_runs() {
        let raw = "Title\r\n\u{c}\n\n\nBody line one  \n\n\n\nBody line two\n\n";
        let cleaned = clean_text(raw);
        assert!(!cleaned.contains('\r'));
        assert!(!cleaned.contains('\u{c}'));
        assert!(
            !cleaned.contains("\n\n\n"),
            "no 3+ newline runs: {cleaned:?}"
        );
        assert!(cleaned.starts_with("Title"));
        assert!(cleaned.ends_with("Body line two"));
    }

    #[test]
    fn strip_subtitles_keeps_only_spoken_text() {
        let vtt = "WEBVTT\n\n1\n00:00:01.000 --> 00:00:04.000\nHello there\n\n2\n00:00:04.000 --> 00:00:06.000\nGeneral Kenobi";
        let text = strip_subtitles(vtt);
        assert_eq!(text, "Hello there\nGeneral Kenobi");
    }

    #[test]
    fn derive_summary_uses_first_line_then_falls_back_to_stem() {
        assert_eq!(
            derive_summary("\n\n  First real line\nSecond", "stem"),
            "First real line"
        );
        assert_eq!(
            derive_summary("   \n  \n", "fallback-stem"),
            "fallback-stem"
        );
        let long = "x".repeat(500);
        assert_eq!(derive_summary(&long, "stem").chars().count(), 120);
    }
}
