use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, NaiveTime, TimeDelta};
use memora_llm::LlmClient;
use tokio::task::JoinHandle;

use crate::challenger::{Challenger, ChallengerConfig};
use crate::claims::ClaimStore;
use crate::consolidate::{AtlasWriter, WorldMapWriter};
use crate::index::Index;

#[derive(Debug, Clone)]
pub struct ConsolidationScheduleConfig {
    pub daily_at: String,
}

impl Default for ConsolidationScheduleConfig {
    fn default() -> Self {
        Self {
            daily_at: "03:00".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChallengerScheduleConfig {
    pub daily_at: String,
}

impl Default for ChallengerScheduleConfig {
    fn default() -> Self {
        Self {
            daily_at: "07:00".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SchedulerConfig {
    pub consolidation: ConsolidationScheduleConfig,
    pub challenger: ChallengerScheduleConfig,
}

pub struct Scheduler {
    handles: Vec<JoinHandle<()>>,
}

impl Scheduler {
    pub fn spawn(
        config: SchedulerConfig,
        db: Arc<Index>,
        llm: Arc<dyn LlmClient>,
        vault: PathBuf,
    ) -> Self {
        let consolidation_vault = vault.clone();
        let consolidation_llm = llm.clone();
        let consolidation_db = db.clone();
        let consolidation_cfg = config.consolidation.daily_at.clone();
        let consolidation_handle = tokio::spawn(async move {
            loop {
                let delay = time_until_next_tick(&consolidation_cfg);
                tokio::time::sleep(delay).await;

                let claim_store = ClaimStore::new(&consolidation_db);
                let atlas = AtlasWriter {
                    db: &consolidation_db,
                    claim_store: &claim_store,
                    llm: consolidation_llm.as_ref(),
                    vault: &consolidation_vault,
                };
                let world = WorldMapWriter {
                    db: &consolidation_db,
                    claim_store: &claim_store,
                    llm: consolidation_llm.as_ref(),
                    vault: &consolidation_vault,
                };

                let report = atlas.rebuild_all_changed().await;
                if !report.failed_regions.is_empty() {
                    tracing::warn!(
                        failures = report.failed_regions.len(),
                        "atlas scheduler tick completed with failures"
                    );
                }
                if let Err(err) = world.rebuild().await {
                    tracing::warn!(error = %err, "world map scheduler tick failed");
                }
            }
        });

        let challenger_vault = vault.clone();
        let challenger_llm = llm.clone();
        let challenger_db = db.clone();
        let challenger_cfg = config.challenger.daily_at.clone();
        let challenger_handle = tokio::spawn(async move {
            loop {
                let delay = time_until_next_tick(&challenger_cfg);
                tokio::time::sleep(delay).await;

                let claim_store = ClaimStore::new(&challenger_db);
                let challenger = Challenger {
                    db: &challenger_db,
                    claim_store: &claim_store,
                    llm: challenger_llm.as_ref(),
                    vault: &challenger_vault,
                    config: ChallengerConfig::default(),
                };

                match challenger.run_once().await {
                    Ok(report) => {
                        if let Err(err) = challenger.persist_report(&report) {
                            tracing::warn!(error = %err, "challenger scheduler write failed");
                        }
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "challenger scheduler tick failed");
                    }
                }
            }
        });

        Self {
            handles: vec![consolidation_handle, challenger_handle],
        }
    }

    pub fn abort(self) {
        for handle in self.handles {
            handle.abort();
        }
    }
}

fn time_until_next_tick(daily_at: &str) -> Duration {
    let fallback = NaiveTime::from_hms_opt(3, 0, 0).expect("03:00 should be valid");
    let target = NaiveTime::parse_from_str(daily_at, "%H:%M").unwrap_or(fallback);
    let now = Local::now();
    let today_target = now.date_naive().and_time(target);
    let next = if today_target > now.naive_local() {
        today_target
    } else {
        today_target + TimeDelta::days(1)
    };
    let seconds = (next - now.naive_local()).num_seconds().max(1);
    Duration::from_secs(seconds as u64)
}
