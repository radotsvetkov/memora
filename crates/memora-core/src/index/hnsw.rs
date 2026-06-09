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
    /// Live vectors by id. `hnsw_rs` has no delete and cannot hand its points
    /// back, so we keep our own copy of the live vectors to rebuild (compact) the
    /// graph and drop tombstoned entries. Bounded by the live note count.
    vectors: HashMap<String, Vec<f32>>,
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
        self.vectors.insert(id.to_string(), vec.to_vec());
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
            self.vectors.remove(id);
            self.dirty = true;
        }
        Ok(())
    }

    /// Number of tombstoned slots (deleted or superseded by re-upsert) still
    /// carried in the underlying graph. These never leave `hnsw_rs` on their own.
    pub fn tombstone_count(&self) -> usize {
        self.idx_to_id.len().saturating_sub(self.id_to_idx.len())
    }

    /// Rebuild the graph from the live vectors only, discarding tombstoned points.
    ///
    /// `hnsw_rs` cannot delete points, so deletes and re-upserts accumulate dead
    /// vectors that both bloat the graph and crowd out live results in search
    /// (which only over-fetches). Compaction reclaims them. Returns `true` if it
    /// rebuilt. It is a no-op when there is nothing to reclaim, or when the live
    /// vectors are not fully known (an index loaded from an older on-disk format
    /// that predates the stored-vectors field — a re-index repopulates them).
    pub fn compact(&mut self) -> Result<bool> {
        let live_count = self.id_to_idx.len();
        if self.tombstone_count() == 0 {
            return Ok(false);
        }
        // Collect live (id, vector) pairs in a stable order. If any live id lacks
        // a stored vector we cannot faithfully rebuild, so bail without changing
        // anything.
        let mut live: Vec<(String, Vec<f32>)> = Vec::with_capacity(live_count);
        for maybe in &self.idx_to_id {
            let Some(id) = maybe else { continue };
            match self.vectors.get(id) {
                Some(vec) => live.push((id.clone(), vec.clone())),
                None => return Ok(false),
            }
        }
        debug_assert_eq!(live.len(), live_count);

        let hnsw = Self::new_hnsw();
        let mut id_to_idx = HashMap::with_capacity(live.len());
        let mut idx_to_id = Vec::with_capacity(live.len());
        let mut vectors = HashMap::with_capacity(live.len());
        for (id, vec) in live {
            let idx = idx_to_id.len();
            hnsw.insert((vec.as_slice(), idx));
            idx_to_id.push(Some(id.clone()));
            id_to_idx.insert(id.clone(), idx);
            vectors.insert(id, vec);
        }

        self.hnsw = hnsw;
        self.id_to_idx = id_to_idx;
        self.idx_to_id = idx_to_id;
        self.vectors = vectors;
        self.dirty = true;
        Ok(true)
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
            vectors: self.vectors.clone(),
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

    /// Decode the metadata, accepting both the current format and the
    /// pre-0.1.30 format that had no stored `vectors`.
    ///
    /// bincode is positional and non-self-describing, so a newly added field
    /// cannot be defaulted in place: deserializing an old payload into the new
    /// struct hits EOF and errors. We therefore try the current layout first and,
    /// on failure, fall back to the old layout and lift it with an empty
    /// `vectors` map. Compaction stays disabled (a no-op) until the next
    /// re-index repopulates the vectors — but the existing graph loads intact,
    /// so search keeps working across the upgrade with no forced re-embed.
    fn decode_persisted(bytes: &[u8]) -> Result<PersistedVectorIndex> {
        if let Ok(current) = bincode::deserialize::<PersistedVectorIndex>(bytes) {
            return Ok(current);
        }
        let legacy: PersistedVectorIndexV1 = bincode::deserialize(bytes)
            .context("deserialize vector index metadata (legacy fallback)")?;
        Ok(PersistedVectorIndex {
            id_to_idx: legacy.id_to_idx,
            idx_to_id: legacy.idx_to_id,
            vectors: HashMap::new(),
            dim: legacy.dim,
        })
    }

    fn try_load(path: &Path, dim: usize, bin_path: &Path) -> Result<Self> {
        let bytes = fs::read(bin_path)
            .with_context(|| format!("read vector metadata {}", bin_path.display()))?;
        let persisted = Self::decode_persisted(&bytes)?;
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
            vectors: persisted.vectors,
            dim,
            path: path.to_path_buf(),
            dirty: false,
        })
    }

    fn new_hnsw() -> Hnsw<'static, f32, DistCosine> {
        Hnsw::new(
            DEFAULT_MAX_CONNECTIONS,
            DEFAULT_MAX_ELEMENTS_HINT,
            DEFAULT_MAX_LAYER,
            DEFAULT_EF_CONSTRUCTION,
            DistCosine {},
        )
    }

    fn new_empty(path: &Path, dim: usize, dirty: bool) -> Self {
        Self {
            hnsw: Self::new_hnsw(),
            id_to_idx: HashMap::new(),
            idx_to_id: Vec::new(),
            vectors: HashMap::new(),
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
    // Live vectors, needed for compaction. Old indexes that predate this field
    // are handled by the explicit legacy fallback in `decode_persisted` (bincode
    // cannot default a missing field), not by serde defaults.
    vectors: HashMap<String, Vec<f32>>,
    dim: usize,
}

/// The pre-0.1.30 on-disk metadata layout (no stored `vectors`). Kept only so
/// `decode_persisted` can read indexes written by earlier versions.
#[derive(Debug, Serialize, Deserialize)]
struct PersistedVectorIndexV1 {
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
    fn compact_reclaims_tombstones_and_preserves_search() -> Result<()> {
        let temp = tempdir()?;
        let mut index = VectorIndex::open_or_create(&temp.path().join("vectors"), 4)?;
        index.upsert("a", &[1.0, 0.0, 0.0, 0.0])?;
        index.upsert("b", &[0.0, 1.0, 0.0, 0.0])?;
        index.upsert("c", &[0.0, 0.0, 1.0, 0.0])?;
        index.upsert("d", &[0.0, 0.0, 0.0, 1.0])?;
        // Tombstones: re-upsert `a` (moves it), delete `c`.
        index.upsert("a", &[0.9, 0.1, 0.0, 0.0])?;
        index.delete("c")?;
        assert_eq!(index.tombstone_count(), 2);

        assert!(index.compact()?, "compaction should rebuild");
        assert_eq!(index.tombstone_count(), 0, "tombstones reclaimed");

        assert_eq!(index.search(&[1.0, 0.0, 0.0, 0.0], 1)?[0].0, "a");
        assert_eq!(index.search(&[0.0, 1.0, 0.0, 0.0], 1)?[0].0, "b");
        assert_eq!(index.search(&[0.0, 0.0, 0.0, 1.0], 1)?[0].0, "d");
        let near_c = index.search(&[0.0, 0.0, 1.0, 0.0], 4)?;
        assert!(
            !near_c.iter().any(|(id, _)| id == "c"),
            "deleted id must not resurface: {near_c:?}"
        );
        Ok(())
    }

    #[test]
    fn decode_persisted_reads_legacy_format_without_vectors() {
        // A pre-0.1.30 metadata payload had no `vectors` field. bincode is
        // positional, so it must be decoded via the explicit legacy fallback.
        let legacy = super::PersistedVectorIndexV1 {
            id_to_idx: std::collections::HashMap::from([("a".to_string(), 0usize)]),
            idx_to_id: vec![Some("a".to_string())],
            dim: 8,
        };
        let bytes = bincode::serialize(&legacy).expect("serialize legacy");

        // The current struct cannot deserialize the old bytes directly...
        assert!(
            bincode::deserialize::<super::PersistedVectorIndex>(&bytes).is_err(),
            "old bincode payload must not silently parse as the new struct"
        );
        // ...but the fallback lifts it, with vectors empty (compaction disabled
        // until a re-index repopulates them).
        let decoded = VectorIndex::decode_persisted(&bytes).expect("legacy fallback");
        assert_eq!(decoded.dim, 8);
        assert_eq!(decoded.idx_to_id, vec![Some("a".to_string())]);
        assert_eq!(decoded.id_to_idx.get("a"), Some(&0));
        assert!(decoded.vectors.is_empty());
    }

    #[test]
    fn compact_is_noop_without_tombstones() -> Result<()> {
        let temp = tempdir()?;
        let mut index = VectorIndex::open_or_create(&temp.path().join("vectors"), 4)?;
        index.upsert("a", &[1.0, 0.0, 0.0, 0.0])?;
        index.upsert("b", &[0.0, 1.0, 0.0, 0.0])?;
        assert_eq!(index.tombstone_count(), 0);
        assert!(!index.compact()?, "nothing to reclaim");
        Ok(())
    }

    #[test]
    fn compact_then_save_load_roundtrip_is_stable() -> Result<()> {
        let temp = tempdir()?;
        let path = temp.path().join("vectors");
        let mut index = VectorIndex::open_or_create(&path, 8)?;
        let mut rng = StdRng::seed_from_u64(11);
        let mut vectors = Vec::new();
        for i in 0..50usize {
            let v = (0..8)
                .map(|_| rng.gen_range(-1.0..1.0))
                .collect::<Vec<f32>>();
            index.upsert(&format!("id-{i}"), &v)?;
            vectors.push(v);
        }
        // Tombstone the first half by re-upserting it.
        for (i, v) in vectors.iter().enumerate().take(25) {
            index.upsert(&format!("id-{i}"), v)?;
        }
        assert_eq!(index.tombstone_count(), 25);

        assert!(index.compact()?);
        assert_eq!(index.tombstone_count(), 0);
        let expected = index.search(&vectors[40], 5)?;
        index.save()?;
        drop(index);

        let loaded = VectorIndex::open_or_create(&path, 8)?;
        let actual = loaded.search(&vectors[40], 5)?;
        assert_eq!(expected, actual, "search stable across compact + save/load");
        Ok(())
    }

    #[test]
    fn compact_handles_all_deleted() -> Result<()> {
        let temp = tempdir()?;
        let mut index = VectorIndex::open_or_create(&temp.path().join("vectors"), 4)?;
        index.upsert("a", &[1.0, 0.0, 0.0, 0.0])?;
        index.upsert("b", &[0.0, 1.0, 0.0, 0.0])?;
        index.delete("a")?;
        index.delete("b")?;
        assert!(index.compact()?);
        assert_eq!(index.tombstone_count(), 0);
        assert!(index.search(&[1.0, 0.0, 0.0, 0.0], 5)?.is_empty());
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
