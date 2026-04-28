-- L0: notes
CREATE TABLE IF NOT EXISTS notes (
  id TEXT PRIMARY KEY,
  path TEXT UNIQUE NOT NULL,
  region TEXT NOT NULL,
  source TEXT NOT NULL,
  privacy TEXT NOT NULL DEFAULT 'private',
  body_hash TEXT NOT NULL,
  body_size INTEGER NOT NULL,
  summary TEXT NOT NULL,
  tags_json TEXT NOT NULL,
  created TEXT NOT NULL,
  updated TEXT NOT NULL,
  qvalue REAL NOT NULL DEFAULT 0.0
);
CREATE INDEX IF NOT EXISTS idx_notes_region ON notes(region);
CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated);

CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
  id UNINDEXED,
  summary,
  body,
  tags,
  tokenize='porter unicode61'
);

CREATE TABLE IF NOT EXISTS wikilinks (
  src_id TEXT NOT NULL,
  dst_target TEXT NOT NULL,
  PRIMARY KEY (src_id, dst_target),
  FOREIGN KEY (src_id) REFERENCES notes(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS hebbian_edges (
  a_id TEXT NOT NULL,
  b_id TEXT NOT NULL,
  weight REAL NOT NULL DEFAULT 0.0,
  last_coactivated TEXT NOT NULL,
  PRIMARY KEY (a_id, b_id),
  CHECK (a_id < b_id)
);
CREATE INDEX IF NOT EXISTS idx_hebbian_a ON hebbian_edges(a_id);
CREATE INDEX IF NOT EXISTS idx_hebbian_b ON hebbian_edges(b_id);

-- L1: claims
CREATE TABLE IF NOT EXISTS claims (
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  predicate TEXT NOT NULL,
  object TEXT NOT NULL,
  note_id TEXT NOT NULL,
  span_start INTEGER NOT NULL,
  span_end INTEGER NOT NULL,
  span_fingerprint TEXT NOT NULL,
  valid_from TEXT NOT NULL,
  valid_until TEXT,
  confidence REAL NOT NULL DEFAULT 0.7,
  privacy TEXT NOT NULL,
  extracted_by TEXT NOT NULL,
  extracted_at TEXT NOT NULL,
  FOREIGN KEY (note_id) REFERENCES notes(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_claims_note ON claims(note_id);
CREATE INDEX IF NOT EXISTS idx_claims_subject ON claims(subject);
CREATE INDEX IF NOT EXISTS idx_claims_predicate ON claims(predicate);
CREATE INDEX IF NOT EXISTS idx_claims_validity ON claims(valid_from, valid_until);

CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5(
  id UNINDEXED, subject, predicate, object,
  tokenize='porter unicode61'
);

CREATE TABLE IF NOT EXISTS claim_relations (
  src_id TEXT NOT NULL,
  dst_id TEXT NOT NULL,
  relation TEXT NOT NULL,
  weight REAL NOT NULL DEFAULT 1.0,
  created TEXT NOT NULL,
  PRIMARY KEY (src_id, dst_id, relation)
);
CREATE INDEX IF NOT EXISTS idx_claim_rel_src ON claim_relations(src_id);
CREATE INDEX IF NOT EXISTS idx_claim_rel_dst ON claim_relations(dst_id);

CREATE TABLE IF NOT EXISTS provenance (
  derived_claim_id TEXT NOT NULL,
  source_claim_id TEXT NOT NULL,
  PRIMARY KEY (derived_claim_id, source_claim_id)
);
CREATE INDEX IF NOT EXISTS idx_provenance_source ON provenance(source_claim_id);

CREATE TABLE IF NOT EXISTS stale_claims (
  claim_id TEXT PRIMARY KEY,
  reason TEXT NOT NULL,
  marked_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS decisions (
  id TEXT PRIMARY KEY,
  claim_id TEXT NOT NULL,
  title TEXT NOT NULL,
  decided_on TEXT NOT NULL,
  decided_by TEXT,
  status TEXT NOT NULL,
  FOREIGN KEY (claim_id) REFERENCES claims(id)
);

CREATE TABLE IF NOT EXISTS retrievals (
  query_id TEXT PRIMARY KEY,
  query_text TEXT NOT NULL,
  claim_ids_json TEXT NOT NULL,
  ts TEXT NOT NULL,
  marked_useful_json TEXT
);
