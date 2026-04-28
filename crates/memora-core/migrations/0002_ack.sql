CREATE TABLE IF NOT EXISTS acknowledged_contradictions (
  a_id TEXT NOT NULL,
  b_id TEXT NOT NULL,
  ack_at TEXT NOT NULL,
  ack_by TEXT,
  PRIMARY KEY (a_id, b_id)
);
