use std::str::FromStr;

use chrono::Utc;
use rusqlite::{params, OptionalExtension};

use crate::claims::{Claim, ClaimRelation};
use crate::index::{Index, IndexError};
use crate::note::Privacy;

pub struct ClaimStore<'a> {
    db: &'a Index,
}

impl<'a> ClaimStore<'a> {
    pub fn new(db: &'a Index) -> Self {
        Self { db }
    }

    pub fn upsert(&self, claim: &Claim) -> Result<(), IndexError> {
        self.db.with_transaction(|tx| self.upsert_in_tx(tx, claim))
    }

    pub(crate) fn upsert_in_tx(
        &self,
        tx: &rusqlite::Transaction<'_>,
        claim: &Claim,
    ) -> Result<(), IndexError> {
        let span_start = i64::try_from(claim.span_start)
            .map_err(|_| IndexError::Schema("span_start does not fit in i64".to_string()))?;
        let span_end = i64::try_from(claim.span_end)
            .map_err(|_| IndexError::Schema("span_end does not fit in i64".to_string()))?;

        tx.execute(
            "INSERT OR REPLACE INTO claims
             (id, subject, predicate, object, note_id, span_start, span_end, span_fingerprint,
              valid_from, valid_until, confidence, privacy, extracted_by, extracted_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                claim.id,
                claim.subject,
                claim.predicate,
                claim.object,
                claim.note_id,
                span_start,
                span_end,
                claim.span_fingerprint,
                claim.valid_from.to_rfc3339(),
                claim.valid_until.map(|dt| dt.to_rfc3339()),
                claim.confidence,
                claim.privacy.to_string(),
                claim.extracted_by,
                claim.extracted_at.to_rfc3339(),
            ],
        )?;
        tx.execute("DELETE FROM claims_fts WHERE id = ?", params![claim.id])?;
        tx.execute(
            "INSERT INTO claims_fts (id, subject, predicate, object) VALUES (?, ?, ?, ?)",
            params![claim.id, claim.subject, claim.predicate, claim.object],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<Claim>, IndexError> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, subject, predicate, object, note_id, span_start, span_end, span_fingerprint,
                    valid_from, valid_until, confidence, privacy, extracted_by, extracted_at
             FROM claims
             WHERE id = ?",
        )?;
        let row = stmt.query_row(params![id], map_claim_row).optional()?;
        Ok(row)
    }

    pub fn delete_for_note(&self, note_id: &str) -> Result<(), IndexError> {
        self.db
            .with_transaction(|tx| self.delete_for_note_in_tx(tx, note_id))
    }

    pub(crate) fn delete_for_note_in_tx(
        &self,
        tx: &rusqlite::Transaction<'_>,
        note_id: &str,
    ) -> Result<(), IndexError> {
        tx.execute(
            "DELETE FROM claims_fts WHERE id IN (SELECT id FROM claims WHERE note_id = ?)",
            params![note_id],
        )?;
        tx.execute("DELETE FROM claims WHERE note_id = ?", params![note_id])?;
        Ok(())
    }

