use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use hnsw_rs::api::AnnT;
use hnsw_rs::hnsw::Hnsw;
use hnsw_rs::hnswio::HnswIo;
use hnsw_rs::prelude::DistCosine;
use serde::{Deserialize, Serialize};

const DEFAULT_EF_CONSTRUCTION: usize = 200;
const DEFAULT_MAX_CONNECTIONS: usize = 16;
const DEFAULT_MAX_LAYER: usize = 16;
const DEFAULT_MAX_ELEMENTS_HINT: usize = 100_000;

pub struct VectorIndex {
    hnsw: Hnsw<'static, f32, DistCosine>,
    id_to_idx: HashMap<String, usize>,
    idx_to_id: Vec<Option<String>>,
    dim: usize,
    path: PathBuf,
    dirty: bool,
}

impl VectorIndex {
    pub fn open_or_create(path: &Path, dim: usize) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create vector index dir {}", parent.display()))?;
        }

        let bin_path = Self::bin_path(path);
        if bin_path.exists() {
            let bytes = fs::read(&bin_path)
                .with_context(|| format!("read vector metadata {}", bin_path.display()))?;
            let persisted: PersistedVectorIndex =
                bincode::deserialize(&bytes).context("deserialize vector index metadata")?;
            if persisted.dim != dim {
                bail!(
                    "vector index dim mismatch for {}: on disk {}, requested {}",
                    path.display(),
                    persisted.dim,
                    dim
                );
            }
            let hnsw = Self::load_hnsw(path)?;
            return Ok(Self {
                hnsw,
                id_to_idx: persisted.id_to_idx,
                idx_to_id: persisted.idx_to_id,
                dim,
                path: path.to_path_buf(),
                dirty: false,
            });
        }

        let hnsw = Hnsw::new(
            DEFAULT_MAX_CONNECTIONS,
            DEFAULT_MAX_ELEMENTS_HINT,
            DEFAULT_MAX_LAYER,
            DEFAULT_EF_CONSTRUCTION,
            DistCosine {},
        );
        Ok(Self {
            hnsw,
            id_to_idx: HashMap::new(),
            idx_to_id: Vec::new(),
            dim,
            path: path.to_path_buf(),
            dirty: true,
        })
    }

    pub fn upsert(&mut self, id: &str, vec: &[f32]) -> Result<()> {
        self.ensure_dim(vec)?;
        if let Some(old_idx) = self.id_to_idx.remove(id) {
            if old_idx < self.idx_to_id.len() {
                self.idx_to_id[old_idx] = None;
            }
        }

        let next_idx = self.idx_to_id.len();
        self.hnsw.insert((vec, next_idx));
        self.idx_to_id.push(Some(id.to_string()));
        self.id_to_idx.insert(id.to_string(), next_idx);
        self.dirty = true;
        Ok(())
    }

    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>> {
        self.ensure_dim(query)?;
        if k == 0 {
            return Ok(Vec::new());
        }

        let fetch_k = k.saturating_mul(2).max(k);
        let ef = fetch_k.max(DEFAULT_EF_CONSTRUCTION);
        let neighbors = self.hnsw.search(query, fetch_k, ef);

        let mut out = Vec::with_capacity(k);
        for neighbor in neighbors {
            let idx = neighbor.d_id;
            let Some(Some(id)) = self.idx_to_id.get(idx) else {
                continue;
            };
            // hnsw_rs returns cosine distance, convert to cosine similarity.
            let score = 1.0 - neighbor.distance;
            out.push((id.clone(), score));
            if out.len() == k {
                break;
            }
        }
        Ok(out)
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        if let Some(idx) = self.id_to_idx.remove(id) {
            if idx < self.idx_to_id.len() {
                self.idx_to_id[idx] = None;
            }
            self.dirty = true;
        }
        Ok(())
    }

    pub fn save(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create vector index dir {}", parent.display()))?;
        }

        let persisted = PersistedVectorIndex {
            id_to_idx: self.id_to_idx.clone(),
            idx_to_id: self.idx_to_id.clone(),
            dim: self.dim,
        };
        let encoded = bincode::serialize(&persisted).context("serialize vector index metadata")?;
        let bin_path = Self::bin_path(&self.path);
        let bin_tmp = PathBuf::from(format!("{}.tmp", bin_path.display()));
        fs::write(&bin_tmp, encoded)
            .with_context(|| format!("write vector metadata {}", bin_tmp.display()))?;
        fs::rename(&bin_tmp, &bin_path).with_context(|| {
            format!(
                "atomically replace vector metadata {} -> {}",
                bin_tmp.display(),
                bin_path.display()
            )
        })?;

        let (dir, final_basename) = Self::hnsw_dir_and_basename(&self.path)?;
        let tmp_basename = format!("{final_basename}.tmp");
        self.hnsw
            .file_dump(&dir, &tmp_basename)
            .with_context(|| format!("dump hnsw graph {}", self.path.display()))?;

        let final_graph = dir.join(format!("{final_basename}.hnsw.graph"));
        let final_data = dir.join(format!("{final_basename}.hnsw.data"));
        let tmp_graph = dir.join(format!("{tmp_basename}.hnsw.graph"));
        let tmp_data = dir.join(format!("{tmp_basename}.hnsw.data"));
        fs::rename(&tmp_graph, &final_graph).with_context(|| {
            format!(
                "atomically replace hnsw graph {} -> {}",
                tmp_graph.display(),
                final_graph.display()
            )
        })?;
        fs::rename(&tmp_data, &final_data).with_context(|| {
            format!(
                "atomically replace hnsw data {} -> {}",
                tmp_data.display(),
                final_data.display()
            )
        })?;

        self.dirty = false;
        Ok(())
    }

    fn ensure_dim(&self, vec: &[f32]) -> Result<()> {
        if vec.len() != self.dim {
            bail!(
                "vector dim mismatch for {}: expected {}, got {}",
                self.path.display(),
                self.dim,
                vec.len()
            );
        }
        Ok(())
    }

    /// Loads an existing HNSW graph from disk.
    ///
    /// This intentionally uses `Box::leak` for `HnswIo` because `hnsw_rs` requires
    /// `HnswIo` to outlive the loaded `Hnsw`. The leak is bounded (at most once per
    /// process startup when an existing index is loaded), and we accept leaking this
    /// small struct rather than restructuring ownership in this phase.
    ///
    /// Cleanup is tracked for Phase 12.
    fn load_hnsw(path: &Path) -> Result<Hnsw<'static, f32, DistCosine>> {
        let (dir, basename) = Self::hnsw_dir_and_basename(path)?;
        let io = Box::leak(Box::new(HnswIo::new(&dir, &basename)));
        io.load_hnsw::<f32, DistCosine>()
            .with_context(|| format!("load hnsw graph for {}", path.display()))
    }

    fn bin_path(path: &Path) -> PathBuf {
        PathBuf::from(format!("{}.bin", path.display()))
    }

    fn hnsw_dir_and_basename(path: &Path) -> Result<(PathBuf, String)> {
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let Some(file_name) = path.file_name().and_then(|f| f.to_str()) else {
            bail!("invalid vector index path: {}", path.display());
        };
        Ok((dir, file_name.to_string()))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedVectorIndex {
    id_to_idx: HashMap<String, usize>,
    idx_to_id: Vec<Option<String>>,
    dim: usize,
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use tempfile::tempdir;

    use super::VectorIndex;

    #[test]
    fn search_returns_exact_match_top1() -> Result<()> {
        let temp = tempdir()?;
        let mut index = VectorIndex::open_or_create(&temp.path().join("vectors"), 64)?;
        let mut rng = StdRng::seed_from_u64(42);
        let mut vectors = Vec::new();
        for i in 0..100usize {
            let vec = (0..64)
                .map(|_| rng.gen_range(-1.0..1.0))
                .collect::<Vec<f32>>();
            index.upsert(&format!("id-{i}"), &vec)?;
            vectors.push(vec);
        }

        let query = vectors[37].clone();
        let results = index.search(&query, 5)?;
        assert_eq!(results[0].0, "id-37");
        assert!(results[0].1 > 0.99);
        Ok(())
    }

    #[test]
    fn tombstoned_upsert_uses_latest_vector() -> Result<()> {
        let temp = tempdir()?;
        let mut index = VectorIndex::open_or_create(&temp.path().join("vectors"), 4)?;

        let old = vec![1.0, 0.0, 0.0, 0.0];
        let new = vec![0.0, 1.0, 0.0, 0.0];
        index.upsert("a", &old)?;
        index.upsert("b", &old)?;
        index.upsert("a", &new)?;

        let results_old = index.search(&old, 1)?;
        assert_eq!(results_old[0].0, "b");

        let results_new = index.search(&new, 1)?;
        assert_eq!(results_new[0].0, "a");
        assert!(results_new[0].1 > 0.99);
        Ok(())
    }
}
