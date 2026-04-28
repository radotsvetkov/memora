use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use memora_core::answer::AnsweringPipeline;
use memora_core::challenger::Challenger;
use memora_core::cite::{CitationStatus, CitationValidator};
use memora_core::claims::{ClaimStore, Provenance, StalenessTracker};
use memora_core::consolidate::{AtlasWriter, WorldMapWriter};
use memora_core::indexer::Indexer;
use memora_core::note::{self, Frontmatter, Note, NoteSource, Privacy};
use memora_core::{
    Embedder, HybridRetriever, Index, PrivacyConfig, PrivacyFilter, Vault, VaultEvent, VectorIndex,
};
use memora_llm::{
    make_client, CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError,
    LlmProvider,
};
use rmcp::handler::server::tool::Parameters;
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::{model::ErrorData as McpError, schemars, tool, ServerHandler};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct MemoraMcpServer {
    runtime: Arc<MemoraRuntime>,
}

impl MemoraMcpServer {
    pub fn new() -> Self {
        let runtime = MemoraRuntime::from_env().unwrap_or_else(|_| MemoraRuntime::default_paths());
        Self {
            runtime: Arc::new(runtime),
        }
    }
}

impl Default for MemoraMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool(tool_box)]
impl MemoraMcpServer {
    #[tool(description = "memora_query: {query, k?}")]
    async fn memora_query(
        &self,
        #[tool(aggr)] Parameters(input): Parameters<QueryInput>,
    ) -> Result<CallToolResult, McpError> {
        let value = self.runtime.query(input).await.map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_query_cited: {query, k?}")]
    async fn memora_query_cited(
        &self,
        #[tool(aggr)] Parameters(input): Parameters<QueryInput>,
    ) -> Result<CallToolResult, McpError> {
        let value = self
            .runtime
            .query_cited(input)
            .await
            .map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_get_note: {id}")]
    async fn memora_get_note(
        &self,
        #[tool(aggr)] Parameters(input): Parameters<NoteInput>,
    ) -> Result<CallToolResult, McpError> {
        let value = self.runtime.get_note(input).await.map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_get_atlas: {region}")]
    async fn memora_get_atlas(
        &self,
        #[tool(aggr)] Parameters(input): Parameters<RegionInput>,
    ) -> Result<CallToolResult, McpError> {
        let value = self.runtime.get_atlas(input).await.map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_get_world_map: {}")]
    async fn memora_get_world_map(&self) -> Result<CallToolResult, McpError> {
        let value = self.runtime.get_world_map().await.map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_neighbors: {id, top_n?}")]
    async fn memora_neighbors(
        &self,
        #[tool(aggr)] Parameters(input): Parameters<NeighborInput>,
    ) -> Result<CallToolResult, McpError> {
        let value = self.runtime.neighbors(input).await.map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_record_useful: {query_id, useful_ids}")]
    async fn memora_record_useful(
        &self,
        #[tool(aggr)] Parameters(input): Parameters<RecordUsefulInput>,
    ) -> Result<CallToolResult, McpError> {
        let value = self
            .runtime
            .record_useful(input)
            .await
            .map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_capture: {region, summary, body, tags, privacy?}")]
    async fn memora_capture(
        &self,
        #[tool(aggr)] Parameters(input): Parameters<CaptureInput>,
    ) -> Result<CallToolResult, McpError> {
        let value = self.runtime.capture(input).await.map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_consolidate: {scope}")]
    async fn memora_consolidate(
        &self,
        #[tool(aggr)] Parameters(input): Parameters<ConsolidateInput>,
    ) -> Result<CallToolResult, McpError> {
        let value = self
            .runtime
            .consolidate(input)
            .await
            .map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_verify_claim: {claim_id}")]
    async fn memora_verify_claim(
        &self,
        #[tool(aggr)] Parameters(input): Parameters<VerifyClaimInput>,
    ) -> Result<CallToolResult, McpError> {
        let value = self
            .runtime
            .verify_claim(input)
            .await
            .map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_stale_claims: {}")]
    async fn memora_stale_claims(&self) -> Result<CallToolResult, McpError> {
        let value = self.runtime.stale_claims().await.map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_contradictions: {subject?}")]
    async fn memora_contradictions(
        &self,
        #[tool(aggr)] Parameters(input): Parameters<ContradictionsInput>,
    ) -> Result<CallToolResult, McpError> {
        let value = self
            .runtime
            .contradictions(input)
            .await
            .map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_challenge: {}")]
    async fn memora_challenge(&self) -> Result<CallToolResult, McpError> {
        let value = self.runtime.challenge().await.map_err(to_mcp_error)?;
        json_tool_result(value)
    }

    #[tool(description = "memora_decisions: {}")]
    async fn memora_decisions(&self) -> Result<CallToolResult, McpError> {
        let value = self.runtime.decisions().await.map_err(to_mcp_error)?;
        json_tool_result(value)
    }
}

#[tool(tool_box)]
impl ServerHandler for MemoraMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Memora MCP server for verifiable vault memory.".to_string()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

fn to_mcp_error(err: impl std::fmt::Display) -> McpError {
    McpError::internal_error(err.to_string(), None)
}

fn json_tool_result(value: Value) -> Result<CallToolResult, McpError> {
    let content = Content::json(value).map_err(to_mcp_error)?;
    Ok(CallToolResult::success(vec![content]))
}

#[derive(Debug, Clone)]
pub struct MemoraRuntime {
    pub vault_root: PathBuf,
    pub index_db: PathBuf,
    pub vector_index: PathBuf,
}

impl MemoraRuntime {
    pub fn default_paths() -> Self {
        Self {
            vault_root: PathBuf::from("vault"),
            index_db: PathBuf::from(".memora/memora.db"),
            vector_index: PathBuf::from(".memora/vectors"),
        }
    }

    pub fn from_env() -> Result<Self> {
        Ok(Self {
            vault_root: PathBuf::from(
                std::env::var("MEMORA_VAULT").context("MEMORA_VAULT not set")?,
            ),
            index_db: PathBuf::from(
                std::env::var("MEMORA_INDEX_DB").context("MEMORA_INDEX_DB not set")?,
            ),
            vector_index: PathBuf::from(
                std::env::var("MEMORA_VECTOR_INDEX").context("MEMORA_VECTOR_INDEX not set")?,
            ),
        })
    }

    pub async fn query(&self, input: QueryInput) -> Result<Value> {
        let k = input.k.unwrap_or(5) as usize;
        let index = Index::open(&self.index_db)?;
        let embedder = DeterministicEmbedder::new(64);
        let vector = VectorIndex::open_or_create(&self.vector_index, embedder.dim())?;
        let retriever = HybridRetriever {
            index: &index,
            vec: &vector,
            embedder: &embedder,
        };
        let hits = retriever.search(&input.query, k).await?;
        let mut output_hits = Vec::new();
        let mut regions = BTreeSet::new();
        for hit in hits {
            let Some(row) = index.get_note(&hit.id)? else {
                continue;
            };
            let snippet = note_snippet(&self.vault_root, &row.path)?;
            regions.insert(row.region.clone());
            output_hits.push(json!({
                "id": row.id,
                "summary": row.summary,
                "region": row.region,
                "score": hit.score,
                "snippet": snippet
            }));
        }
        Ok(json!({
            "hits": output_hits,
            "regions_used": regions.into_iter().collect::<Vec<_>>()
        }))
    }

    pub async fn query_cited(&self, input: QueryInput) -> Result<Value> {
        let k = input.k.unwrap_or(5) as usize;
        let index = Index::open(&self.index_db)?;
        let embedder = DeterministicEmbedder::new(64);
        let vector = VectorIndex::open_or_create(&self.vector_index, embedder.dim())?;
        let claim_store = ClaimStore::new(&index);
        let retriever = HybridRetriever {
            index: &index,
            vec: &vector,
            embedder: &embedder,
        };
        let validator = CitationValidator {
            store: &claim_store,
            index: &index,
            vault_root: &self.vault_root,
        };
        let llm = build_llm(LlmProvider::Ollama, None)?;
        let pipeline = AnsweringPipeline {
            retriever: &retriever,
            claim_store: &claim_store,
            validator: &validator,
            llm: llm.as_ref(),
            privacy_filter: PrivacyFilter::new_for(LlmProvider::Ollama),
            privacy_config: PrivacyConfig::default(),
        };
        let mut answer = pipeline.answer(&input.query, k).await?;
        if answer.verified_count == 0 {
            let claim_hits = claim_store.search_fts(&input.query, 1)?;
            if let Some((claim_id, _)) = claim_hits.first() {
                answer = validator.validate(&format!("[claim:{claim_id}]")).await?;
            }
        }
        Ok(serialize_cited_answer(answer))
    }

    pub async fn get_note(&self, input: NoteInput) -> Result<Value> {
        let index = Index::open(&self.index_db)?;
        let row = index
            .get_note(&input.id)?
            .ok_or_else(|| anyhow!("note not found: {}", input.id))?;
        let parsed = note::parse(&resolve_note_path(&self.vault_root, &row.path))?;
        let hebbian = index.hebbian_neighbors(&row.id, 5)?;
        Ok(json!({
            "id": row.id,
            "region": row.region,
            "summary": row.summary,
            "body": parsed.body,
            "tags": parsed.fm.tags,
            "refs": parsed.fm.refs,
            "wikilinks": parsed.wikilinks,
            "hebbian_neighbors": hebbian.into_iter().map(|(id, score)| json!({"id": id, "score": score})).collect::<Vec<_>>()
        }))
    }

    pub async fn get_atlas(&self, input: RegionInput) -> Result<Value> {
        let atlas_markdown =
            fs::read_to_string(self.vault_root.join(&input.region).join("_atlas.md"))?;
        let conn = rusqlite::Connection::open(&self.index_db)?;
        let note_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE region = ?",
            params![input.region.as_str()],
            |row| row.get(0),
        )?;
        Ok(json!({
            "region": input.region,
            "atlas_markdown": atlas_markdown,
            "note_count": note_count
        }))
    }

    pub async fn get_world_map(&self) -> Result<Value> {
        let markdown = fs::read_to_string(self.vault_root.join("world_map.md"))?;
        Ok(json!({ "markdown": markdown }))
    }

    pub async fn neighbors(&self, input: NeighborInput) -> Result<Value> {
        let top_n = input.top_n.unwrap_or(5) as usize;
        let index = Index::open(&self.index_db)?;
        let hebbian = index.hebbian_neighbors(&input.id, top_n)?;
        let wikilinks = index.wikilink_targets(&input.id)?;
        Ok(json!({
            "hebbian": hebbian.into_iter().map(|(id, weight)| json!({"id": id, "weight": weight})).collect::<Vec<_>>(),
            "wikilinks": wikilinks
        }))
    }

    pub async fn record_useful(&self, input: RecordUsefulInput) -> Result<Value> {
        let conn = rusqlite::Connection::open(&self.index_db)?;
        conn.execute(
            "UPDATE retrievals SET marked_useful_json = ? WHERE query_id = ?",
            params![serde_json::to_string(&input.useful_ids)?, input.query_id],
        )?;
        Ok(json!({ "ok": true }))
    }

    pub async fn capture(&self, input: CaptureInput) -> Result<Value> {
        let id = format!("note-{}", Uuid::new_v4().simple());
        let rel_path = PathBuf::from(&input.region).join(format!("{id}.md"));
        let abs_path = self.vault_root.join(&rel_path);
        if let Some(parent) = abs_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let now = Utc::now();
        let note = Note {
            path: rel_path.clone(),
            fm: Frontmatter {
                id: id.clone(),
                region: input.region,
                source: NoteSource::Personal,
                privacy: parse_privacy(input.privacy.as_deref())?,
                created: now,
                updated: now,
                summary: input.summary,
                tags: input.tags,
                refs: Vec::new(),
            },
            body: input.body,
            wikilinks: Vec::new(),
        };
        fs::write(&abs_path, note::render(&note))?;

        let index = Index::open(&self.index_db)?;
        let embedder = Arc::new(DeterministicEmbedder::new(64));
        let vector = Arc::new(Mutex::new(VectorIndex::open_or_create(
            &self.vector_index,
            embedder.dim(),
        )?));
        let vault = Vault::new(self.vault_root.clone());
        let indexer = Indexer::new(&vault, &index, embedder, vector);
        indexer.handle_event(VaultEvent::Created(abs_path)).await?;
        Ok(json!({ "id": id, "path": rel_path.to_string_lossy().to_string() }))
    }

    pub async fn consolidate(&self, input: ConsolidateInput) -> Result<Value> {
        let index = Index::open(&self.index_db)?;
        let store = ClaimStore::new(&index);
        let llm = build_llm(LlmProvider::Ollama, None)?;
        let atlas = AtlasWriter {
            db: &index,
            claim_store: &store,
            llm: llm.as_ref(),
            vault: &self.vault_root,
        };
        let world = WorldMapWriter {
            db: &index,
            claim_store: &store,
            llm: llm.as_ref(),
            vault: &self.vault_root,
        };
        if input.scope == "all" {
            let report = atlas.rebuild_all_changed().await;
            world.rebuild().await?;
            return Ok(json!({
                "regions_rebuilt": report.rebuilt_regions,
                "notes_moved": 0
            }));
        }
        let region = input
            .scope
            .strip_prefix("region:")
            .ok_or_else(|| anyhow!("scope must be 'all' or 'region:<name>'"))?;
        atlas.rebuild_region(region).await?;
        world.rebuild().await?;
        Ok(json!({
            "regions_rebuilt": [region],
            "notes_moved": 0
        }))
    }

    pub async fn verify_claim(&self, input: VerifyClaimInput) -> Result<Value> {
        let index = Index::open(&self.index_db)?;
        let store = ClaimStore::new(&index);
        let Some(claim) = store.get(&input.claim_id)? else {
            return Ok(
                json!({ "exists": false, "span_intact": false, "current_text": Value::Null }),
            );
        };
        let Some(note_row) = index.get_note(&claim.note_id)? else {
            return Ok(
                json!({ "exists": true, "span_intact": false, "current_text": Value::Null }),
            );
        };
        let body = note::parse(&resolve_note_path(&self.vault_root, &note_row.path))?.body;
        let current_text = body
            .get(claim.span_start..claim.span_end)
            .map(ToString::to_string);
        let span_intact = current_text
            .as_deref()
            .map(|text| memora_core::Claim::compute_fingerprint(text) == claim.span_fingerprint)
            .unwrap_or(false);
        Ok(json!({
            "exists": true,
            "span_intact": span_intact,
            "current_text": current_text
        }))
    }

    pub async fn stale_claims(&self) -> Result<Value> {
        let index = Index::open(&self.index_db)?;
        let store = ClaimStore::new(&index);
        let provenance = Provenance::new(&index);
        let tracker = StalenessTracker::new(&index, &provenance);
        let rows = tracker.list_stale()?;
        let out = rows
            .into_iter()
            .map(|(claim_id, reason)| {
                let claim = store.get(&claim_id).ok().flatten();
                json!({
                    "claim_id": claim_id,
                    "reason": reason,
                    "note_id": claim.as_ref().map(|c| c.note_id.clone())
                })
            })
            .collect::<Vec<_>>();
        Ok(json!(out))
    }

    pub async fn contradictions(&self, input: ContradictionsInput) -> Result<Value> {
        let index = Index::open(&self.index_db)?;
        let store = ClaimStore::new(&index);
        let rows = store
            .contradictions_unack()?
            .into_iter()
            .filter(|(left, right)| {
                if let Some(subject) = &input.subject {
                    left.subject == *subject || right.subject == *subject
                } else {
                    true
                }
            })
            .map(|(left, right)| {
                json!({
                    "left_claim_id": left.id,
                    "right_claim_id": right.id,
                    "subject": left.subject,
                    "left_note_id": left.note_id,
                    "right_note_id": right.note_id
                })
            })
            .collect::<Vec<_>>();
        Ok(json!(rows))
    }

    pub async fn challenge(&self) -> Result<Value> {
        let index = Index::open(&self.index_db)?;
        let store = ClaimStore::new(&index);
        let llm = build_llm(LlmProvider::Ollama, None)?;
        let challenger = Challenger {
            db: &index,
            claim_store: &store,
            llm: llm.as_ref(),
            vault: &self.vault_root,
            config: memora_core::ChallengerConfig::default(),
        };
        Ok(serde_json::to_value(challenger.run_once().await?)?)
    }

    pub async fn decisions(&self) -> Result<Value> {
        let conn = rusqlite::Connection::open(&self.index_db)?;
        let mut stmt = conn.prepare(
            "SELECT id, title, decided_on, status FROM decisions ORDER BY decided_on DESC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "title": row.get::<_, String>(1)?,
                "decided_on": row.get::<_, String>(2)?,
                "status": row.get::<_, String>(3)?
            }))
        })?;
        let mut out: Vec<Value> = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(json!(out))
    }

    pub async fn invoke_tool(&self, tool: &str, args: Value) -> Result<Value> {
        match tool {
            "memora_query" => self.query(serde_json::from_value(args)?).await,
            "memora_query_cited" => self.query_cited(serde_json::from_value(args)?).await,
            "memora_get_note" => self.get_note(serde_json::from_value(args)?).await,
            "memora_get_atlas" => self.get_atlas(serde_json::from_value(args)?).await,
            "memora_get_world_map" => self.get_world_map().await,
            "memora_neighbors" => self.neighbors(serde_json::from_value(args)?).await,
            "memora_record_useful" => self.record_useful(serde_json::from_value(args)?).await,
            "memora_capture" => self.capture(serde_json::from_value(args)?).await,
            "memora_consolidate" => self.consolidate(serde_json::from_value(args)?).await,
            "memora_verify_claim" => self.verify_claim(serde_json::from_value(args)?).await,
            "memora_stale_claims" => self.stale_claims().await,
            "memora_contradictions" => self.contradictions(serde_json::from_value(args)?).await,
            "memora_challenge" => self.challenge().await,
            "memora_decisions" => self.decisions().await,
            other => Err(anyhow!("unknown tool: {other}")),
        }
    }
}

fn serialize_cited_answer(answer: memora_core::CitedAnswer) -> Value {
    let checks = answer
        .checks
        .into_iter()
        .map(|check| {
            json!({
                "claim_id": check.claim_id,
                "status": citation_status_to_str(check.status),
                "source_text": check.source_text,
                "quote": check.quote
            })
        })
        .collect::<Vec<_>>();
    json!({
        "raw_text": answer.raw_text,
        "clean_text": answer.clean_text,
        "checks": checks,
        "verified_count": answer.verified_count,
        "unverified_count": answer.unverified_count,
        "mismatch_count": answer.mismatch_count,
        "redacted_count": answer.redacted_count,
        "degraded": answer.degraded
    })
}

fn citation_status_to_str(status: CitationStatus) -> &'static str {
    match status {
        CitationStatus::Verified => "verified",
        CitationStatus::Unverified => "unverified",
        CitationStatus::FingerprintMismatch => "fingerprint_mismatch",
        CitationStatus::QuoteMismatch => "quote_mismatch",
    }
}

fn note_snippet(vault_root: &Path, indexed_path: &str) -> Result<String> {
    let parsed = note::parse(&resolve_note_path(vault_root, indexed_path))?;
    Ok(parsed
        .body
        .split_whitespace()
        .take(30)
        .collect::<Vec<_>>()
        .join(" "))
}

fn resolve_note_path(vault_root: &Path, indexed_path: &str) -> PathBuf {
    let raw = PathBuf::from(indexed_path);
    if raw.is_absolute() || raw.exists() {
        raw
    } else {
        vault_root.join(raw)
    }
}

#[derive(Debug, Clone)]
struct DeterministicEmbedder {
    dim: usize,
}

impl DeterministicEmbedder {
    fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[async_trait]
impl Embedder for DeterministicEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            let mut bytes = blake3::hash(text.as_bytes()).as_bytes().to_vec();
            while bytes.len() < self.dim * 4 {
                bytes.extend_from_slice(blake3::hash(&bytes).as_bytes());
            }
            let mut vector = Vec::with_capacity(self.dim);
            for chunk in bytes.chunks_exact(4).take(self.dim) {
                let bits = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                vector.push((bits as f32 / u32::MAX as f32) * 2.0 - 1.0);
            }
            out.push(vector);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        "memora-mcp/deterministic"
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct QueryInput {
    pub query: String,
    #[serde(default)]
    pub k: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct NoteInput {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct RegionInput {
    pub region: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct NeighborInput {
    pub id: String,
    #[serde(default)]
    pub top_n: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct RecordUsefulInput {
    pub query_id: String,
    pub useful_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CaptureInput {
    pub region: String,
    pub summary: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub privacy: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ConsolidateInput {
    pub scope: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct VerifyClaimInput {
    pub claim_id: String,
}

#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
pub struct ContradictionsInput {
    #[serde(default)]
    pub subject: Option<String>,
}

fn parse_privacy(raw: Option<&str>) -> Result<Privacy> {
    match raw.unwrap_or("private") {
        "public" => Ok(Privacy::Public),
        "private" => Ok(Privacy::Private),
        "secret" => Ok(Privacy::Secret),
        other => Err(anyhow!("invalid privacy value: {other}")),
    }
}

fn build_llm(provider: LlmProvider, model: Option<String>) -> Result<Box<dyn LlmClient>> {
    let allow_network = std::env::var("MEMORA_ENABLE_NETWORK_LLM")
        .map(|v| v == "1")
        .unwrap_or(false);
    if allow_network {
        match make_client(provider, model) {
            Ok(client) => Ok(client),
            Err(_) => Ok(Box::new(FallbackLlm)),
        }
    } else {
        Ok(Box::new(FallbackLlm))
    }
}

struct FallbackLlm;

#[async_trait]
impl LlmClient for FallbackLlm {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let content = req
            .messages
            .first()
            .map(|m| m.content.as_str())
            .unwrap_or_default();
        let response = if req.json_mode {
            if content.contains("proposal") || content.contains("action") {
                "{\"action\":\"archive\"}".to_string()
            } else {
                "{\"proposed_subregions\":[]}".to_string()
            }
        } else if content.contains("[claim:") {
            let marker = content
                .split("[claim:")
                .nth(1)
                .and_then(|tail| tail.split(']').next())
                .unwrap_or("unknown");
            format!("[claim:{marker}]")
        } else if content.contains("Summarize this contradiction") {
            "Potential contradiction detected.".to_string()
        } else if content.contains("clarifying question") {
            "What source confirms this claim?".to_string()
        } else if content.contains("Write 200-300 words") {
            "Fallback region index narrative.".to_string()
        } else {
            "Fallback Memora response.".to_string()
        };
        Ok(CompletionResponse {
            text: response,
            model: "fallback/local".to_string(),
            input_tokens: None,
            output_tokens: None,
        })
    }

    fn model_name(&self) -> &str {
        "fallback/local"
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::Local
    }
}
