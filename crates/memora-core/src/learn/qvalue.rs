use std::collections::HashSet;

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use crate::index::Index;

pub struct QValueLearner<'a> {
    db: &'a Index,
    alpha: f32,
}

impl<'a> QValueLearner<'a> {
    pub fn new(db: &'a Index) -> Self {
        Self { db, alpha: 0.1 }
    }

    pub fn with_alpha(db: &'a Index, alpha: f32) -> Self {
        Self { db, alpha }
    }

    pub fn reinforce(&self, useful_ids: &[&str], all_returned_ids: &[&str]) -> Result<()> {
        let useful = useful_ids.iter().copied().collect::<HashSet<_>>();
        self.db.with_transaction(|tx| {
            for id in all_returned_ids {
                let reward = if useful.contains(id) {
                    1.0_f32
                } else {
                    -0.1_f32
                };
                let q_old = tx
                    .query_row(
                        "SELECT qvalue FROM notes WHERE id = ?",
                        params![id],
                        |row| row.get::<_, f32>(0),
                    )
                    .optional()?
                    .unwrap_or(0.0);
                let q_new = q_old + self.alpha * (reward - q_old);
                tx.execute(
                    "UPDATE notes SET qvalue = ? WHERE id = ?",
                    params![q_new, id],
                )?;
            }
            Ok(())
        })?;
        Ok(())
    }
}