    pub fn list_for_note(&self, note_id: &str) -> Result<Vec<Claim>, IndexError> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, subject, predicate, object, note_id, span_start, span_end, span_fingerprint,
                    valid_from, valid_until, confidence, privacy, extracted_by, extracted_at
             FROM claims
             WHERE note_id = ?
             ORDER BY span_start ASC",
        )?;
        let rows = stmt.query_map(params![note_id], map_claim_row)?;
        let mut claims = Vec::new();
        for row in rows {
            claims.push(row?);
        }
        Ok(claims)
    }

    pub fn claim_ids_for_note(&self, note_id: &str) -> Result<Vec<String>, IndexError> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare("SELECT id FROM claims WHERE note_id = ? ORDER BY id ASC")?;
        let rows = stmt.query_map(params![note_id], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    pub fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<(String, f32)>, IndexError> {
        let limit = i64::try_from(limit)
            .map_err(|_| IndexError::Schema("limit does not fit in i64".to_string()))?;
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, bm25(claims_fts) AS score
             FROM claims_fts
             WHERE claims_fts MATCH ?
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

    pub fn find_by_subject_predicate(
        &self,
        subject: &str,
        predicate: &str,
    ) -> Result<Vec<Claim>, IndexError> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, subject, predicate, object, note_id, span_start, span_end, span_fingerprint,
                    valid_from, valid_until, confidence, privacy, extracted_by, extracted_at
             FROM claims
             WHERE subject = ? AND predicate = ? AND valid_until IS NULL
             ORDER BY valid_from DESC",
        )?;
        let rows = stmt.query_map(params![subject, predicate], map_claim_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn find_by_subject(&self, subject: &str) -> Result<Vec<Claim>, IndexError> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, subject, predicate, object, note_id, span_start, span_end, span_fingerprint,
                    valid_from, valid_until, confidence, privacy, extracted_by, extracted_at
             FROM claims
             WHERE subject = ? AND valid_until IS NULL
             ORDER BY valid_from DESC",
        )?;
        let rows = stmt.query_map(params![subject], map_claim_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn add_relation(
        &self,
        src: &str,
        dst: &str,
        relation: ClaimRelation,
        weight: f32,
    ) -> Result<(), IndexError> {
        let conn = self.db.pool.get()?;
        conn.execute(
            "INSERT OR IGNORE INTO claim_relations (src_id, dst_id, relation, weight, created)
             VALUES (?, ?, ?, ?, ?)",
            params![
                src,
                dst,
                relation.to_string(),
                weight,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn has_relation(
        &self,
        src: &str,
        dst: &str,
        relation: ClaimRelation,
    ) -> Result<bool, IndexError> {
        let conn = self.db.pool.get()?;
        let exists = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM claim_relations
                WHERE src_id = ? AND dst_id = ? AND relation = ?
            )",
            params![src, dst, relation.to_string()],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(exists == 1)
    }

    pub fn contradictions_unack(&self) -> Result<Vec<(Claim, Claim)>, IndexError> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT
                a.id, a.subject, a.predicate, a.object, a.note_id, a.span_start, a.span_end, a.span_fingerprint,
                a.valid_from, a.valid_until, a.confidence, a.privacy, a.extracted_by, a.extracted_at,
                b.id, b.subject, b.predicate, b.object, b.note_id, b.span_start, b.span_end, b.span_fingerprint,
                b.valid_from, b.valid_until, b.confidence, b.privacy, b.extracted_by, b.extracted_at
             FROM claim_relations r
             JOIN claims a ON a.id = r.src_id
             JOIN claims b ON b.id = r.dst_id
             LEFT JOIN acknowledged_contradictions ack
               ON ack.a_id = r.src_id AND ack.b_id = r.dst_id
             WHERE r.relation = 'contradicts'
               AND ack.a_id IS NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            let left = map_claim_row_from_offset(row, 0)?;
            let right = map_claim_row_from_offset(row, 14)?;
            Ok((left, right))
        })?;
        let mut pairs = Vec::new();
        for row in rows {
            pairs.push(row?);
        }
        Ok(pairs)
    }

    pub fn current_only(&self, ids: &[String]) -> Result<Vec<Claim>, IndexError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; ids.len()].join(", ");
        let sql = format!(
            "SELECT id, subject, predicate, object, note_id, span_start, span_end, span_fingerprint,
                    valid_from, valid_until, confidence, privacy, extracted_by, extracted_at
             FROM claims
             WHERE id IN ({placeholders})
               AND (valid_until IS NULL OR valid_until > ?)"
        );
        let now = Utc::now().to_rfc3339();
        let mut values = ids
            .iter()
            .map(|id| rusqlite::types::Value::from(id.clone()))
            .collect::<Vec<_>>();
        values.push(rusqlite::types::Value::from(now));

        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), map_claim_row)?;
        let mut claims = Vec::new();
        for row in rows {
            claims.push(row?);
        }
        Ok(claims)
    }
}

fn map_claim_row(row: &rusqlite::Row<'_>) -> Result<Claim, rusqlite::Error> {
    map_claim_row_from_offset(row, 0)
}

