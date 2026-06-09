//! Memora core: vault scanning, note parsing, claim graph, retrieval, validation.

pub mod answer;
pub mod challenger;
pub mod cite;
pub mod claims;
pub mod config;
pub mod consolidate;
pub mod embed;
pub mod facade;
pub mod index;
pub mod indexer;
pub mod note;
pub mod privacy;
pub mod retrieve;
pub mod scheduler;
pub mod vault;
pub mod vault_config;
pub mod vault_path;

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
pub use embed::{
    build_embedder, normalize_text, DeterministicEmbedder, Embedder, OllamaEmbedder, OpenAiEmbedder,
};
pub use facade::Memora;
pub use index::{Index, IndexError, NoteRow, RebuildStats, VectorIndex};
pub use note::{Frontmatter, Note, NoteSource, ParseError, Privacy};
pub use privacy::{PrivacyFilter, RedactedClaim, RedactionStats};
pub use retrieve::{HitSource, HybridRetriever, RetrievalHit};
pub use scheduler::{
    ChallengerScheduleConfig, ConsolidationScheduleConfig, Scheduler, SchedulerConfig,
};
pub use vault::{scan, Vault, VaultError, VaultEvent};
pub use vault_config::VaultConfig;
pub use vault_path::{join_vault_relative, resolve_note_path, validate_region, VaultPathError};
