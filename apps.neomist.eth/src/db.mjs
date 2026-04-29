import initSqlJs from 'sql.js';
import { createRequire } from 'node:module';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { PATHS, SYNC_STATE_KEYS } from './config.mjs';

const require = createRequire(import.meta.url);
let sqlJsPromise = null;

export async function openStore() {
  await mkdir(PATHS.stateDir, { recursive: true });
  const schemaSql = await readFile(PATHS.schemaPath, 'utf8');
  const SQL = await loadSqlJs();

  let db;
  try {
    const existingBytes = await readFile(PATHS.dbPath);
    db = new SQL.Database(existingBytes);
  } catch (error) {
    if (error?.code !== 'ENOENT') {
      throw error;
    }
    db = new SQL.Database();
  }

  db.exec(schemaSql);
  ensureSchemaMigrations(db);
  return createStore(db);
}

async function loadSqlJs() {
  if (!sqlJsPromise) {
    const wasmPath = require.resolve('sql.js/dist/sql-wasm.wasm');
    sqlJsPromise = initSqlJs({
      locateFile(file) {
        return path.join(path.dirname(wasmPath), file);
      },
    });
  }
  return sqlJsPromise;
}

function createStore(db) {
  let dirty = false;

  return {
    async close() {
      await this.flush();
      db.close();
    },
    async flush() {
      if (!dirty) {
        return;
      }
      const bytes = db.export();
      await writeFile(PATHS.dbPath, bytes);
      dirty = false;
    },
    getHeadCursorBlockInclusive() {
      const row = getOne(db, 'SELECT value FROM sync_state WHERE key = ?', [SYNC_STATE_KEYS.headCursorBlockInclusive]);
      return row ? Number(row.value) : null;
    },
    setHeadCursorBlockInclusive(blockNumber) {
      run(db, 'INSERT INTO sync_state(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value', [SYNC_STATE_KEYS.headCursorBlockInclusive, String(blockNumber)]);
      dirty = true;
    },
    getBackfillCursorBlockExclusive() {
      const row = getOne(db, 'SELECT value FROM sync_state WHERE key = ?', [SYNC_STATE_KEYS.backfillCursorBlockExclusive])
        ?? getOne(db, 'SELECT value FROM sync_state WHERE key = ?', [SYNC_STATE_KEYS.legacyCursorBlockExclusive]);
      return row ? Number(row.value) : null;
    },
    setBackfillCursorBlockExclusive(blockNumber) {
      run(db, 'INSERT INTO sync_state(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value', [SYNC_STATE_KEYS.backfillCursorBlockExclusive, String(blockNumber)]);
      dirty = true;
    },
    getExistingNodes(nodes) {
      if (nodes.length === 0) {
        return new Set();
      }
      const placeholders = nodes.map(() => '?').join(', ');
      const rows = getAll(db, `SELECT node FROM names WHERE node IN (${placeholders})`, nodes);
      return new Set(rows.map((row) => row.node));
    },
    getNameRowsByNodes(nodes) {
      if (nodes.length === 0) {
        return new Map();
      }
      const placeholders = nodes.map(() => '?').join(', ');
      const rows = getAll(db, `SELECT * FROM names WHERE node IN (${placeholders})`, nodes);
      return new Map(rows.map((row) => [row.node, row]));
    },
    upsertName(record) {
      run(
        db,
        `INSERT INTO names (
          node, name, parent_name, is_subdomain, contenthash_hex, contenthash_protocol,
          root_cid, source_block, source_tx_hash, source_event_id, first_seen_at, last_seen_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(node) DO UPDATE SET
          name = excluded.name,
          parent_name = excluded.parent_name,
          is_subdomain = excluded.is_subdomain,
          contenthash_hex = excluded.contenthash_hex,
          contenthash_protocol = excluded.contenthash_protocol,
          root_cid = excluded.root_cid,
          source_block = excluded.source_block,
          source_tx_hash = excluded.source_tx_hash,
          source_event_id = excluded.source_event_id,
          last_seen_at = excluded.last_seen_at`,
        [
          record.node,
          record.name,
          record.parent_name,
          record.is_subdomain,
          record.contenthash_hex,
          record.contenthash_protocol,
          record.root_cid,
          record.source_block,
          record.source_tx_hash,
          record.source_event_id,
          record.seen_at,
          record.seen_at,
        ],
      );
      dirty = true;
    },
    insertNameVersion(record) {
      run(
        db,
        `INSERT OR IGNORE INTO name_versions (
          node, name, parent_name, is_subdomain, contenthash_hex, contenthash_protocol,
          root_cid, source_block, source_tx_hash, source_event_id, seen_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        [
          record.node,
          record.name,
          record.parent_name,
          record.is_subdomain,
          record.contenthash_hex,
          record.contenthash_protocol,
          record.root_cid,
          record.source_block,
          record.source_tx_hash,
          record.source_event_id,
          record.seen_at,
        ],
      );
      dirty = true;
    },
    insertProbe(nodeRecord, probe) {
      const success = probe.success ? 1 : 0;
      db.exec('BEGIN');
      try {
        run(
          db,
          `INSERT INTO probes (
            node, name, root_cid, eth_link_url, probed_at, http_status, content_type,
            content_length, location_header, x_ipfs_path, x_ipfs_roots_json, title,
            icon_url, fetch_error, body_bytes, success
          ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          [
            nodeRecord.node,
            nodeRecord.name,
            nodeRecord.root_cid,
            probe.ethLinkUrl,
            probe.probedAt,
            probe.httpStatus,
            probe.contentType,
            probe.contentLength,
            probe.locationHeader,
            probe.xIpfsPath,
            JSON.stringify(probe.xIpfsRoots ?? []),
            probe.title,
            probe.iconUrl,
            probe.fetchError,
            probe.bodyBytes,
            success,
          ],
        );
        run(
          db,
          `UPDATE names
           SET
             last_probe_ok_at = CASE WHEN ? = 1 THEN ? ELSE last_probe_ok_at END,
             last_probe_status = ?,
             last_probe_success = ?
           WHERE node = ?`,
          [success, probe.probedAt, probe.httpStatus, success, nodeRecord.node],
        );
        db.exec('COMMIT');
      } catch (error) {
        db.exec('ROLLBACK');
        throw error;
      }
      dirty = true;
    },
    getStats() {
      return {
        names: getOne(db, 'SELECT COUNT(*) AS value FROM names').value,
        successful_names: getOne(db, 'SELECT COUNT(*) AS value FROM names WHERE last_probe_success = 1').value,
        failed_or_unprobed_names: getOne(db, 'SELECT COUNT(*) AS value FROM names WHERE last_probe_success = 0').value,
        name_versions: getOne(db, 'SELECT COUNT(*) AS value FROM name_versions').value,
        probes: getOne(db, 'SELECT COUNT(*) AS value FROM probes').value,
        head_cursor_block_inclusive: this.getHeadCursorBlockInclusive(),
        backfill_cursor_block_exclusive: this.getBackfillCursorBlockExclusive(),
      };
    },
    listNames(limit) {
      return getAll(
        db,
        `SELECT node, name, parent_name, is_subdomain, contenthash_protocol, root_cid, source_block,
                source_tx_hash, last_seen_at, last_probe_status, last_probe_success
         FROM names
         ORDER BY lower(name) ASC
         LIMIT ?`,
        [limit],
      );
    },
    showName(identifier) {
      const row = identifier.startsWith('0x')
        ? getOne(db, 'SELECT * FROM names WHERE node = ?', [identifier])
        : getOne(db, 'SELECT * FROM names WHERE lower(name) = lower(?)', [identifier]);
      if (!row) {
        return null;
      }
      return {
        name: row,
        latest_probe: getOne(db, 'SELECT * FROM probes WHERE node = ? ORDER BY probed_at DESC, id DESC LIMIT 1', [row.node]),
        versions: getAll(
          db,
          `SELECT node, name, parent_name, is_subdomain, contenthash_hex, contenthash_protocol,
                  root_cid, source_block, source_tx_hash, source_event_id, seen_at
           FROM name_versions
           WHERE node = ?
           ORDER BY source_block DESC, id DESC`,
          [row.node],
        ),
      };
    },
    listProbeFailures(limit) {
      return getAll(
        db,
        `SELECT node, name, root_cid, http_status, fetch_error, probed_at, eth_link_url
         FROM probes
         WHERE success = 0
         ORDER BY probed_at DESC, id DESC
         LIMIT ?`,
        [limit],
      );
    },
    listExportableNames() {
      return getAll(
        db,
        `SELECT node, name, parent_name, is_subdomain
         FROM names
         WHERE last_probe_success = 1
         ORDER BY lower(name) ASC`,
      );
    },
    listContenthashVersionsForNodes(nodes) {
      if (nodes.length === 0) {
        return [];
      }
      const placeholders = nodes.map(() => '?').join(', ');
      return getAll(
        db,
        `SELECT node, contenthash_protocol, root_cid, source_block, source_tx_hash
         FROM name_versions
         WHERE node IN (${placeholders})
         ORDER BY source_block DESC, id DESC`,
        nodes,
      );
    },
  };
}