fn map_claim_row_from_offset(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> Result<Claim, rusqlite::Error> {
    let span_start_raw: i64 = row.get(offset + 5)?;
    let span_end_raw: i64 = row.get(offset + 6)?;
    let valid_from: String = row.get(offset + 8)?;
    let valid_until: Option<String> = row.get(offset + 9)?;
    let privacy: String = row.get(offset + 11)?;
    let extracted_at: String = row.get(offset + 13)?;

    let span_start = usize::try_from(span_start_raw).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            offset + 5,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "span_start is negative",
            )),
        )
    })?;
    let span_end = usize::try_from(span_end_raw).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            offset + 6,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "span_end is negative",
            )),
        )
    })?;

    let valid_from = chrono::DateTime::parse_from_rfc3339(&valid_from)
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                offset + 8,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?
        .with_timezone(&Utc);
    let valid_until = match valid_until {
        Some(value) => Some(
            chrono::DateTime::parse_from_rfc3339(&value)
                .map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        offset + 9,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?
                .with_timezone(&Utc),
        ),
        None => None,
    };
    let privacy = Privacy::from_str(&privacy).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            offset + 11,
            rusqlite::types::Type::Text,
            Box::new(err),
        )
    })?;
    let extracted_at = chrono::DateTime::parse_from_rfc3339(&extracted_at)
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                offset + 13,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?
        .with_timezone(&Utc);

    Ok(Claim {
        id: row.get(offset)?,
        subject: row.get(offset + 1)?,
        predicate: row.get(offset + 2)?,
        object: row.get(offset + 3)?,
        note_id: row.get(offset + 4)?,
        span_start,
        span_end,
        span_fingerprint: row.get(offset + 7)?,
        valid_from,
        valid_until,
        confidence: row.get(offset + 10)?,
        privacy,
        extracted_by: row.get(offset + 12)?,
        extracted_at,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::TimeZone;
    use rusqlite::params;

    use super::*;
    use crate::index::Index;

    fn seed_note(index: &Index, note_id: &str) -> Result<(), IndexError> {
        let conn = index.pool.get()?;
        conn.execute(
            "INSERT INTO notes
             (id, path, region, source, privacy, body_hash, body_size, summary, tags_json, created, updated, qvalue)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                note_id,
                format!("vault/{note_id}.md"),
                "test/unit",
                "personal",
                "private",
                "body-hash",
                16_i64,
                "test note",
                "[]",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:00Z",
                0.0_f64,
            ],
        )?;
        Ok(())
    }

    fn make_claim(
        note_id: &str,
        object: &str,
        valid_from: chrono::DateTime<Utc>,
        valid_until: Option<chrono::DateTime<Utc>>,
    ) -> Claim {
        Claim {
            id: Claim::compute_id("X", "works_at", object, note_id, 0),
            subject: "X".to_string(),
            predicate: "works_at".to_string(),
            object: object.to_string(),
            note_id: note_id.to_string(),
            span_start: 0,
            span_end: 4,
            span_fingerprint: Claim::compute_fingerprint(object),
            valid_from,
            valid_until,
            confidence: 1.0,
            privacy: Privacy::Private,
            extracted_by: "test/unit".to_string(),
            extracted_at: valid_from,
        }
    }

    #[test]
    fn find_by_subject_and_predicate_return_only_current_claims() -> Result<(), IndexError> {
        let index = Index::open(Path::new(":memory:"))?;
        seed_note(&index, "note-x")?;
        let store = ClaimStore::new(&index);

        let t1 = Utc
            .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
            .single()
            .expect("valid datetime");
        let t2 = Utc
            .with_ymd_and_hms(2024, 6, 1, 0, 0, 0)
            .single()
            .expect("valid datetime");
        let t3 = Utc
            .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
            .single()
            .expect("valid datetime");

        let claim_a = make_claim("note-x", "Co1", t1, Some(t2));
        let claim_b = make_claim("note-x", "Co2", t2, Some(t3));
        let claim_c = make_claim("note-x", "Co3", t3, None);
        store.upsert(&claim_a)?;
        store.upsert(&claim_b)?;
        store.upsert(&claim_c)?;

        let by_subject_predicate = store.find_by_subject_predicate("X", "works_at")?;
        assert_eq!(by_subject_predicate.len(), 1);
        assert_eq!(by_subject_predicate[0].id, claim_c.id);

        let by_subject = store.find_by_subject("X")?;
        assert_eq!(by_subject.len(), 1);
        assert_eq!(by_subject[0].id, claim_c.id);

        Ok(())
    }
}
