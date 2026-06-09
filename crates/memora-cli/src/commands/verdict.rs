//! Shared rendering for citation verdicts, used by `memora demo` and
//! `memora verify`. One place decides how a verified, rejected, or superseded
//! citation looks, in the terminal and in the HTML Proof Report.
use memora_core::CitationStatus;

/// One line of a verdict: a piece of text and the status of its citation.
pub struct VerdictLine {
    pub text: String,
    pub status: CitationStatus,
}

pub struct RenderOpts {
    pub color: bool,
    /// Show the "a naive tool would have shown all of this" contrast block.
    pub show_naive_contrast: bool,
}

/// Whether ANSI colour should be emitted (honours the NO_COLOR convention).
pub fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

/// Plain-English reason a citation was rejected, flagged, or accepted.
pub fn reason(status: CitationStatus) -> &'static str {
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

fn color_code(status: CitationStatus) -> &'static str {
    match status {
        CitationStatus::Verified => "1;32",
        CitationStatus::Superseded => "1;33",
        _ => "1;31",
    }
}

fn badge_label(status: CitationStatus) -> &'static str {
    match status {
        CitationStatus::Verified => "✓ VERIFIED  ",
        CitationStatus::Superseded => "⚠ SUPERSEDED",
        _ => "✗ REJECTED  ",
    }
}

