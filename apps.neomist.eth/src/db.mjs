import initSqlJs from 'sql.js';
import { createRequire } from 'node:module';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
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
      const tempPath = `${PATHS.dbPath}.tmp-${process.pid}-${Date.now()}`;
      await writeFile(tempPath, bytes);
      await rename(tempPath, PATHS.dbPath);
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
            icon_url, manifest_url, fetch_error, body_bytes, success
          ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
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
            probe.manifestUrl,
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
    getStats(options = {}) {
      return buildStats(db, {
        category: options.category,
        headCursorBlockInclusive: this.getHeadCursorBlockInclusive(),
        backfillCursorBlockExclusive: this.getBackfillCursorBlockExclusive(),
      });
    },
    listNames(limit, options = {}) {
      const orderBy = options.sort === 'score'
        ? `current_analysis.quality_score IS NULL ASC,
           current_analysis.quality_score DESC,
           lower(names.name) ASC`
        : 'lower(names.name) ASC';
      return getAll(
        db,
        `SELECT names.node, names.name, names.parent_name, names.is_subdomain, names.contenthash_protocol,
                names.root_cid, names.source_block, names.source_tx_hash, names.last_seen_at,
                names.last_probe_status, names.last_probe_success,
                current_analysis.category AS analysis_category,
                current_analysis.quality_tier AS analysis_quality_tier,
                current_analysis.quality_score AS analysis_quality_score,
                current_analysis.security_risk AS analysis_security_risk,
                current_analysis.security_threat_type AS analysis_security_threat_type,
                current_analysis.safe_to_list AS analysis_safe_to_list
         FROM names
         LEFT JOIN analyses AS current_analysis
           ON current_analysis.root_cid = names.root_cid
          AND current_analysis.status = 'success'
          ORDER BY ${orderBy}
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
        latest_analysis: getOne(db, 'SELECT * FROM analyses WHERE root_cid = ? ORDER BY analyzed_at DESC, id DESC LIMIT 1', [row.root_cid]),
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
        `SELECT names.node, names.name, names.parent_name, names.is_subdomain, names.root_cid,
                latest_probe.title, latest_probe.icon_url, latest_probe.manifest_url,
                latest_analysis.root_cid AS analysis_root_cid,
                latest_analysis.model AS analysis_model,
                latest_analysis.analyzed_at AS analysis_analyzed_at,
                latest_analysis.analysis_json
         FROM names
         LEFT JOIN probes AS latest_probe
           ON latest_probe.id = (
             SELECT id
             FROM probes
             WHERE node = names.node
             ORDER BY probed_at DESC, id DESC
             LIMIT 1
           )
         LEFT JOIN analyses AS latest_analysis
           ON latest_analysis.id = (
             SELECT id
             FROM analyses
             WHERE root_cid = names.root_cid
               AND status = 'success'
             ORDER BY analyzed_at DESC, id DESC
             LIMIT 1
           )
         WHERE names.last_probe_success = 1
         ORDER BY lower(names.name) ASC`,
      );
    },
    listNamesMissingAnalysis(limit, options = {}) {
      const retryFailed = options.retryFailed ? 1 : 0;
      return getAll(
        db,
        `SELECT names.*, latest_probe.content_type AS probe_content_type,
                latest_probe.title AS probe_title,
                latest_probe.icon_url AS probe_icon_url,
                latest_probe.manifest_url AS probe_manifest_url,
                latest_probe.eth_link_url AS probe_content_url,
                latest_probe.x_ipfs_path AS probe_ipfs_path
         FROM names
         LEFT JOIN probes AS latest_probe
           ON latest_probe.id = (
             SELECT id
             FROM probes
             WHERE node = names.node
             ORDER BY probed_at DESC, id DESC
             LIMIT 1
           )
         WHERE names.last_probe_success = 1
           AND names.name NOT LIKE '[%].%'
           AND NOT EXISTS (
             SELECT 1
             FROM analyses
             WHERE root_cid = names.root_cid
               AND status = 'success'
           )
           AND (
             ? = 1
             OR NOT EXISTS (
               SELECT 1
               FROM analyses
               WHERE root_cid = names.root_cid
             )
             OR EXISTS (
               SELECT 1
               FROM analyses
               WHERE root_cid = names.root_cid
                 AND status = 'failed'
                 AND error LIKE 'wac opencode exited with status 1%'
             )
           )
         ORDER BY lower(names.name) ASC
         LIMIT ?`,
        [retryFailed, limit],
      );
    },
    upsertAnalysis(record) {
      run(
        db,
        `INSERT INTO analyses (
          node, name, root_cid, model, status, analyzed_at, duration_ms,
          category, category_confidence, quality_tier, quality_score,
          security_risk, security_risk_score, security_threat_type, safe_to_list, summary,
          analysis_json, error
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(root_cid) DO UPDATE SET
          node = excluded.node,
          name = excluded.name,
          model = excluded.model,
          status = excluded.status,
          analyzed_at = excluded.analyzed_at,
          duration_ms = excluded.duration_ms,
          category = excluded.category,
          category_confidence = excluded.category_confidence,
          quality_tier = excluded.quality_tier,
          quality_score = excluded.quality_score,
           security_risk = excluded.security_risk,
           security_risk_score = excluded.security_risk_score,
           security_threat_type = excluded.security_threat_type,
           safe_to_list = excluded.safe_to_list,
           summary = excluded.summary,
           analysis_json = excluded.analysis_json,
           error = excluded.error
         WHERE analyses.status != 'success'
            OR excluded.status = 'success'`,
        [
          record.node,
          record.name,
          record.root_cid,
          record.model,
          record.status,
          record.analyzed_at,
          record.duration_ms,
          record.category,
          record.category_confidence,
          record.quality_tier,
          record.quality_score,
          record.security_risk,
          record.security_risk_score,
          record.security_threat_type,
          record.safe_to_list,
          record.summary,
          record.analysis_json,
          record.error,
        ],
      );
      dirty = true;
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

function buildStats(db, options) {
  const category = normalizeOptionalText(options.category);
  const currentNames = getOne(db, 'SELECT COUNT(*) AS value FROM names').value;
  const successfulNames = getOne(db, 'SELECT COUNT(*) AS value FROM names WHERE last_probe_success = 1').value;
  const failedOrUnprobedNames = getOne(db, 'SELECT COUNT(*) AS value FROM names WHERE last_probe_success = 0').value;
  const successfulCurrentAnalyzedNames = getOne(
    db,
    `SELECT COUNT(*) AS value
     FROM names
     JOIN analyses ON analyses.root_cid = names.root_cid AND analyses.status = 'success'
     WHERE names.last_probe_success = 1`,
  ).value;
  const missingCurrentAnalysisNames = getOne(
    db,
    `SELECT COUNT(*) AS value
     FROM names
     WHERE last_probe_success = 1
       AND NOT EXISTS (
         SELECT 1
         FROM analyses
         WHERE analyses.root_cid = names.root_cid
           AND analyses.status = 'success'
       )`,
  ).value;
  const failedOrIncompleteAnalyses = getOne(db, "SELECT COUNT(*) AS value FROM analyses WHERE status != 'success'").value;
  const appScope = getAppScopeStats(db, category);

  return {
    overview: {
      names: currentNames,
      successful_names: successfulNames,
      failed_or_unprobed_names: failedOrUnprobedNames,
      name_versions: getOne(db, 'SELECT COUNT(*) AS value FROM name_versions').value,
      probes: getOne(db, 'SELECT COUNT(*) AS value FROM probes').value,
      analyses: getOne(db, 'SELECT COUNT(*) AS value FROM analyses').value,
      successful_analyses: getOne(db, "SELECT COUNT(*) AS value FROM analyses WHERE status = 'success'").value,
      head_cursor_block_inclusive: options.headCursorBlockInclusive,
      backfill_cursor_block_exclusive: options.backfillCursorBlockExclusive,
    },
    filter: category ? { category } : null,
    coverage: {
      current_names: currentNames,
      current_successful_names: successfulNames,
      current_successful_analyzed_names: successfulCurrentAnalyzedNames,
      missing_current_successful_analysis_names: missingCurrentAnalysisNames,
      failed_or_incomplete_analyses: failedOrIncompleteAnalyses,
      current_successful_analysis_coverage: ratio(successfulCurrentAnalyzedNames, successfulNames),
    },
    app_scope: appScope,
    apps_by_category: countScopedAppsBy(db, category, 'analyses.category', 'category'),
    quality_tiers: countScopedAppsBy(db, category, 'analyses.quality_tier', 'quality_tier'),
    security_risks: countScopedAppsBy(db, category, 'analyses.security_risk', 'security_risk'),
    threats: getThreatStats(db, category),
    threat_types: countScopedThreatsByType(db, category),
    safe_to_list: getSafeToListStats(db, category),
    probe_health: {
      current_successful_names: successfulNames,
      failed_or_unprobed_names: failedOrUnprobedNames,
      current_probe_success_rate: ratio(successfulNames, currentNames),
      failures_by_http_status: getAll(
        db,
        `SELECT CASE WHEN last_probe_status IS NULL THEN 'none' ELSE CAST(last_probe_status AS TEXT) END AS http_status,
                COUNT(*) AS names
         FROM names
         WHERE last_probe_success = 0
         GROUP BY http_status
         ORDER BY names DESC, http_status ASC`,
      ),
    },
    contenthash_protocols: countScopedAppsBy(db, category, 'names.contenthash_protocol', 'contenthash_protocol'),
    domain_shape: getDomainShapeStats(db, category),
    unique_root_cids: getUniqueRootCidStats(db, category),
  };
}

function getAppScopeStats(db, category) {
  const { fromWhere, params } = scopedCurrentAppsQuery(category);
  const row = getOne(db, `SELECT COUNT(*) AS apps, COUNT(DISTINCT names.root_cid) AS unique_root_cids ${fromWhere}`, params);
  return {
    category,
    current_successful_analyzed_apps: row.apps,
    unique_root_cids: row.unique_root_cids,
  };
}

function countScopedAppsBy(db, category, columnSql, key) {
  const { fromWhere, params } = scopedCurrentAppsQuery(category);
  return getAll(
    db,
    `SELECT ${columnSql} AS ${key}, COUNT(*) AS apps
     ${fromWhere}
     GROUP BY ${columnSql}
     ORDER BY apps DESC, lower(${columnSql}) ASC`,
    params,
  );
}

function getThreatStats(db, category) {
  const { fromWhere, params } = scopedCurrentAppsQuery(category);
  return {
    threatened_apps: getOne(db, `SELECT COUNT(*) AS value ${fromWhere} AND analyses.security_threat_type != 'none'`, params).value,
    high_or_critical_risk_apps: getOne(db, `SELECT COUNT(*) AS value ${fromWhere} AND analyses.security_risk IN ('high', 'critical')`, params).value,
  };
}

function countScopedThreatsByType(db, category) {
  const { fromWhere, params } = scopedCurrentAppsQuery(category);
  return getAll(
    db,
    `SELECT analyses.security_threat_type AS threat_type, COUNT(*) AS apps
     ${fromWhere}
       AND analyses.security_threat_type != 'none'
     GROUP BY analyses.security_threat_type
     ORDER BY apps DESC, threat_type ASC`,
    params,
  );
}

function getSafeToListStats(db, category) {
  const { fromWhere, params } = scopedCurrentAppsQuery(category);
  return getOne(
    db,
    `SELECT
       COALESCE(SUM(CASE WHEN analyses.safe_to_list = 1 THEN 1 ELSE 0 END), 0) AS safe,
       COALESCE(SUM(CASE WHEN analyses.safe_to_list = 0 THEN 1 ELSE 0 END), 0) AS unsafe,
       COALESCE(SUM(CASE WHEN analyses.safe_to_list IS NULL THEN 1 ELSE 0 END), 0) AS unknown
     ${fromWhere}`,
    params,
  );
}

function getDomainShapeStats(db, category) {
  const { fromWhere, params } = scopedCurrentAppsQuery(category);
  return getAll(
    db,
    `SELECT CASE WHEN names.is_subdomain = 1 THEN 'subdomain' ELSE 'root_name' END AS shape,
            COUNT(*) AS apps
     ${fromWhere}
     GROUP BY shape
     ORDER BY apps DESC, shape ASC`,
    params,
  );
}

function getUniqueRootCidStats(db, category) {
  const { fromWhere, params } = scopedCurrentAppsQuery(category);
  const duplicateRows = getOne(
    db,
    `SELECT COUNT(*) AS duplicate_root_cid_groups,
            COALESCE(SUM(apps), 0) AS apps_sharing_duplicate_root_cids
     FROM (
       SELECT names.root_cid, COUNT(*) AS apps
       ${fromWhere}
       GROUP BY names.root_cid
       HAVING COUNT(*) > 1
     )`,
    params,
  );
  return {
    current_successful_analyzed_root_cids: getOne(db, `SELECT COUNT(DISTINCT names.root_cid) AS value ${fromWhere}`, params).value,
    duplicate_root_cid_groups: duplicateRows.duplicate_root_cid_groups,
    apps_sharing_duplicate_root_cids: duplicateRows.apps_sharing_duplicate_root_cids,
  };
}

function scopedCurrentAppsQuery(category) {
  const params = category ? [category] : [];
  return {
    fromWhere: `FROM names
      JOIN analyses ON analyses.root_cid = names.root_cid AND analyses.status = 'success'
      WHERE names.last_probe_success = 1${category ? ' AND lower(analyses.category) = lower(?)' : ''}`,
    params,
  };
}

function ratio(numerator, denominator) {
  return denominator === 0 ? 0 : numerator / denominator;
}

function normalizeOptionalText(value) {
  if (value == null) {
    return null;
  }
  const text = String(value).trim();
  return text || null;
}

function ensureSchemaMigrations(db) {
  ensureColumn(db, 'names', 'source_tx_hash', 'TEXT');
  ensureColumn(db, 'names', 'source_event_id', 'TEXT');
  ensureColumn(db, 'probes', 'manifest_url', 'TEXT');
  ensureColumn(db, 'analyses', 'security_threat_type', 'TEXT');
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
