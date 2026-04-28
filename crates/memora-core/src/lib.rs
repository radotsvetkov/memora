//! Memora core: vault scanning, note parsing, claim graph, retrieval, validation.

pub mod note;
pub mod vault;

pub use note::{Frontmatter, Note, NoteSource, ParseError, Privacy};
pub use vault::{scan, Vault, VaultError, VaultEvent};
