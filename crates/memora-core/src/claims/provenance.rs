use rusqlite::params;

use crate::index::{Index, IndexError};

pub struct Provenance<'a> {
    db: &'a Index,
}

impl<'a> Provenance<'a> {
    pub fn new(db: &'a Index) -> Self {
        Self { db }
    }

    pub fn record(&self, derived: &str, sources: &[&str]) -> Result<(), IndexError> {
        if sources.is_empty() {
            return Ok(());
        }
        let conn = self.db.pool.get()?;
        for source in sources {
            conn.execute(
                "INSERT OR IGNORE INTO provenance (derived_claim_id, source_claim_id)
                 VALUES (?, ?)",
                params![derived, source],
            )?;
        }
        Ok(())
    }

    pub fn sources_of(&self, derived: &str) -> Result<Vec<String>, IndexError> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT source_claim_id
             FROM provenance
             WHERE derived_claim_id = ?
             ORDER BY source_claim_id ASC",
        )?;
        let rows = stmt.query_map(params![derived], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    pub fn derivatives_of(&self, source: &str) -> Result<Vec<String>, IndexError> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT derived_claim_id
             FROM provenance
             WHERE source_claim_id = ?
             ORDER BY derived_claim_id ASC",
        )?;
        let rows = stmt.query_map(params![source], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }
}
