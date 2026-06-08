use std::collections::{BTreeSet, VecDeque};

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
        // Walk the provenance graph transitively: a change to a source claim
        // makes its derivatives stale, and their derivatives in turn (A -> B ->
        // C marks both B and C). `visited` is seeded with the source claims so
        // they are never marked stale themselves and so cycles terminate.
        let mut stale_ids = BTreeSet::new();
        let mut visited: BTreeSet<String> = source_claim_ids.iter().cloned().collect();
        let mut queue: VecDeque<String> = source_claim_ids.iter().cloned().collect();
        while let Some(current) = queue.pop_front() {
            for derivative in self.prov.derivatives_of(&current)? {
                if visited.insert(derivative.clone()) {
                    stale_ids.insert(derivative.clone());
                    queue.push_back(derivative);
                }
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

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn staleness_propagates_transitively() {
        let temp = tempdir().expect("tempdir");
        let index = Index::open(&temp.path().join("memora.db")).expect("index");
        let prov = Provenance::new(&index);
        // A -> B -> C: B derives from A, C derives from B.
        prov.record("claim-b", &["claim-a"]).expect("record b<-a");
        prov.record("claim-c", &["claim-b"]).expect("record c<-b");

        let tracker = StalenessTracker::new(&index, &prov);
        let marked = tracker
            .mark_source_edited_claims(&["claim-a".to_string()])
            .expect("mark");
        assert_eq!(marked, 2, "both B and C should be marked stale, not just B");

        let stale: BTreeSet<String> = tracker
            .list_stale()
            .expect("list")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(stale.contains("claim-b"));
        assert!(stale.contains("claim-c"));
        assert!(
            !stale.contains("claim-a"),
            "the edited source itself must not be marked stale"
        );
    }

    #[test]
    fn staleness_terminates_on_cycles() {
        let temp = tempdir().expect("tempdir");
        let index = Index::open(&temp.path().join("memora.db")).expect("index");
        let prov = Provenance::new(&index);
        prov.record("y", &["x"]).expect("record y<-x");
        prov.record("x", &["y"]).expect("record x<-y"); // cycle

        let tracker = StalenessTracker::new(&index, &prov);
        let marked = tracker
            .mark_source_edited_claims(&["x".to_string()])
            .expect("mark");
        // x -> y, then y -> x (already visited as the source) stops the walk.
        assert_eq!(marked, 1);
    }
}
