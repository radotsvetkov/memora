use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};

use crate::index::Index;

pub struct HebbianLearner<'a> {
    db: &'a Index,
}

impl<'a> HebbianLearner<'a> {
    pub fn new(db: &'a Index) -> Self {
        Self { db }
    }

    pub fn record_coactivation(&self, ids: &[&str]) -> Result<()> {
        if ids.len() < 2 {
            return Ok(());
        }

        let mut deduped = ids.iter().map(|id| (*id).to_string()).collect::<Vec<_>>();
        deduped.sort();
        deduped.dedup();
        if deduped.len() < 2 {
            return Ok(());
        }

        let now = Utc::now();
        self.db.with_transaction(|tx| {
            for i in 0..deduped.len() {
                for j in (i + 1)..deduped.len() {
                    let a = &deduped[i];
                    let b = &deduped[j];

                    let existing = tx
                        .query_row(
                            "SELECT weight, last_coactivated
                             FROM hebbian_edges
                             WHERE a_id = ? AND b_id = ?",
                            params![a, b],
                            |row| Ok((row.get::<_, f32>(0)?, row.get::<_, String>(1)?)),
                        )
                        .optional()?;

                    let (decayed_weight, decay_factor) = if let Some((weight, last_str)) = existing
                    {
                        let last = DateTime::parse_from_rfc3339(&last_str)
                            .map(|dt| dt.with_timezone(&Utc))
                            .ok();
                        let days_since_last = last
                            .map(|ts| (now - ts).num_seconds().max(0) as f32 / 86_400.0)
                            .unwrap_or(0.0);
                        let factor = 0.999_f32.powf(days_since_last);
                        (weight * factor, factor)
                    } else {
                        (0.0, 1.0)
                    };

                    let new_weight = decayed_weight + 1.0;
                    tx.execute(
                        "INSERT INTO hebbian_edges (a_id, b_id, weight, last_coactivated)
                         VALUES (?, ?, ?, ?)
                         ON CONFLICT(a_id, b_id) DO UPDATE SET
                           weight = excluded.weight,
                           last_coactivated = excluded.last_coactivated",
                        params![a, b, new_weight, now.to_rfc3339()],
                    )?;
                    tracing::debug!(
                        a_id = %a,
                        b_id = %b,
                        decay_factor = decay_factor,
                        new_weight = new_weight,
                        "recorded hebbian coactivation"
                    );
                }
            }
            Ok(())
        })?;

        Ok(())
    }

    pub fn neighbors(&self, id: &str, top_n: usize) -> Result<Vec<(String, f32)>> {
        Ok(self.db.hebbian_neighbors(id, top_n)?)
    }
}
