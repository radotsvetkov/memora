//! Memora core: vault scanning, note parsing, claim graph, retrieval, validation.

pub mod embed;
pub mod index;
pub mod indexer;
pub mod note;
pub mod vault;

pub use embed::{normalize_text, Embedder, OpenAiEmbedder};
pub use index::{Index, IndexError, NoteRow, RebuildStats, VectorIndex};
pub use note::{Frontmatter, Note, NoteSource, ParseError, Privacy};
pub use vault::{scan, Vault, VaultError, VaultEvent};
