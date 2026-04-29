CREATE TABLE IF NOT EXISTS sync_state (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS names (
  node TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  parent_name TEXT,
  is_subdomain INTEGER NOT NULL,
  contenthash_hex TEXT NOT NULL,
  contenthash_protocol TEXT NOT NULL,
  root_cid TEXT NOT NULL,
  source_block INTEGER,
  source_tx_hash TEXT,
  source_event_id TEXT,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  last_probe_ok_at TEXT,
  last_probe_status INTEGER,
  last_probe_success INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_names_name ON names(name);
CREATE INDEX IF NOT EXISTS idx_names_root_cid ON names(root_cid);
CREATE INDEX IF NOT EXISTS idx_names_probe_success ON names(last_probe_success);

CREATE TABLE IF NOT EXISTS name_versions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  node TEXT NOT NULL,
  name TEXT NOT NULL,
  parent_name TEXT,
  is_subdomain INTEGER NOT NULL,
  contenthash_hex TEXT NOT NULL,
  contenthash_protocol TEXT NOT NULL,
  root_cid TEXT NOT NULL,
  source_block INTEGER,
  source_tx_hash TEXT,
  source_event_id TEXT NOT NULL,
  seen_at TEXT NOT NULL,
  UNIQUE(node, source_event_id),
  FOREIGN KEY(node) REFERENCES names(node)
);

CREATE INDEX IF NOT EXISTS idx_name_versions_node_block ON name_versions(node, source_block DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_name_versions_root_cid ON name_versions(root_cid);

CREATE TABLE IF NOT EXISTS probes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  node TEXT NOT NULL,
  name TEXT NOT NULL,
  root_cid TEXT NOT NULL,
  eth_link_url TEXT,
  probed_at TEXT NOT NULL,
  http_status INTEGER,
  content_type TEXT,
  content_length INTEGER,
  location_header TEXT,
  x_ipfs_path TEXT,
  x_ipfs_roots_json TEXT,
  title TEXT,
  icon_url TEXT,
  fetch_error TEXT,
  body_bytes INTEGER NOT NULL DEFAULT 0,
  success INTEGER NOT NULL,
  FOREIGN KEY(node) REFERENCES names(node)
);

CREATE INDEX IF NOT EXISTS idx_probes_node ON probes(node, probed_at DESC);
CREATE INDEX IF NOT EXISTS idx_probes_success ON probes(success, probed_at DESC);
