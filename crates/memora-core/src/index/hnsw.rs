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

        Self::cleanup_stale_tmp_artifacts(path)?;
        let bin_path = Self::bin_path(path);
        let graph_path = Self::hnsw_graph_path(path)?;
        let data_path = Self::hnsw_data_path(path)?;
        let has_bin = bin_path.exists();
        let has_graph = graph_path.exists();
        let has_data = data_path.exists();

        if has_bin && has_graph && has_data {
            match Self::try_load(path, dim, &bin_path) {
                Ok(index) => return Ok(index),
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %error,
                        "failed to load persisted vector index; rebuilding from empty"
                    );
                    Self::cleanup_all_index_artifacts(path)?;
                }
            }
        } else if has_bin || has_graph || has_data {
            tracing::warn!(
                path = %path.display(),
                has_bin,
                has_graph,
                has_data,
                "partial vector index state detected; removing artifacts and rebuilding from empty"
            );
            Self::cleanup_all_index_artifacts(path)?;
        }

        Ok(Self::new_empty(path, dim, false))
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
        if self.hnsw.get_nb_point() == 0 {
            Self::cleanup_all_index_artifacts(&self.path)?;
            self.dirty = false;
            return Ok(());
        }

        let encoded = bincode::serialize(&persisted).context("serialize vector index metadata")?;
        let bin_path = Self::bin_path(&self.path);
        let (dir, final_basename) = Self::hnsw_dir_and_basename(&self.path)?;
        let bin_tmp = Self::tmp_path(&bin_path);
        let tmp_basename = format!("{final_basename}.tmp");
        let final_graph = dir.join(format!("{final_basename}.hnsw.graph"));
        let final_data = dir.join(format!("{final_basename}.hnsw.data"));
        let tmp_graph = dir.join(format!("{tmp_basename}.hnsw.graph"));
        let tmp_data = dir.join(format!("{tmp_basename}.hnsw.data"));
        let save_result = (|| -> Result<()> {
            fs::write(&bin_tmp, encoded)
                .with_context(|| format!("write vector metadata {}", bin_tmp.display()))?;
            self.hnsw
                .file_dump(&dir, &tmp_basename)
                .with_context(|| format!("dump hnsw graph {}", self.path.display()))?;
            Self::replace_file(&tmp_graph, &final_graph)?;
            Self::replace_file(&tmp_data, &final_data)?;
            Self::replace_file(&bin_tmp, &bin_path)?;
            Ok(())
        })();
        if let Err(error) = save_result {
            Self::remove_if_exists(&bin_tmp)?;
            Self::remove_if_exists(&tmp_graph)?;
            Self::remove_if_exists(&tmp_data)?;
            return Err(error);
        }

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

    fn try_load(path: &Path, dim: usize, bin_path: &Path) -> Result<Self> {
        let bytes = fs::read(bin_path)
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
        Ok(Self {
            hnsw,
            id_to_idx: persisted.id_to_idx,
            idx_to_id: persisted.idx_to_id,
            dim,
            path: path.to_path_buf(),
            dirty: false,
        })
    }

    fn new_empty(path: &Path, dim: usize, dirty: bool) -> Self {
        let hnsw = Hnsw::new(
            DEFAULT_MAX_CONNECTIONS,
            DEFAULT_MAX_ELEMENTS_HINT,
            DEFAULT_MAX_LAYER,
            DEFAULT_EF_CONSTRUCTION,
            DistCosine {},
        );
        Self {
            hnsw,
            id_to_idx: HashMap::new(),
            idx_to_id: Vec::new(),
            dim,
            path: path.to_path_buf(),
            dirty,
        }
    }

    fn bin_path(path: &Path) -> PathBuf {
        PathBuf::from(format!("{}.bin", path.display()))
    }

    fn tmp_path(path: &Path) -> PathBuf {
        PathBuf::from(format!("{}.tmp", path.display()))
    }

    fn hnsw_graph_path(path: &Path) -> Result<PathBuf> {
        let (dir, basename) = Self::hnsw_dir_and_basename(path)?;
        Ok(dir.join(format!("{basename}.hnsw.graph")))
    }

    fn hnsw_data_path(path: &Path) -> Result<PathBuf> {
        let (dir, basename) = Self::hnsw_dir_and_basename(path)?;
        Ok(dir.join(format!("{basename}.hnsw.data")))
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

    fn remove_if_exists(path: &Path) -> Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("remove file {}", path.display())),
        }
    }

    fn replace_file(src: &Path, dst: &Path) -> Result<()> {
        Self::remove_if_exists(dst)?;
        fs::rename(src, dst)
            .with_context(|| format!("atomically replace {} -> {}", src.display(), dst.display()))
    }

    fn cleanup_all_index_artifacts(path: &Path) -> Result<()> {
        let bin = Self::bin_path(path);
        let bin_tmp = Self::tmp_path(&bin);
        let graph = Self::hnsw_graph_path(path)?;
        let data = Self::hnsw_data_path(path)?;
        let (dir, basename) = Self::hnsw_dir_and_basename(path)?;
        let tmp_graph = dir.join(format!("{basename}.tmp.hnsw.graph"));
        let tmp_data = dir.join(format!("{basename}.tmp.hnsw.data"));
        Self::remove_if_exists(&bin)?;
        Self::remove_if_exists(&bin_tmp)?;
        Self::remove_if_exists(&graph)?;
        Self::remove_if_exists(&data)?;
        Self::remove_if_exists(&tmp_graph)?;
        Self::remove_if_exists(&tmp_data)?;
        Ok(())
    }

    fn cleanup_stale_tmp_artifacts(path: &Path) -> Result<()> {
        let parent = path.parent().unwrap_or(Path::new("."));
        let basename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
        for entry in fs::read_dir(parent)
            .with_context(|| format!("scan vector index directory {}", parent.display()))?
        {
            let entry = entry.with_context(|| format!("read entry in {}", parent.display()))?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let matches = name_str.ends_with(".tmp")
                || name_str.contains(".tmp.hnsw.")
                || name_str.starts_with(&format!("{basename}.tmp"));
            if matches {
                let file_path = entry.path();
                if file_path.is_file() {
                    Self::remove_if_exists(&file_path)?;
                    tracing::warn!(
                        path = %file_path.display(),
                        "removed stale temp artifact from prior failed save"
                    );
                }
            }
        }
        Ok(())
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
    use std::fs;

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

    #[test]
    fn save_then_load_roundtrip() -> Result<()> {
        let temp = tempdir()?;
        let path = temp.path().join("vectors");
        let mut index = VectorIndex::open_or_create(&path, 32)?;
        let mut rng = StdRng::seed_from_u64(7);
        let mut vectors = Vec::new();
        for i in 0..100usize {
            let vec = (0..32)
                .map(|_| rng.gen_range(-1.0..1.0))
                .collect::<Vec<f32>>();
            index.upsert(&format!("id-{i}"), &vec)?;
            vectors.push(vec);
        }
        let expected = index.search(&vectors[18], 5)?;
        index.save()?;
        drop(index);

        let loaded = VectorIndex::open_or_create(&path, 32)?;
        let actual = loaded.search(&vectors[18], 5)?;
        assert_eq!(expected, actual);
        Ok(())
    }

    #[test]
    fn save_recovers_from_stale_tmp_files() -> Result<()> {
        let temp = tempdir()?;
        let path = temp.path().join("vectors");
        fs::write(temp.path().join("vectors.tmp"), b"stale")?;
        fs::write(temp.path().join("vectors.tmp.hnsw.graph"), b"stale")?;
        fs::write(temp.path().join("vectors.tmp.hnsw.data"), b"stale")?;

        let _index = VectorIndex::open_or_create(&path, 8)?;
        assert!(!temp.path().join("vectors.tmp").exists());
        assert!(!temp.path().join("vectors.tmp.hnsw.graph").exists());
        assert!(!temp.path().join("vectors.tmp.hnsw.data").exists());
        Ok(())
    }

    #[test]
    fn load_recovers_from_partial_state() -> Result<()> {
        let temp = tempdir()?;
        let path = temp.path().join("vectors");
        let mut index = VectorIndex::open_or_create(&path, 16)?;
        let vec = vec![0.5; 16];
        index.upsert("a", &vec)?;
        index.save()?;
        fs::remove_file(temp.path().join("vectors.hnsw.graph"))?;
        fs::remove_file(temp.path().join("vectors.hnsw.data"))?;

        let recovered = VectorIndex::open_or_create(&path, 16)?;
        let results = recovered.search(&vec, 3)?;
        assert!(results.is_empty());
        assert!(!temp.path().join("vectors.bin").exists());
        Ok(())
    }

    #[test]
    fn save_atomic_failure_does_not_corrupt() -> Result<()> {
        let temp = tempdir()?;
        let path = temp.path().join("vectors");
        let mut index = VectorIndex::open_or_create(&path, 8)?;
        let vec = vec![0.3; 8];
        index.upsert("seed", &vec)?;
        index.save()?;
        fs::remove_file(temp.path().join("vectors.hnsw.graph"))?;
        fs::remove_file(temp.path().join("vectors.hnsw.data"))?;

        let mut partial = VectorIndex::open_or_create(&path, 8)?;
        partial.save()?;
        assert!(!temp.path().join("vectors.bin").exists());
        assert!(!temp.path().join("vectors.tmp.hnsw.graph").exists());
        assert!(!temp.path().join("vectors.tmp.hnsw.data").exists());

        let recovered = VectorIndex::open_or_create(&path, 8)?;
        let results = recovered.search(&vec, 3)?;
        assert!(results.is_empty());
        Ok(())
    }
}
