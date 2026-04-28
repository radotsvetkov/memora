//! Memora core: vault scanning, note parsing, claim graph, retrieval, validation.

pub mod embed;
pub mod index;
pub mod indexer;
pub mod learn;
pub mod note;
pub mod retrieve;
pub mod vault;

pub use embed::{normalize_text, Embedder, OpenAiEmbedder};
pub use index::{Index, IndexError, NoteRow, RebuildStats, VectorIndex};
pub use learn::QValueLearner;
pub use note::{Frontmatter, Note, NoteSource, ParseError, Privacy};
pub use retrieve::{spread, HebbianLearner, HitSource, HybridRetriever, RetrievalHit};
pub use vault::{scan, Vault, VaultError, VaultEvent};
