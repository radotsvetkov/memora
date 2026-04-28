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
             WHERE subject = ? AND predicate = ?
             ORDER BY valid_from DESC",
        )?;
        let rows = stmt.query_map(params![subject, predicate], map_claim_row)?;
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
    let span_start_raw: i64 = row.get(5)?;
    let span_end_raw: i64 = row.get(6)?;
    let valid_from: String = row.get(8)?;
    let valid_until: Option<String> = row.get(9)?;
    let privacy: String = row.get(11)?;
    let extracted_at: String = row.get(13)?;

    let span_start = usize::try_from(span_start_raw).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "span_start is negative",
            )),
        )
    })?;
    let span_end = usize::try_from(span_end_raw).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "span_end is negative",
            )),
        )
    })?;

    let valid_from = chrono::DateTime::parse_from_rfc3339(&valid_from)
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(err))
        })?
        .with_timezone(&Utc);
    let valid_until = match valid_until {
        Some(value) => Some(
            chrono::DateTime::parse_from_rfc3339(&value)
                .map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        9,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?
                .with_timezone(&Utc),
        ),
        None => None,
    };
    let privacy = Privacy::from_str(&privacy).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let extracted_at = chrono::DateTime::parse_from_rfc3339(&extracted_at)
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                13,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?
        .with_timezone(&Utc);

    Ok(Claim {
        id: row.get(0)?,
        subject: row.get(1)?,
        predicate: row.get(2)?,
        object: row.get(3)?,
        note_id: row.get(4)?,
        span_start,
        span_end,
        span_fingerprint: row.get(7)?,
        valid_from,
        valid_until,
        confidence: row.get(10)?,
        privacy,
        extracted_by: row.get(12)?,
        extracted_at,
    })
}
