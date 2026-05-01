-- Allow NULL object for unary predicates. Rebuild claims + FTS; preserve row ids
-- (FKs in decisions, etc. remain valid when foreign_keys are re-enabled).
-- Runs inside the migrator's outer transaction (no nested BEGIN/COMMIT).

PRAGMA foreign_keys=OFF;

CREATE TABLE claims_new (
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  predicate TEXT NOT NULL,
  object TEXT,
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

INSERT INTO claims_new (
  id, subject, predicate, object, note_id, span_start, span_end, span_fingerprint,
  valid_from, valid_until, confidence, privacy, extracted_by, extracted_at
)
SELECT
  id, subject, predicate, object, note_id, span_start, span_end, span_fingerprint,
  valid_from, valid_until, confidence, privacy, extracted_by, extracted_at
FROM claims;

DROP TABLE claims;
ALTER TABLE claims_new RENAME TO claims;

CREATE INDEX IF NOT EXISTS idx_claims_note ON claims(note_id);
CREATE INDEX IF NOT EXISTS idx_claims_subject ON claims(subject);
CREATE INDEX IF NOT EXISTS idx_claims_predicate ON claims(predicate);
CREATE INDEX IF NOT EXISTS idx_claims_validity ON claims(valid_from, valid_until);

DELETE FROM claims_fts;
INSERT INTO claims_fts (id, subject, predicate, object)
SELECT id, subject, predicate, IFNULL(object, '') FROM claims;

PRAGMA foreign_keys=ON;
