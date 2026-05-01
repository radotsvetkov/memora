//! Memora core: vault scanning, note parsing, claim graph, retrieval, validation.

pub mod answer;
pub mod challenger;
pub mod cite;
pub mod claims;
pub mod config;
pub mod consolidate;
pub mod embed;
pub mod index;
pub mod indexer;
pub mod learn;
pub mod note;
pub mod privacy;
pub mod retrieve;
pub mod scheduler;
pub mod vault;

pub use answer::AnsweringPipeline;
pub use challenger::{
    Challenger, ChallengerConfig, ChallengerReport, ContradictionAlert, CrossRegionAlert,
    FrontierAlert, StaleAlert,
};
pub use cite::{parse_claim_markers, CitationStatus, CitationValidator, CitedAnswer};
pub use claims::{
    Claim, ClaimExtractor, ClaimRelation, ClaimStore, ContradictionDetector, Provenance,
    StalenessTracker,
};
pub use config::PrivacyConfig;
pub use consolidate::{AtlasWriter, WorldMapWriter};
pub use embed::{normalize_text, Embedder, OllamaEmbedder, OpenAiEmbedder};
pub use index::{Index, IndexError, NoteRow, RebuildStats, VectorIndex};
pub use learn::QValueLearner;
pub use note::{Frontmatter, Note, NoteSource, ParseError, Privacy};
pub use privacy::{PrivacyFilter, RedactedClaim, RedactionStats};
pub use retrieve::{spread, HebbianLearner, HitSource, HybridRetriever, RetrievalHit};
pub use scheduler::{
    ChallengerScheduleConfig, ConsolidationScheduleConfig, Scheduler, SchedulerConfig,
};
pub use vault::{scan, Vault, VaultError, VaultEvent};