function ensureSchemaMigrations(db) {
  ensureColumn(db, 'names', 'source_tx_hash', 'TEXT');
  ensureColumn(db, 'names', 'source_event_id', 'TEXT');
}

function ensureColumn(db, tableName, columnName, columnType) {
  const rows = getAll(db, `PRAGMA table_info(${tableName})`);
  if (rows.some((row) => row.name === columnName)) {
    return;
  }
  db.run(`ALTER TABLE ${tableName} ADD COLUMN ${columnName} ${columnType}`);
}

function run(db, sql, params = []) {
  db.run(sql, params);
}

function getOne(db, sql, params = []) {
  const statement = db.prepare(sql);
  try {
    statement.bind(params);
    if (!statement.step()) {
      return null;
    }
    return normalizeRow(statement.getAsObject());
  } finally {
    statement.free();
  }
}

function getAll(db, sql, params = []) {
  const statement = db.prepare(sql);
  try {
    statement.bind(params);
    const rows = [];
    while (statement.step()) {
      rows.push(normalizeRow(statement.getAsObject()));
    }
    return rows;
  } finally {
    statement.free();
  }
}

function normalizeRow(row) {
  const normalized = {};
  for (const [key, value] of Object.entries(row)) {
    normalized[key] = value;
  }
  return normalized;
}
