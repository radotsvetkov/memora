//! Personal-vault quality harness — NOT YET IMPLEMENTED.
//!
//! This binary intentionally emits NO quality numbers. Retrieval accuracy,
//! contradiction precision/recall, and privacy-leak rate over a real personal
//! vault depend on a labeled corpus and live LLM extraction, which this repo
//! does not yet ship. Printing placeholder constants here previously created the
//! false impression of measured results; that has been removed.
//!
//! For the one core metric that IS measured deterministically (and needs no
//! API key), run:
//!
//!   cargo run -p memora-bench --release --bin bench_citation_rejection
//!
//! To build the full personal-vault harness, wire this to: a fixed labeled
//! vault, the real index/extract/retrieve pipeline, and gold annotations, then
//! report metrics with their methodology.
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!(
        "bench_personal_vault is not implemented and emits no metrics.\n\
         Run `cargo run -p memora-bench --release --bin bench_citation_rejection` \
         for the measured citation-rejection metric."
    );
    Ok(())
}
