use std::collections::HashSet;
use std::fs;
use std::path::Path;

use chrono::Utc;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension};
use thiserror::Error;

use crate::note::Note;

#[derive(Debug, Error)]
pub enum IndexError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Pool(#[from] r2d2::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("schema error: {0}")]
    Schema(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteRow {
    pub id: String,
    pub path: String,
    pub region: String,
    pub source: String,
    pub privacy: String,
    pub body_hash: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub created: String,
    pub updated: String,
    pub qvalue: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebuildStats {
    pub inserted: usize,
    pub skipped: usize,
    pub errors: usize,
}

pub struct Index {
    pub(crate) pool: Pool<SqliteConnectionManager>,
}

const EMBEDDED_MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init.sql", include_str!("../../migrations/0001_init.sql")),
    ("0002_ack.sql", include_str!("../../migrations/0002_ack.sql")),
    (
        "0003_consolidation_runs.sql",
        include_str!("../../migrations/0003_consolidation_runs.sql"),
    ),
];

impl Index {
    pub fn open(db_path: &Path) -> Result<Self, IndexError> {
        if db_path != Path::new(":memory:") {
            if let Some(parent) = db_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|err| IndexError::Schema(format!("create db directory: {err}")))?;
            }
        }

        let manager = if db_path == Path::new(":memory:") {
            SqliteConnectionManager::memory()
        } else {
            SqliteConnectionManager::file(db_path)
        }
        .with_init(|conn| {
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;\
                 PRAGMA foreign_keys=ON;\
                 PRAGMA synchronous=NORMAL;",
            )
        });

        let pool = Pool::builder().max_size(1).build(manager)?;
        let index = Self { pool };
        index.run_migrations()?;
        Ok(index)
    }

    pub fn upsert_note(&self, note: &Note, body: &str) -> Result<(), IndexError> {
        let body_hash = truncated_blake3_hex(body);
        let tags_json = serde_json::to_string(&note.fm.tags)?;
        let tags_fts = note.fm.tags.join(" ");
        let path = note.path.to_string_lossy().to_string();
        let source = note.fm.source.to_string();
        let privacy = note.fm.privacy.to_string();
        let created = note.fm.created.to_rfc3339();
        let updated = note.fm.updated.to_rfc3339();
        let body_size = i64::try_from(body.len())
            .map_err(|_| IndexError::Schema("body size does not fit in i64".to_string()))?;

        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;

        tx.execute(
            "INSERT OR REPLACE INTO notes
            (id, path, region, source, privacy, body_hash, body_size, summary, tags_json, created, updated, qvalue)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0.0)",
            params![
                note.fm.id,
                path,
                note.fm.region,
                source,
                privacy,
                body_hash,
                body_size,
                note.fm.summary,
                tags_json,
                created,
                updated,
            ],
        )?;

        tx.execute("DELETE FROM notes_fts WHERE id = ?", params![note.fm.id])?;
        tx.execute(
            "INSERT INTO notes_fts (id, summary, body, tags) VALUES (?, ?, ?, ?)",
            params![note.fm.id, note.fm.summary, body, tags_fts],
        )?;

        tx.execute(
            "DELETE FROM wikilinks WHERE src_id = ?",
            params![note.fm.id],
        )?;
        for target in &note.wikilinks {
            tx.execute(
                "INSERT INTO wikilinks (src_id, dst_target) VALUES (?, ?)",
                params![note.fm.id, target],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn delete_note(&self, id: &str) -> Result<(), IndexError> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM notes_fts WHERE id = ?", params![id])?;
        conn.execute("DELETE FROM notes WHERE id = ?", params![id])?;
        Ok(())
    }

    pub fn bm25_search(&self, query: &str, limit: usize) -> Result<Vec<(String, f32)>, IndexError> {
        let limit = i64::try_from(limit)
            .map_err(|_| IndexError::Schema("limit does not fit in i64".to_string()))?;
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, bm25(notes_fts) AS score
             FROM notes_fts
             WHERE notes_fts MATCH ?
             ORDER BY score
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![query, limit], |row| {
            let id: String = row.get(0)?;
            let score: f64 = row.get(1)?;
            Ok((id, (-score) as f32))
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_note(&self, id: &str) -> Result<Option<NoteRow>, IndexError> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, path, region, source, privacy, body_hash, summary, tags_json, created, updated, qvalue
             FROM notes
             WHERE id = ?",
        )?;

        let row = stmt
            .query_row(params![id], |row| {
                let tags_json: String = row.get(7)?;
                let tags: Vec<String> = serde_json::from_str(&tags_json).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;

                Ok(NoteRow {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    region: row.get(2)?,
                    source: row.get(3)?,
                    privacy: row.get(4)?,
                    body_hash: row.get(5)?,
                    summary: row.get(6)?,
                    tags,
                    created: row.get(8)?,
                    updated: row.get(9)?,
                    qvalue: row.get(10)?,
                })
            })
            .optional()?;
        Ok(row)
    }

    pub fn all_ids(&self) -> Result<Vec<String>, IndexError> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare("SELECT id FROM notes ORDER BY id ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    pub fn wikilink_targets(&self, src_id: &str) -> Result<Vec<String>, IndexError> {
        let conn = self.pool.get()?;
        let mut stmt = conn
            .prepare("SELECT dst_target FROM wikilinks WHERE src_id = ? ORDER BY dst_target ASC")?;
        let rows = stmt.query_map(params![src_id], |row| row.get::<_, String>(0))?;
        let mut targets = Vec::new();
        for row in rows {
            targets.push(row?);
        }
        Ok(targets)
    }

    pub fn note_id_for_target(&self, target: &str) -> Result<Option<String>, IndexError> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT id FROM notes WHERE id = ?",
            params![target],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(IndexError::from)
    }

    pub fn qvalue(&self, id: &str) -> Result<Option<f32>, IndexError> {
        let conn = self.pool.get()?;
        conn.query_row(
            "SELECT qvalue FROM notes WHERE id = ?",
            params![id],
            |row| row.get::<_, f32>(0),
        )
        .optional()
        .map_err(IndexError::from)
    }

    pub fn hebbian_neighbors(
        &self,
        id: &str,
        top_n: usize,
    ) -> Result<Vec<(String, f32)>, IndexError> {
        let top_n = i64::try_from(top_n)
            .map_err(|_| IndexError::Schema("top_n does not fit in i64".to_string()))?;
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT b_id, weight FROM hebbian_edges WHERE a_id = ?
             UNION
             SELECT a_id, weight FROM hebbian_edges WHERE b_id = ?
             ORDER BY weight DESC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![id, id, top_n], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn record_retrieval(
        &self,
        query_id: &str,
        query_text: &str,
        claim_ids: &[String],
    ) -> Result<(), IndexError> {
        let claim_ids_json = serde_json::to_string(claim_ids)?;
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO retrievals (query_id, query_text, claim_ids_json, ts, marked_useful_json)
             VALUES (?, ?, ?, ?, NULL)",
            params![
                query_id,
                query_text,
                claim_ids_json,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub(crate) fn with_transaction<T, F>(&self, f: F) -> Result<T, IndexError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, IndexError>,
    {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub(crate) fn id_by_path(&self, path: &Path) -> Result<Option<String>, IndexError> {
        let conn = self.pool.get()?;
        let path = path.to_string_lossy().to_string();
        let id = conn
            .query_row(
                "SELECT id FROM notes WHERE path = ?",
                params![path],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(id)
    }

    fn run_migrations(&self) -> Result<(), IndexError> {
        let mut conn = self.pool.get()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations (
                name TEXT PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )?;

        let applied = {
            let mut stmt = conn.prepare("SELECT name FROM _migrations")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut names = HashSet::new();
            for row in rows {
                names.insert(row?);
            }
            names
        };

        for &(name, sql) in EMBEDDED_MIGRATIONS {
            if applied.contains(name) {
                continue;
            }
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO _migrations (name, applied_at) VALUES (?, ?)",
                params![name, Utc::now().to_rfc3339()],
            )?;
            tx.commit()?;
        }

        Ok(())
    }
}

fn truncated_blake3_hex(body: &str) -> String {
    let hash = blake3::hash(body.as_bytes());
    hash.as_bytes()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::note::{Frontmatter, Note, NoteSource, Privacy};

    fn make_note(id: &str, path: &str, summary: &str, body: &str, wikilinks: Vec<String>) -> Note {
        Note {
            path: PathBuf::from(path),
            fm: Frontmatter {
                id: id.to_string(),
                region: "test/unit".to_string(),
                source: NoteSource::Personal,
                privacy: Privacy::Private,
                created: Utc
                    .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                    .single()
                    .expect("valid created datetime"),
                updated: Utc
                    .with_ymd_and_hms(2026, 1, 2, 0, 0, 0)
                    .single()
                    .expect("valid updated datetime"),
                summary: summary.to_string(),
                tags: vec!["unit".to_string(), "search".to_string()],
                refs: Vec::new(),
            },
            body: body.to_string(),
            wikilinks,
        }
    }

    #[test]
    fn migrations_upsert_bm25_order_and_delete_cascade_work() {
        let index = Index::open(Path::new(":memory:")).expect("open in-memory index");

        let note_a = make_note(
            "note-a",
            "vault/note-a.md",
            "Nebula report with astronomy keywords",
            "nebula nebula star cluster",
            vec!["target-a".to_string(), "target-b".to_string()],
        );
        let note_b = make_note(
            "note-b",
            "vault/note-b.md",
            "Star observations",
            "star log with one nebula mention",
            vec!["target-a".to_string()],
        );

        index
            .upsert_note(&note_a, &note_a.body)
            .expect("upsert note a");
        index
            .upsert_note(&note_b, &note_b.body)
            .expect("upsert note b");

        let results = index.bm25_search("nebula", 10).expect("run bm25 search");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "note-a");
        assert!(results[0].1 > results[1].1);

        {
            let conn = index.pool.get().expect("get pooled connection");
            let migration_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))
                .expect("query migration count");
            assert!(migration_count >= 1);

            let wikilink_count_before: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM wikilinks WHERE src_id = 'note-a'",
                    [],
                    |row| row.get(0),
                )
                .expect("query wikilinks before delete");
            assert_eq!(wikilink_count_before, 2);
        }

        index.delete_note("note-a").expect("delete note-a");

        {
            let conn = index.pool.get().expect("get pooled connection");
            let wikilink_count_after: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM wikilinks WHERE src_id = 'note-a'",
                    [],
                    |row| row.get(0),
                )
                .expect("query wikilinks after delete");
            assert_eq!(wikilink_count_after, 0);
        }
        assert!(index
            .get_note("note-a")
            .expect("lookup deleted note")
            .is_none());
    }

    #[test]
    fn embedded_migrations_list_is_not_empty() {
        assert!(!EMBEDDED_MIGRATIONS.is_empty());
        assert_eq!(EMBEDDED_MIGRATIONS[0].0, "0001_init.sql");
    }
}