fn paint(color: bool, code: &str, s: &str) -> String {
    if color {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn line_text(color: bool, status: CitationStatus, text: &str) -> String {
    match status {
        CitationStatus::Verified => paint(color, "32", text),
        CitationStatus::Superseded => paint(color, "33", text),
        _ if color => format!("\x1b[9m\x1b[31m{text}\x1b[0m"),
        _ => format!("[rejected] {text}"),
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Count (verified, rejected, superseded) across the lines. "Rejected" folds
/// together hallucinated ids and fingerprint/quote mismatches.
pub fn tally(lines: &[VerdictLine]) -> (usize, usize, usize) {
    let mut v = 0;
    let mut r = 0;
    let mut s = 0;
    for l in lines {
        match l.status {
            CitationStatus::Verified => v += 1,
            CitationStatus::Superseded => s += 1,
            _ => r += 1,
        }
    }
    (v, r, s)
}

/// Render the verdict to stdout.
pub fn render_terminal(lines: &[VerdictLine], clean_text: &str, raw_text: &str, opts: &RenderOpts) {
    let c = opts.color;
    for l in lines {
        println!(
            "{}  {}",
            paint(c, color_code(l.status), badge_label(l.status)),
            paint(c, "2", reason(l.status))
        );
        println!("   {}", line_text(c, l.status, &l.text));
        println!();
    }

    let (v, r, s) = tally(lines);
    println!("{}", paint(c, "1", "Verdict"));
    println!(
        "  {}   {}   {}",
        paint(c, "1;32", &format!("{v} verified")),
        paint(c, "1;31", &format!("{r} rejected")),
        paint(c, "1;33", &format!("{s} superseded")),
    );
    println!();

    if opts.show_naive_contrast {
        println!(
            "{}",
            paint(
                c,
                "2",
                "A naive RAG/prompt-cite tool would have shown all of this as fact:"
            )
        );
        println!("   {}", paint(c, "2", &one_line(raw_text)));
        println!();
    }

    println!(
        "{}",
        paint(c, "1", "What memora returns (only what it could prove):")
    );
    let clean = if clean_text.trim().is_empty() {
        "(nothing — none of the citations could be verified)".to_string()
    } else {
        one_line(clean_text)
    };
    println!("   {}", paint(c, "32", &clean));
}

/// Escape text for safe inclusion in HTML element content.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Open a generated file in the OS default browser (best-effort, ignores errors).
pub fn open_in_browser(path: &std::path::Path) {
    let (bin, args): (&str, Vec<std::ffi::OsString>) = if cfg!(target_os = "macos") {
        ("open", vec![path.as_os_str().to_owned()])
    } else if cfg!(target_os = "windows") {
        (
            "cmd",
            vec!["/C".into(), "start".into(), path.as_os_str().to_owned()],
        )
    } else {
        ("xdg-open", vec![path.as_os_str().to_owned()])
    };
    let _ = std::process::Command::new(bin).args(args).status();
}

/// A standalone, dependency-free HTML "Proof Report".
pub fn render_html(lines: &[VerdictLine], clean_text: &str) -> String {
    let mut cards = String::new();
    for l in lines {
        let (cls, label) = match l.status {
            CitationStatus::Verified => ("ok", "VERIFIED"),
            CitationStatus::Superseded => ("warn", "SUPERSEDED"),
            CitationStatus::Unverified => ("bad", "REJECTED — hallucinated id"),
            CitationStatus::FingerprintMismatch => ("bad", "REJECTED — hash mismatch"),
            CitationStatus::QuoteMismatch => ("bad", "REJECTED — quote not in source"),
        };
        let text = html_escape(&l.text);
        let text = if matches!(
            l.status,
            CitationStatus::Verified | CitationStatus::Superseded
        ) {
            text
        } else {
            format!("<s>{text}</s>")
        };
        cards.push_str(&format!(
            "<div class=\"card {cls}\"><span class=\"label\">{label}</span><div class=\"sentence\">{text}</div><div class=\"reason\">{}</div></div>\n",
            html_escape(reason(l.status))
        ));
    }
    let clean = if clean_text.trim().is_empty() {
        "(nothing — none of the citations could be verified)".to_string()
    } else {
        html_escape(&one_line(clean_text))
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Memora Proof Report</title>\
<style>:root{{color-scheme:dark}}body{{font:16px/1.5 -apple-system,Segoe UI,Roboto,sans-serif;background:#0b0f14;color:#e6edf3;max-width:760px;margin:40px auto;padding:0 20px}}\
h1{{font-size:22px}}.sub{{color:#8b949e;margin-bottom:24px}}\
.card{{border-left:4px solid #30363d;background:#11161d;border-radius:8px;padding:14px 16px;margin:12px 0}}\
.card.ok{{border-color:#2ea043}}.card.bad{{border-color:#f85149}}.card.warn{{border-color:#d29922}}\
.label{{font-size:12px;font-weight:700;letter-spacing:.04em}}.card.ok .label{{color:#3fb950}}.card.bad .label{{color:#ff7b72}}.card.warn .label{{color:#e3b341}}\
.sentence{{margin:6px 0;font-size:17px}}.sentence s{{color:#ff7b72;text-decoration-color:#f85149}}\
.reason{{color:#8b949e;font-size:14px}}\
.final{{margin-top:28px;padding:16px;background:#0d1117;border:1px solid #30363d;border-radius:8px}}\
.final .k{{color:#8b949e;font-size:13px}}.final .v{{color:#3fb950;margin-top:6px}}</style></head><body>\
<h1>Memora Proof Report</h1>\
<div class=\"sub\">Every citation re-read against the source span and re-hashed. No API key, nothing mocked.</div>\
{cards}\
<div class=\"final\"><div class=\"k\">A naive tool returns all of the above as fact. memora returns only what it could prove:</div><div class=\"v\">{clean}</div></div>\
</body></html>"
    )
}

/// Extract the sentence in `text` that contains the marker for `claim_id`, so a
/// verdict can show the model's actual sentence rather than a bare id.
pub fn sentence_for_marker(text: &str, claim_id: &str) -> String {
    let marker = format!("[claim:{claim_id}]");
    match text.find(&marker) {
        Some(pos) => {
            let start = text[..pos]
                .rfind(['.', '!', '?', '\n'])
                .map(|i| i + 1)
                .unwrap_or(0);
            let end = text[pos..]
                .find(['.', '!', '?'])
                .map(|i| pos + i + 1)
                .unwrap_or(text.len());
            text[start..end].trim().to_string()
        }
        None => marker,
    }
}
