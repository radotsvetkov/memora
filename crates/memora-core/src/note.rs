use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use gray_matter::engine::YAML;
use gray_matter::Matter;
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteSource {
    #[default]
    Personal,
    Reference,
    Derived,
}

impl Display for NoteSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Personal => "personal",
            Self::Reference => "reference",
            Self::Derived => "derived",
        };
        f.write_str(value)
    }
}

impl FromStr for NoteSource {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "personal" => Ok(Self::Personal),
            "reference" => Ok(Self::Reference),
            "derived" => Ok(Self::Derived),
            _ => Err(ParseError::InvalidFrontmatter(format!(
                "invalid note source: {s}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "lowercase")]
pub enum Privacy {
    Public = 0,
    #[default]
    Private = 1,
    Secret = 2,
}

impl Display for Privacy {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Secret => "secret",
        };
        f.write_str(value)
    }
}

impl FromStr for Privacy {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "public" => Ok(Self::Public),
            "private" => Ok(Self::Private),
            "secret" => Ok(Self::Secret),
            _ => Err(ParseError::InvalidFrontmatter(format!(
                "invalid privacy level: {s}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frontmatter {
    pub id: String,
    pub region: String,
    #[serde(default)]
    pub source: NoteSource,
    #[serde(default)]
    pub privacy: Privacy,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub path: PathBuf,
    pub fm: Frontmatter,
    pub body: String,
    pub wikilinks: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("missing YAML frontmatter")]
    MissingFrontmatter,
    #[error("invalid frontmatter: {0}")]
    InvalidFrontmatter(String),
    #[error("missing required frontmatter field: {0}")]
    MissingField(&'static str),
}

#[derive(Debug, Deserialize)]
struct RawFrontmatter {
    id: Option<String>,
    region: Option<String>,
    #[serde(default)]
    source: NoteSource,
    #[serde(default)]
    privacy: Privacy,
    created: Option<DateTime<Utc>>,
    updated: Option<DateTime<Utc>>,
    summary: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    refs: Vec<String>,
}

impl TryFrom<RawFrontmatter> for Frontmatter {
    type Error = ParseError;

    fn try_from(value: RawFrontmatter) -> Result<Self, Self::Error> {
        let id = value.id.ok_or(ParseError::MissingField("id"))?;
        if id.trim().is_empty() {
            return Err(ParseError::InvalidFrontmatter(
                "field `id` cannot be empty".to_string(),
            ));
        }

        let region = value.region.ok_or(ParseError::MissingField("region"))?;
        let has_region_segment = region.split('/').any(|segment| !segment.trim().is_empty());
        if !has_region_segment {
            return Err(ParseError::InvalidFrontmatter(
                "field `region` must contain at least one segment".to_string(),
            ));
        }

        let created = value.created.ok_or(ParseError::MissingField("created"))?;
        let updated = value.updated.ok_or(ParseError::MissingField("updated"))?;

        let summary = value.summary.ok_or(ParseError::MissingField("summary"))?;
        if summary.chars().count() > 500 {
            return Err(ParseError::InvalidFrontmatter(
                "field `summary` cannot exceed 500 characters".to_string(),
            ));
        }

        Ok(Self {
            id,
            region,
            source: value.source,
            privacy: value.privacy,
            created,
            updated,
            summary,
            tags: value.tags,
            refs: value.refs,
        })
    }
}

pub fn parse(path: &Path) -> Result<Note, ParseError> {
    let source = std::fs::read_to_string(path)?;
    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(&source);
    let data = parsed.data.ok_or(ParseError::MissingFrontmatter)?;
    let raw: RawFrontmatter = data
        .deserialize()
        .map_err(|err| ParseError::InvalidFrontmatter(err.to_string()))?;
    let fm = Frontmatter::try_from(raw)?;
    let body = parsed.content.to_string();
    let wikilinks = extract_wikilinks(&body)?;

    Ok(Note {
        path: path.to_path_buf(),
        fm,
        body,
        wikilinks,
    })
}

pub fn render(note: &Note) -> String {
    let frontmatter = match serde_yaml::to_string(&note.fm) {
        Ok(serialized) => serialized.trim_end().to_string(),
        Err(err) => {
            tracing::error!(error = %err, path = %note.path.display(), "failed to serialize frontmatter");
            String::new()
        }
    };
    format!("---\n{frontmatter}\n---\n{}", note.body)
}

fn extract_wikilinks(body: &str) -> Result<Vec<String>, ParseError> {
    let regex = Regex::new(r"\[\[([^\]|#\n]+)")
        .map_err(|err| ParseError::InvalidFrontmatter(err.to_string()))?;
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();

    for capture in regex.captures_iter(body) {
        if let Some(matched) = capture.get(1) {
            let link = matched.as_str().trim();
            if !link.is_empty() && seen.insert(link.to_string()) {
                ordered.push(link.to_string());
            }
        }
    }

    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::io::Write;
    use std::str::FromStr;

    use tempfile::NamedTempFile;

    use super::*;

    fn write_temp(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("create temp file");
        file.write_all(contents.as_bytes())
            .expect("write temp file content");
        file.flush().expect("flush temp file");
        file
    }

    #[test]
    fn parse_extracts_wikilinks_preserves_order_without_duplicates() {
        let note_text = r#"---
id: note-1
region: work/projects
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-02T00:00:00Z
summary: "A project note."
tags: [work]
refs: [world-map]
---
# Body
See [[Alpha]] then [[Beta|Custom]] and [[Alpha]] again.
Also [[Alpha#section]] should still resolve to Alpha.
"#;

        let file = write_temp(note_text);
        let note = parse(file.path()).expect("note parse should succeed");

        assert_eq!(
            note.wikilinks,
            vec!["Alpha".to_string(), "Beta".to_string()]
        );
    }

    #[test]
    fn parse_fails_when_frontmatter_missing() {
        let file = write_temp("# Just markdown\nNo YAML block.");
        let err = parse(file.path()).expect_err("missing frontmatter should fail");
        assert!(matches!(err, ParseError::MissingFrontmatter));
    }

    #[test]
    fn parse_fails_with_missing_required_field_id() {
        let note_text = r#"---
region: work
created: 2026-04-01T00:00:00Z
updated: 2026-04-02T00:00:00Z
summary: "No id here."
---
content
"#;
        let file = write_temp(note_text);
        let err = parse(file.path()).expect_err("missing id should fail");
        assert!(matches!(err, ParseError::MissingField("id")));
    }

    #[test]
    fn privacy_ordering_is_public_private_secret() {
        assert_eq!(Privacy::Public.cmp(&Privacy::Private), Ordering::Less);
        assert_eq!(Privacy::Private.cmp(&Privacy::Secret), Ordering::Less);
        assert_eq!(Privacy::Public.cmp(&Privacy::Secret), Ordering::Less);
    }

    #[test]
    fn note_source_from_str_accepts_known_values() {
        assert_eq!(
            NoteSource::from_str("personal").expect("personal should parse"),
            NoteSource::Personal
        );
        assert_eq!(
            NoteSource::from_str("reference").expect("reference should parse"),
            NoteSource::Reference
        );
        assert_eq!(
            NoteSource::from_str("derived").expect("derived should parse"),
            NoteSource::Derived
        );
        assert!(NoteSource::from_str("unknown").is_err());
    }

    #[test]
    fn render_and_parse_round_trip_frontmatter_fields() {
        let original = r#"---
id: world-map
region: meta
source: derived
privacy: public
created: 2026-04-01T00:00:00Z
updated: 2026-04-28T00:00:00Z
summary: "Auto-generated overview of the vault."
tags: [meta]
refs: [work-atlas]
---
# World Map
Body text with [[Atlas]].
"#;

        let first = write_temp(original);
        let parsed = parse(first.path()).expect("initial parse should succeed");
        let rendered = render(&parsed);

        let second = write_temp(&rendered);
        let reparsed = parse(second.path()).expect("rendered parse should succeed");
        assert_eq!(parsed.fm, reparsed.fm);
    }
}
