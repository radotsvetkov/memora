//! `memora ingest` — normalize an external document into a vault note so it flows
//! through the existing index -> extract -> verify pipeline. memora's whole model
//! is byte-span verification over markdown, so ingestion is just: turn the source
//! into clean markdown text, write it as a note with valid frontmatter, and let
//! `memora index` pick it up.
//!
//! Supported: plain text, markdown, VTT/SRT transcripts, PDF (with the `pdf`
//! feature), and web pages — a URL or `.html` file (with the `web` feature).
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use clap::Args;

use memora_core::note::{rewrite_with_frontmatter, Frontmatter, NoteSource, Privacy};
use memora_core::vault_path::validate_region;

#[derive(Debug, Args)]
pub struct IngestArgs {
    /// File or URL to ingest: a URL or `.html` (needs the `web` feature), `.pdf`
    /// (needs the `pdf` feature), `.txt`, `.md`, `.markdown`, `.vtt`, `.srt`.
    #[arg(value_name = "file_or_url")]
    pub file: String,
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

pub async fn run(args: IngestArgs) -> Result<()> {
    let privacy: Privacy = args.privacy.parse().map_err(|_| {
        anyhow!(
            "invalid --privacy '{}' (use public|private|secret)",
            args.privacy
        )
    })?;

    // A source is either a URL (fetched) or a local file (read by extension).
    let (raw_body, id_seed, stem, title, kind) = if is_url(&args.file) {
        let (body, title) = fetch_and_extract(&args.file).await?;
        (
            body,
            args.file.clone(),
            url_slug(&args.file),
            title,
            "web".to_string(),
        )
    } else {
        let path = PathBuf::from(&args.file);
        if !path.is_file() {
            bail!("file not found: {}", path.display());
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let (body, title) = extract_file(&path, &ext)?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        let kind = if ext.is_empty() {
            "file".to_string()
        } else {
            ext
        };
        (body, args.file.clone(), stem, title, kind)
    };

    let body = clean_text(&raw_body);
    if body.trim().is_empty() {
        bail!("no text could be extracted from {}", args.file);
    }

    let id = format!("{}-{}", slugify(&stem), short_hash(&id_seed));
    let summary = title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| derive_summary(&body, &stem));

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
        tags: vec!["ingested".to_string(), kind],
        refs: Vec::new(),
    };
    rewrite_with_frontmatter(&note_path, &frontmatter, &body)
        .map_err(|e| anyhow!("write note: {e}"))?;

    println!("Ingested {} -> {}", args.file, note_path.display());
    println!(
        "Next: memora index --vault {}   (extracts claims and makes it verifiable)",
        args.vault.display()
    );
    Ok(())
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Extract a local file by extension. Returns the text and an optional title
/// (only HTML carries one).
fn extract_file(path: &Path, ext: &str) -> Result<(String, Option<String>)> {
    match ext {
        "txt" | "text" | "md" | "markdown" => Ok((
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?,
            None,
        )),
        "vtt" | "srt" => Ok((
            strip_subtitles(
                &std::fs::read_to_string(path)
                    .with_context(|| format!("read {}", path.display()))?,
            ),
            None,
        )),
        "pdf" => Ok((extract_pdf(path)?, None)),
        "html" | "htm" => extract_html_file(path),
        other => bail!(
            "unsupported file type '.{other}'. Supported: a URL or .html (with the `web` \
             feature), .pdf (with the `pdf` feature), .txt, .md, .markdown, .vtt, .srt"
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

#[cfg(feature = "web")]
fn extract_html_file(path: &Path) -> Result<(String, Option<String>)> {
    let html = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    Ok(extract_html(&html))
}

#[cfg(not(feature = "web"))]
fn extract_html_file(_path: &Path) -> Result<(String, Option<String>)> {
    bail!(
        "HTML ingestion needs memora built with the `web` feature \
         (e.g. `cargo install memora-cli --features web`)."
    )
}

#[cfg(feature = "web")]
async fn fetch_and_extract(url: &str) -> Result<(String, Option<String>)> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("memora-ingest/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build HTTP client")?;
    let html = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetch {url}"))?
        .error_for_status()
        .with_context(|| format!("fetch {url}"))?
        .text()
        .await
        .with_context(|| format!("read response body from {url}"))?;
    Ok(extract_html(&html))
}

#[cfg(not(feature = "web"))]
async fn fetch_and_extract(_url: &str) -> Result<(String, Option<String>)> {
    bail!(
        "Ingesting a URL needs memora built with the `web` feature \
         (e.g. `cargo install memora-cli --features web`)."
    )
}

/// Readable-text extraction: collect the text of content elements (paragraphs,
/// headings, list items, quotes, code) and the page title. Selecting only content
/// elements naturally skips `<script>`, `<style>`, and most navigation chrome.
#[cfg(feature = "web")]
fn extract_html(html: &str) -> (String, Option<String>) {
    use scraper::{Html, Selector};

    let doc = Html::parse_document(html);
    let normalize = |s: String| s.split_whitespace().collect::<Vec<_>>().join(" ");

    let title = Selector::parse("title")
        .ok()
        .and_then(|sel| doc.select(&sel).next())
        .map(|el| normalize(el.text().collect::<String>()))
        .filter(|t| !t.is_empty());

    let content = Selector::parse("p, h1, h2, h3, h4, h5, h6, li, blockquote, pre")
        .expect("static selector is valid");
    let mut parts = Vec::new();
    for el in doc.select(&content) {
        let text = normalize(el.text().collect::<String>());
        if !text.is_empty() {
            parts.push(text);
        }
    }
    (parts.join("\n\n"), title)
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

/// A filesystem-safe, readable id stem from a filename or URL.
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

/// The slug-able part of a URL: host + path, without scheme or query/fragment.
fn url_slug(url: &str) -> String {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let without_query = without_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or(without_scheme);
    slugify(without_query)
}

/// Stable short id suffix derived from the source path/URL, so re-ingesting the
/// same source updates the same note rather than creating a duplicate.
fn short_hash(seed: &str) -> String {
    blake3::hash(seed.as_bytes())
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
    fn url_slug_drops_scheme_and_query() {
        assert_eq!(
            url_slug("https://example.com/blog/post?utm=1#top"),
            "example-com-blog-post"
        );
        assert_eq!(url_slug("http://example.com/"), "example-com");
    }

    #[test]
    fn is_url_detects_http_schemes() {
        assert!(is_url("https://example.com"));
        assert!(is_url("http://example.com"));
        assert!(!is_url("/local/file.html"));
        assert!(!is_url("file.txt"));
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

    #[cfg(feature = "web")]
    #[test]
    fn extract_html_keeps_content_and_drops_scripts() {
        let html = r#"<html><head><title>  My Page  </title></head>
            <body><nav>Home About</nav><script>var x = 1;</script>
            <article><h1>Heading</h1><p>First paragraph.</p><p>Second paragraph.</p></article>
            <style>.a{color:red}</style></body></html>"#;
        let (text, title) = extract_html(html);
        assert_eq!(title.as_deref(), Some("My Page"));
        assert!(text.contains("Heading"));
        assert!(text.contains("First paragraph."));
        assert!(text.contains("Second paragraph."));
        assert!(!text.contains("var x"), "script content excluded: {text}");
        assert!(
            !text.contains("color:red"),
            "style content excluded: {text}"
        );
    }
}
