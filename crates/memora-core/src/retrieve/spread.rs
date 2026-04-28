use std::cmp::Ordering;
use std::collections::HashSet;

use anyhow::Result;

use crate::index::Index;
use crate::retrieve::hybrid::{HitSource, RetrievalHit};

pub fn spread(seeds: &[RetrievalHit], db: &Index, max_extra: usize) -> Result<Vec<RetrievalHit>> {
    let mut output = seeds.to_vec();
    let mut seen = output
        .iter()
        .map(|hit| hit.id.clone())
        .collect::<HashSet<_>>();
    let mut extras = Vec::<RetrievalHit>::new();

    for seed in seeds.iter().take(3) {
        let neighbors = db
            .hebbian_neighbors(&seed.id, 2)?
            .into_iter()
            .filter(|(_, weight)| *weight > 0.5)
            .collect::<Vec<_>>();
        if !neighbors.is_empty() {
            let max_weight = neighbors
                .iter()
                .map(|(_, weight)| *weight)
                .fold(0.0_f32, f32::max)
                .max(1.0);
            for (id, weight) in neighbors {
                if seen.contains(&id) {
                    continue;
                }
                let score = seed.score * 0.5 * (weight / max_weight);
                extras.push(RetrievalHit {
                    id,
                    score,
                    source: HitSource::Both,
                });
            }
        }

        for target in db.wikilink_targets(&seed.id)? {
            let Some(id) = db.note_id_for_target(&target)? else {
                continue;
            };
            if seen.contains(&id) {
                continue;
            }
            extras.push(RetrievalHit {
                id,
                score: seed.score * 0.4,
                source: HitSource::Both,
            });
        }
    }

    for hit in extras {
        if seen.contains(&hit.id) {
            continue;
        }
        seen.insert(hit.id.clone());
        output.push(hit);
        if output.len().saturating_sub(seeds.len()) >= max_extra {
            break;
        }
    }

    output.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;
    use std::path::PathBuf;

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::note::{Frontmatter, Note, NoteSource, Privacy};

    fn make_note(id: &str, wikilinks: Vec<String>) -> Note {
        Note {
            path: PathBuf::from(format!("vault/{id}.md")),
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
                summary: format!("summary for {id}"),
                tags: Vec::new(),
                refs: Vec::new(),
            },
            body: format!("body for {id}"),
            wikilinks,
        }
    }

    fn setup_index() -> Index {
        Index::open(Path::new(":memory:")).expect("open in-memory index")
    }

    #[test]
    fn spread_adds_wikilink_neighbors() {
        let index = setup_index();
        let note_a = make_note("note-a", vec!["note-b".to_string()]);
        let note_b = make_note("note-b", vec![]);
        index
            .upsert_note(&note_a, &note_a.body)
            .expect("upsert note a");
        index
            .upsert_note(&note_b, &note_b.body)
            .expect("upsert note b");

        let seeds = vec![RetrievalHit {
            id: "note-a".to_string(),
            score: 1.0,
            source: HitSource::Bm25,
        }];

        let hits = spread(&seeds, &index, 5).expect("spread retrieval");
        assert!(hits.iter().any(|hit| hit.id == "note-b"));
    }

    #[test]
    fn spread_adds_hebbian_neighbors() {
        let index = setup_index();
        let note_a = make_note("note-a", vec![]);
        let note_b = make_note("note-b", vec![]);
        index
            .upsert_note(&note_a, &note_a.body)
            .expect("upsert note a");
        index
            .upsert_note(&note_b, &note_b.body)
            .expect("upsert note b");

        index
            .with_transaction(|tx| {
                tx.execute(
                    "INSERT INTO hebbian_edges (a_id, b_id, weight, last_coactivated)
                     VALUES (?, ?, ?, ?)",
                    rusqlite::params!["note-a", "note-b", 1.0_f32, Utc::now().to_rfc3339()],
                )?;
                Ok(())
            })
            .expect("insert hebbian edge");

        let seeds = vec![RetrievalHit {
            id: "note-a".to_string(),
            score: 1.0,
            source: HitSource::Vector,
        }];

        let hits = spread(&seeds, &index, 5).expect("spread retrieval");
        assert!(hits.iter().any(|hit| hit.id == "note-b"));
    }

    #[test]
    fn spread_combines_wikilink_and_hebbian_without_duplicates() {
        let index = setup_index();
        let note_a = make_note("note-a", vec!["note-c".to_string()]);
        let note_b = make_note("note-b", vec![]);
        let note_c = make_note("note-c", vec![]);
        index
            .upsert_note(&note_a, &note_a.body)
            .expect("upsert note a");
        index
            .upsert_note(&note_b, &note_b.body)
            .expect("upsert note b");
        index
            .upsert_note(&note_c, &note_c.body)
            .expect("upsert note c");

        index
            .with_transaction(|tx| {
                tx.execute(
                    "INSERT INTO hebbian_edges (a_id, b_id, weight, last_coactivated)
                     VALUES (?, ?, ?, ?)",
                    rusqlite::params!["note-a", "note-b", 1.2_f32, Utc::now().to_rfc3339()],
                )?;
                Ok(())
            })
            .expect("insert hebbian edge");

        let seeds = vec![RetrievalHit {
            id: "note-a".to_string(),
            score: 1.0,
            source: HitSource::Both,
        }];
        let hits = spread(&seeds, &index, 10).expect("spread retrieval");

        assert!(hits.iter().any(|hit| hit.id == "note-b"));
        assert!(hits.iter().any(|hit| hit.id == "note-c"));
        let unique_ids = hits
            .iter()
            .map(|hit| hit.id.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(unique_ids.len(), hits.len());
    }
}
