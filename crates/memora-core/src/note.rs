use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use gray_matter::engine::YAML;
use gray_matter::Matter;
use regex::Regex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_yaml::{Mapping, Value};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontmatterAction {
    AlreadyComplete,
    InferredAndRewritten,
    InferredInMemoryOnly,
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
    #[serde(default, deserialize_with = "deserialize_vec_or_default")]
    tags: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_vec_or_default")]
    refs: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct PartialFrontmatter {
    id: Option<String>,
    region: Option<String>,
    source: Option<NoteSource>,
    privacy: Option<Privacy>,
    created: Option<DateTime<Utc>>,
    updated: Option<DateTime<Utc>>,
    summary: Option<String>,
    tags: Option<Vec<String>>,
    refs: Option<Vec<String>>,
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
    let source = fs::read_to_string(path)?;
    parse_from_source(path, &source)
}

pub fn parse_or_infer(
    path: &Path,
    vault_root: &Path,
) -> Result<(Note, FrontmatterAction), ParseError> {
    parse_or_infer_impl(path, vault_root, true)
}

pub fn parse_or_infer_in_memory(
    path: &Path,
    vault_root: &Path,
) -> Result<(Note, FrontmatterAction), ParseError> {
    parse_or_infer_impl(path, vault_root, false)
}

pub fn rewrite_with_frontmatter(
    path: &Path,
    frontmatter: &Frontmatter,
    original_body: &str,
) -> Result<(), ParseError> {
    let normalized_frontmatter = Frontmatter {
        id: frontmatter.id.clone(),
        region: frontmatter.region.clone(),
        source: frontmatter.source,
        privacy: frontmatter.privacy,
        created: truncate_datetime_to_seconds(frontmatter.created),
        updated: truncate_datetime_to_seconds(frontmatter.updated),
        summary: frontmatter.summary.clone(),
        tags: frontmatter.tags.clone(),
        refs: frontmatter.refs.clone(),
    };
    let serialized = serde_yaml::to_string(&normalized_frontmatter)
        .map_err(|err| ParseError::InvalidFrontmatter(err.to_string()))?;
    let new_content = format!("---\n{serialized}---\n{original_body}");

    write_content_atomically(path, &new_content)
}

fn parse_or_infer_impl(
    path: &Path,
    vault_root: &Path,
    rewrite: bool,
) -> Result<(Note, FrontmatterAction), ParseError> {
    let source = fs::read_to_string(path)?;
    match parse_from_source(path, &source) {
        Ok(note) => Ok((note, FrontmatterAction::AlreadyComplete)),
        Err(ParseError::MissingFrontmatter) | Err(ParseError::MissingField(_)) => {
            let (existing_partial, original_body) = extract_partial_and_body(&source)?;
            let inferred =
                infer_frontmatter(path, vault_root, &original_body, existing_partial.as_ref())?;
            if rewrite {
                rewrite_with_frontmatter(path, &inferred, &original_body)?;
            }
            let wikilinks = extract_wikilinks(&original_body)?;
            let action = if rewrite {
                FrontmatterAction::InferredAndRewritten
            } else {
                FrontmatterAction::InferredInMemoryOnly
            };
            Ok((
                Note {
                    path: path.to_path_buf(),
                    fm: inferred,
                    body: original_body,
                    wikilinks,
                },
                action,
            ))
        }
        Err(err @ ParseError::InvalidFrontmatter(_)) => {
            if !rewrite {
                return Err(err);
            }
            if normalize_invalid_source_and_privacy(path, &source)? {
                return parse_or_infer_impl(path, vault_root, rewrite);
            }
            Err(err)
        }
        Err(err @ ParseError::Io(_)) => Err(err),
    }
}

fn normalize_invalid_source_and_privacy(path: &Path, source: &str) -> Result<bool, ParseError> {
    let Some((yaml, body)) = split_frontmatter_block(source)? else {
        return Ok(false);
    };
    let mut yaml_value: Value = serde_yaml::from_str(&yaml)
        .map_err(|err| ParseError::InvalidFrontmatter(err.to_string()))?;
    let Some(map) = yaml_value.as_mapping_mut() else {
        return Ok(false);
    };

    let mut changed = false;
    changed |= normalize_enum_frontmatter_field(map, "source", "personal", |value| {
        NoteSource::from_str(value).is_ok()
    });
    changed |= normalize_enum_frontmatter_field(map, "privacy", "private", |value| {
        Privacy::from_str(value).is_ok()
    });

    if !changed {
        return Ok(false);
    }
    let serialized = serde_yaml::to_string(&yaml_value)
        .map_err(|err| ParseError::InvalidFrontmatter(err.to_string()))?;
    let rewritten = format!("---\n{serialized}---\n{body}");
    write_content_atomically(path, &rewritten)?;
    Ok(true)
}

fn normalize_enum_frontmatter_field<F>(
    map: &mut Mapping,
    key: &'static str,
    fallback: &'static str,
    is_valid: F,
) -> bool
where
    F: Fn(&str) -> bool,
{
    let lookup = Value::String(key.to_string());
    let Some(raw) = map.get(&lookup) else {
        return false;
    };
    let mut changed = false;
    match raw {
        Value::String(value) if is_valid(value.as_str()) => {}
        _ => {
            changed = true;
            map.insert(lookup, Value::String(fallback.to_string()));
        }
    }
    changed
}

fn parse_from_source(path: &Path, source: &str) -> Result<Note, ParseError> {
    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(source);
    let data = match parsed.data {
        Some(data) => data,
        None => {
            if starts_with_frontmatter_delimiter(source) {
                return Err(ParseError::InvalidFrontmatter(
                    "malformed YAML frontmatter".to_string(),
                ));
            }
            return Err(ParseError::MissingFrontmatter);
        }
    };
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

fn infer_frontmatter(
    path: &Path,
    vault_root: &Path,
    body_for_summary: &str,
    existing_partial: Option<&PartialFrontmatter>,
) -> Result<Frontmatter, ParseError> {
    let metadata = fs::metadata(path)?;

    let id = match existing_partial.and_then(|partial| partial.id.clone()) {
        Some(value) => value,
        None => infer_unique_id(path, vault_root)?,
    };
    let region = match existing_partial.and_then(|partial| partial.region.clone()) {
        Some(value) => value,
        None => infer_region(path, vault_root)?,
    };
    let created = match existing_partial.and_then(|partial| partial.created) {
        Some(value) => value,
        None => {
            let created_time = metadata.created().or_else(|_| metadata.modified())?;
            datetime_from_system_time_secs(created_time)?
        }
    };
    let updated = match existing_partial.and_then(|partial| partial.updated) {
        Some(value) => value,
        None => datetime_from_system_time_secs(metadata.modified()?)?,
    };
    let summary = match existing_partial.and_then(|partial| partial.summary.clone()) {
        Some(value) => value,
        None => infer_summary(body_for_summary),
    };
    let source = existing_partial
        .and_then(|partial| partial.source)
        .unwrap_or(NoteSource::Personal);
    let privacy = existing_partial
        .and_then(|partial| partial.privacy)
        .unwrap_or(Privacy::Private);
    let tags = existing_partial
        .and_then(|partial| partial.tags.clone())
        .unwrap_or_default();
    let refs = existing_partial
        .and_then(|partial| partial.refs.clone())
        .unwrap_or_default();

    Ok(Frontmatter {
        id,
        region,
        source,
        privacy,
        created,
        updated,
        summary,
        tags,
        refs,
    })
}

pub fn render(note: &Note) -> String {
    let normalized_frontmatter = Frontmatter {
        id: note.fm.id.clone(),
        region: note.fm.region.clone(),
        source: note.fm.source,
        privacy: note.fm.privacy,
        created: truncate_datetime_to_seconds(note.fm.created),
        updated: truncate_datetime_to_seconds(note.fm.updated),
        summary: note.fm.summary.clone(),
        tags: note.fm.tags.clone(),
        refs: note.fm.refs.clone(),
    };
    let frontmatter = match serde_yaml::to_string(&normalized_frontmatter) {
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

fn extract_partial_and_body(
    source: &str,
) -> Result<(Option<PartialFrontmatter>, String), ParseError> {
    let Some((yaml, body)) = split_frontmatter_block(source)? else {
        return Ok((None, source.to_string()));
    };
    let yaml_value: Value = serde_yaml::from_str(&yaml)
        .map_err(|err| ParseError::InvalidFrontmatter(err.to_string()))?;
    let partial = parse_partial_frontmatter(&yaml_value)?;
    Ok((Some(partial), body))
}

fn parse_partial_frontmatter(value: &Value) -> Result<PartialFrontmatter, ParseError> {
    let map = value.as_mapping().ok_or_else(|| {
        ParseError::InvalidFrontmatter("frontmatter must be a YAML mapping".to_string())
    })?;

    Ok(PartialFrontmatter {
        id: deserialize_optional_field(map, "id")?,
        region: deserialize_optional_field(map, "region")?,
        source: deserialize_optional_field(map, "source")?,
        privacy: deserialize_optional_field(map, "privacy")?,
        created: deserialize_optional_field(map, "created")?,
        updated: deserialize_optional_field(map, "updated")?,
        summary: deserialize_optional_field(map, "summary")?,
        tags: deserialize_optional_vec_field(map, "tags")?,
        refs: deserialize_optional_vec_field(map, "refs")?,
    })
}

fn deserialize_optional_field<T>(map: &Mapping, key: &'static str) -> Result<Option<T>, ParseError>
where
    T: DeserializeOwned,
{
    let lookup = Value::String(key.to_string());
    match map.get(&lookup) {
        Some(raw) => serde_yaml::from_value(raw.clone())
            .map(Some)
            .map_err(|err| ParseError::InvalidFrontmatter(format!("invalid field `{key}`: {err}"))),
        None => Ok(None),
    }
}

fn deserialize_optional_vec_field(
    map: &Mapping,
    key: &'static str,
) -> Result<Option<Vec<String>>, ParseError> {
    let lookup = Value::String(key.to_string());
    match map.get(&lookup) {
        Some(Value::Null) => Ok(Some(Vec::new())),
        Some(raw) => serde_yaml::from_value(raw.clone())
            .map(Some)
            .map_err(|err| ParseError::InvalidFrontmatter(format!("invalid field `{key}`: {err}"))),
        None => Ok(None),
    }
}

fn deserialize_vec_or_default<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Vec<String>>::deserialize(deserializer)?;
    Ok(value.unwrap_or_default())
}

fn split_frontmatter_block(source: &str) -> Result<Option<(String, String)>, ParseError> {
    if !starts_with_frontmatter_delimiter(source) {
        return Ok(None);
    }

    let mut cursor = 0usize;
    let Some((first_line, next_cursor)) = next_line(source, cursor) else {
        return Ok(None);
    };
    if first_line != "---" {
        return Ok(None);
    }
    cursor = next_cursor;
    let yaml_start = cursor;

    while let Some((line, next)) = next_line(source, cursor) {
        if line == "---" {
            let yaml = source[yaml_start..cursor].to_string();
            let body = source[next..].to_string();
            return Ok(Some((yaml, body)));
        }
        cursor = next;
    }

    Err(ParseError::InvalidFrontmatter(
        "malformed YAML frontmatter".to_string(),
    ))
}

fn next_line(source: &str, start: usize) -> Option<(&str, usize)> {
    if start >= source.len() {
        return None;
    }
    let remaining = &source[start..];
    match remaining.find('\n') {
        Some(relative_idx) => {
            let mut line = &remaining[..relative_idx];
            if let Some(stripped) = line.strip_suffix('\r') {
                line = stripped;
            }
            Some((line, start + relative_idx + 1))
        }
        None => {
            let mut line = remaining;
            if let Some(stripped) = line.strip_suffix('\r') {
                line = stripped;
            }
            Some((line, source.len()))
        }
    }
}

fn starts_with_frontmatter_delimiter(source: &str) -> bool {
    source.starts_with("---\n") || source.starts_with("---\r\n")
}

fn infer_region(path: &Path, vault_root: &Path) -> Result<String, ParseError> {
    if path.parent().is_none() {
        return Err(ParseError::InvalidFrontmatter(
            "note path has no parent directory".to_string(),
        ));
    }
    if !path.starts_with(vault_root) {
        return Err(ParseError::InvalidFrontmatter(format!(
            "note path `{}` is outside vault root `{}`",
            path.display(),
            vault_root.display()
        )));
    }
    Ok(derive_region_from_path(path, vault_root))
}

/// Derive a note region from its folder path relative to the vault root.
pub fn derive_region_from_path(path: &Path, vault_root: &Path) -> String {
    let rel = path.strip_prefix(vault_root).unwrap_or(path);
    let parent = rel.parent();
    match parent {
        Some(p) if !p.as_os_str().is_empty() => p
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/"),
        _ => "default".to_string(),
    }
}

fn datetime_from_system_time_secs(system_time: SystemTime) -> Result<DateTime<Utc>, ParseError> {
    let seconds = system_time
        .duration_since(UNIX_EPOCH)
        .map_err(|err| ParseError::InvalidFrontmatter(format!("invalid timestamp: {err}")))?
        .as_secs();
    DateTime::<Utc>::from_timestamp(seconds as i64, 0)
        .ok_or_else(|| ParseError::InvalidFrontmatter("invalid timestamp".to_string()))
}

pub fn truncate_datetime_to_seconds(value: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(value.timestamp(), 0).unwrap_or(value)
}

fn write_content_atomically(path: &Path, content: &str) -> Result<(), ParseError> {
    let parent = path.parent().ok_or_else(|| {
        ParseError::InvalidFrontmatter("note path has no parent directory".to_string())
    })?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    fs::rename(temp.path(), path)?;
    Ok(())
}

fn infer_unique_id(path: &Path, vault_root: &Path) -> Result<String, ParseError> {
    let file_stem = path.file_stem().ok_or_else(|| {
        ParseError::InvalidFrontmatter(format!("cannot infer id from path `{}`", path.display()))
    })?;
    let base_slug = slugify(&file_stem.to_string_lossy());
    let base_slug = if base_slug.is_empty() {
        "note".to_string()
    } else {
        base_slug
    };

    let mut collisions = collect_slug_collisions(vault_root, &base_slug)?;
    collisions.sort();
    let position = collisions
        .iter()
        .position(|candidate| candidate == path)
        .unwrap_or(0);
    if position == 0 {
        Ok(base_slug)
    } else {
        Ok(format!("{base_slug}-{}", position + 1))
    }
}

fn collect_slug_collisions(vault_root: &Path, base_slug: &str) -> Result<Vec<PathBuf>, ParseError> {
    let mut stack = vec![vault_root.to_path_buf()];
    let mut matching = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let entry_path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(entry_path);
                continue;
            }
            if entry_path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = entry_path.file_stem() else {
                continue;
            };
            if slugify(&stem.to_string_lossy()) == base_slug {
                matching.push(entry_path);
            }
        }
    }
    Ok(matching)
}

fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn infer_summary(body_for_summary: &str) -> String {
    let first_non_empty = body_for_summary
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("(no summary)");
    truncate_with_ellipsis(first_non_empty, 500)
}

fn truncate_with_ellipsis(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let head: String = input.chars().take(max_chars - 3).collect();
    format!("{head}...")
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::fs;
    use std::io::Write;
    use std::str::FromStr;

    use tempfile::{tempdir, NamedTempFile};

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
    fn parse_treats_null_tags_and_refs_as_empty_lists() {
        let note_text = r#"---
id: null-lists
region: work
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-02T00:00:00Z
summary: "null lists"
tags: null
refs: null
---
Body
"#;
        let file = write_temp(note_text);
        let note = parse(file.path()).expect("null tags/refs should parse");
        assert!(note.fm.tags.is_empty());
        assert!(note.fm.refs.is_empty());
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

    #[test]
    fn parse_or_infer_succeeds_on_complete_frontmatter() {
        let note_text = r#"---
id: complete-note
region: work
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-02T00:00:00Z
summary: "Already complete."
tags: []
refs: []
---
body
"#;
        let file = write_temp(note_text);
        let vault_root = file.path().parent().expect("temp note should have parent");

        let (parsed, action) = parse_or_infer_impl(file.path(), vault_root, false)
            .expect("parse_or_infer should succeed");
        assert_eq!(action, FrontmatterAction::AlreadyComplete);
        assert_eq!(parsed.fm.id, "complete-note");
    }

    #[test]
    fn parse_or_infer_infers_id_from_filename_when_missing() {
        let temp = tempdir().expect("create tempdir");
        let vault_root = temp.path().join("vault");
        fs::create_dir_all(&vault_root).expect("create vault");
        let path = vault_root.join("Project Roadmap Q1.md");
        fs::write(
            &path,
            r#"---
region: work
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Roadmap"
---
Body
"#,
        )
        .expect("write note");

        let (parsed, action) =
            parse_or_infer_impl(&path, &vault_root, false).expect("parse_or_infer should succeed");
        assert_eq!(action, FrontmatterAction::InferredInMemoryOnly);
        assert_eq!(parsed.fm.id, "project-roadmap-q1");
    }

    #[test]
    fn parse_or_infer_infers_region_from_folder_path() {
        let temp = tempdir().expect("create tempdir");
        let vault_root = temp.path().join("vault");
        let note_path = vault_root.join("work/internorga/q1.md");
        fs::create_dir_all(note_path.parent().expect("note should have parent"))
            .expect("create folders");
        fs::write(
            &note_path,
            r#"---
id: q1
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Q1 note"
---
Body
"#,
        )
        .expect("write note");

        let (parsed, _) = parse_or_infer_impl(&note_path, &vault_root, false)
            .expect("parse_or_infer should succeed");
        assert_eq!(parsed.fm.region, "work/internorga");
    }

    #[test]
    fn derive_region_from_path_uses_parent_relative_to_vault_root() {
        let vault_root = Path::new("/vault");
        assert_eq!(
            derive_region_from_path(Path::new("/vault/work/sub/note.md"), vault_root),
            "work/sub"
        );
        assert_eq!(
            derive_region_from_path(Path::new("/vault/note.md"), vault_root),
            "default"
        );
    }

    #[test]
    fn infer_frontmatter_rewrite_serializes_second_precision_timestamps() {
        let temp = tempdir().expect("create tempdir");
        let vault_root = temp.path().join("vault");
        fs::create_dir_all(&vault_root).expect("create vault");
        let path = vault_root.join("fresh.md");
        let body = "Fresh note body.\n";
        fs::write(&path, body).expect("write note body");

        let inferred =
            infer_frontmatter(&path, &vault_root, body, None).expect("infer frontmatter");
        rewrite_with_frontmatter(&path, &inferred, body)
            .expect("rewrite with inferred frontmatter");

        let rewritten = fs::read_to_string(&path).expect("read rewritten note");
        let created_line = rewritten
            .lines()
            .find(|line| line.starts_with("created: "))
            .expect("created line should exist");
        let created_value = created_line.trim_start_matches("created: ").trim();

        assert!(!created_value.contains('.'));
        assert!(created_value.ends_with('Z'));
        assert!(Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
            .expect("valid regex")
            .is_match(created_value));
    }

    #[test]
    fn parse_or_infer_infers_summary_from_first_body_line() {
        let temp = tempdir().expect("create tempdir");
        let vault_root = temp.path().join("vault");
        fs::create_dir_all(&vault_root).expect("create vault");
        let path = vault_root.join("daily.md");
        fs::write(
            &path,
            r#"---
id: daily
region: default
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
---

  First useful summary line  
Second line
"#,
        )
        .expect("write note");

        let (parsed, _) =
            parse_or_infer_impl(&path, &vault_root, false).expect("parse_or_infer should succeed");
        assert_eq!(parsed.fm.summary, "First useful summary line");
    }

    #[test]
    fn parse_or_infer_preserves_existing_partial_fields() {
        let temp = tempdir().expect("create tempdir");
        let vault_root = temp.path().join("vault");
        fs::create_dir_all(&vault_root).expect("create vault");
        let path = vault_root.join("tagged.md");
        fs::write(
            &path,
            r#"---
region: default
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Has tags"
tags: [foo, bar]
---
Body
"#,
        )
        .expect("write note");

        let (parsed, _) =
            parse_or_infer_impl(&path, &vault_root, false).expect("parse_or_infer should succeed");
        assert_eq!(parsed.fm.tags, vec!["foo".to_string(), "bar".to_string()]);
        assert_eq!(parsed.fm.id, "tagged");
    }

    #[test]
    fn parse_or_infer_returns_err_on_malformed_yaml() {
        let temp = tempdir().expect("create tempdir");
        let vault_root = temp.path().join("vault");
        fs::create_dir_all(&vault_root).expect("create vault");
        let path = vault_root.join("broken.md");
        fs::write(
            &path,
            "---\nid: [\nregion: default\ncreated: 2026-04-01T00:00:00Z\nupdated: 2026-04-01T00:00:00Z\nsummary: bad\n---\nbody\n",
        )
        .expect("write note");

        let err = parse_or_infer(&path, &vault_root).expect_err("malformed yaml should fail");
        assert!(matches!(err, ParseError::InvalidFrontmatter(_)));
    }

    #[test]
    fn parse_or_infer_normalizes_invalid_source_and_privacy_values() {
        let temp = tempdir().expect("create tempdir");
        let vault_root = temp.path().join("vault");
        fs::create_dir_all(&vault_root).expect("create vault");
        let path = vault_root.join("invalid-enums.md");
        fs::write(
            &path,
            r#"---
id: invalid-enums
region: default
source: random
privacy: top-secret
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Invalid enum values"
tags: []
refs: []
---
Body
"#,
        )
        .expect("write note");

        let (parsed, _) =
            parse_or_infer(&path, &vault_root).expect("parse_or_infer should recover");
        assert_eq!(parsed.fm.source, NoteSource::Personal);
        assert_eq!(parsed.fm.privacy, Privacy::Private);

        let rewritten = fs::read_to_string(&path).expect("read rewritten note");
        assert!(rewritten.contains("source: personal"));
        assert!(rewritten.contains("privacy: private"));
    }

    #[test]
    fn rewrite_with_frontmatter_atomically_replaces_file() {
        let temp = tempdir().expect("create tempdir");
        let vault_root = temp.path().join("vault");
        fs::create_dir_all(&vault_root).expect("create vault");
        let path = vault_root.join("note.md");
        let body = "\n# Title\nBody line\n";
        fs::write(&path, body).expect("write body");

        let frontmatter = Frontmatter {
            id: "note".to_string(),
            region: "default".to_string(),
            source: NoteSource::Personal,
            privacy: Privacy::Private,
            created: DateTime::from_str("2026-04-01T00:00:00Z").expect("valid datetime"),
            updated: DateTime::from_str("2026-04-01T00:00:00Z").expect("valid datetime"),
            summary: "Summary".to_string(),
            tags: vec![],
            refs: vec![],
        };

        rewrite_with_frontmatter(&path, &frontmatter, body).expect("rewrite should succeed");
        let rewritten = fs::read_to_string(&path).expect("read rewritten note");
        assert!(rewritten.starts_with("---\n"));
        assert!(rewritten.ends_with(body));
    }

    #[test]
    fn render_truncates_fractional_timestamp_precision() {
        let note = Note {
            path: PathBuf::from("vault/note.md"),
            fm: Frontmatter {
                id: "note".to_string(),
                region: "default".to_string(),
                source: NoteSource::Personal,
                privacy: Privacy::Private,
                created: DateTime::from_timestamp(1_714_504_417, 668_688_826)
                    .expect("valid created timestamp"),
                updated: DateTime::from_timestamp(1_714_504_417, 999_999_999)
                    .expect("valid updated timestamp"),
                summary: "Summary".to_string(),
                tags: vec![],
                refs: vec![],
            },
            body: "Body".to_string(),
            wikilinks: vec![],
        };

        let rendered = render(&note);
        let created_line = rendered
            .lines()
            .find(|line| line.starts_with("created: "))
            .expect("created field should be present");
        let updated_line = rendered
            .lines()
            .find(|line| line.starts_with("updated: "))
            .expect("updated field should be present");

        let created_value = created_line.trim_start_matches("created: ").trim();
        let updated_value = updated_line.trim_start_matches("updated: ").trim();
        let ts_pattern =
            Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$").expect("valid regex");
        assert!(ts_pattern.is_match(created_value));
        assert!(ts_pattern.is_match(updated_value));
        assert!(!created_value.contains('.'));
        assert!(!updated_value.contains('.'));
    }

    #[test]
    fn parse_or_infer_handles_id_collision() {
        let temp = tempdir().expect("create tempdir");
        let vault_root = temp.path().join("vault");
        fs::create_dir_all(vault_root.join("a")).expect("create folder a");
        fs::create_dir_all(vault_root.join("b")).expect("create folder b");
        let first = vault_root.join("a/Project Idea.md");
        let second = vault_root.join("b/Project Idea.md");
        fs::write(&first, body_only_note()).expect("write first note");
        fs::write(&second, body_only_note()).expect("write second note");

        let (first_note, _) =
            parse_or_infer_impl(&first, &vault_root, false).expect("first infer should succeed");
        let (second_note, _) =
            parse_or_infer_impl(&second, &vault_root, false).expect("second infer should succeed");
        assert_eq!(first_note.fm.id, "project-idea");
        assert_eq!(second_note.fm.id, "project-idea-2");
    }

    fn body_only_note() -> &'static str {
        "First summary line\n\nMore details.\n"
    }
}
