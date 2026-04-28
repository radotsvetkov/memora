pub mod hebbian;
pub mod hybrid;
pub mod spread;

pub use hebbian::HebbianLearner;
pub use hybrid::{HitSource, HybridRetriever, RetrievalHit};
pub use spread::spread;
