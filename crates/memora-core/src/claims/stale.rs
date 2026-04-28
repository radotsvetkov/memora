use std::collections::BTreeSet;

use chrono::Utc;
use rusqlite::params;

use crate::claims::Provenance;
use crate::index::{Index, IndexError};

pub struct StalenessTracker<'a> {
    db: &'a Index,
    prov: &'a Provenance<'a>,
}

impl<'a> StalenessTracker<'a> {
    pub fn new(db: &'a Index, prov: &'a Provenance<'a>) -> Self {
        Self { db, prov }
    }

    pub fn on_note_changed(&self, note_id: &str) -> Result<usize, IndexError> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id
             FROM claims
             WHERE note_id = ?",
        )?;
        let rows = stmt.query_map(params![note_id], |row| row.get::<_, String>(0))?;
        let mut old_claim_ids = Vec::new();
        for row in rows {
            old_claim_ids.push(row?);
        }
        self.mark_for_sources(&old_claim_ids, "source_edited")
    }

    pub fn on_claim_superseded(&self, claim_id: &str) -> Result<usize, IndexError> {
        self.mark_for_sources(&[claim_id.to_string()], "source_superseded")
    }

    pub fn mark_source_edited_claims(
        &self,
        source_claim_ids: &[String],
    ) -> Result<usize, IndexError> {
        self.mark_for_sources(source_claim_ids, "source_edited")
    }

    pub fn list_stale(&self) -> Result<Vec<(String, String)>, IndexError> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT claim_id, reason
             FROM stale_claims
             ORDER BY marked_at DESC, claim_id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let claim_id: String = row.get(0)?;
            let reason: String = row.get(1)?;
            Ok((claim_id, reason))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn mark_for_sources(
        &self,
        source_claim_ids: &[String],
        reason: &str,
    ) -> Result<usize, IndexError> {
        let mut stale_ids = BTreeSet::new();
        for source_claim_id in source_claim_ids {
            for derivative in self.prov.derivatives_of(source_claim_id)? {
                stale_ids.insert(derivative);
            }
        }

        if stale_ids.is_empty() {
            return Ok(0);
        }

        let now = Utc::now().to_rfc3339();
        let conn = self.db.pool.get()?;
        for claim_id in &stale_ids {
            conn.execute(
                "INSERT OR REPLACE INTO stale_claims (claim_id, reason, marked_at)
                 VALUES (?, ?, ?)",
                params![claim_id, reason, now],
            )?;
        }
        Ok(stale_ids.len())
    }
}
