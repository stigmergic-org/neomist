import { spawn } from 'node:child_process';
import { copyFile, mkdir, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { DEFAULTS, PATHS } from './config.mjs';

const CATEGORY_LABELS = new Set([
  'Finance',
  'Collectibles',
  'Gaming',
  'Social',
  'Governance',
  'Identity',
  'Developer tools',
  'Infrastructure',
  'Analytics',
  'Education',
  'Media',
  'Commerce',
  'Public goods',
  'Security',
  'Wallet',
  'Bridge',
  'Community',
  'Static data',
  'Personal',
  'Blog',
  'Documentation',
  'Redirect',
  'Placeholder',
  'Unavailable',
  'Unknown',
]);

const QUALITY_TIERS = new Set(['excellent', 'good', 'fair', 'low', 'broken', 'unknown']);
const SECURITY_RISKS = new Set(['low', 'medium', 'high', 'critical', 'unknown']);
const SECURITY_THREAT_TYPES = new Set([
  'none',
  'seed_phrase_prompt',
  'private_key_prompt',
  'wallet_drainer',
  'approval_abuse',
  'malicious_redirect',
  'brand_impersonation',
  'obfuscated_code',
  'suspicious_external_script',
  'malware_download',
  'phishing_language',
  'suspicious_signing',
  'other',
]);

export async function analyzeName(store, identifier, options = {}) {
  const shown = store.showName(identifier);
  if (!shown) {
    return {
      identifier,
      found: false,
      analyzed: false,
      status: 'not_found',
    };
  }

  return analyzeTarget(store, buildTargetFromShownName(shown, options), options);
}

export async function analyzeMissingNames(store, options = {}) {
  const limit = options.limit ?? 10;
  const rows = store.listNamesMissingAnalysis(limit);
  const results = [];

  for (const row of rows) {
    const result = await analyzeTarget(store, buildTargetFromListRow(row), options);
    results.push(result);
    await store.flush?.();
  }

  return {
    requestedLimit: limit,
    analyzed: results.length,
    results,
  };
}

async function analyzeTarget(store, target, options) {
  const model = options.model ?? DEFAULTS.analysisModel;
  const timeoutMs = options.timeoutMs ?? DEFAULTS.analysisTimeoutMs;
  const startedAt = Date.now();
  const analyzedAt = new Date().toISOString();

  if (!options.force && target.latestAnalysis?.status === 'success') {
    return {
      identifier: target.identifier,
      found: true,
      analyzed: false,
      status: 'already_analyzed',
      name: target.name,
      node: target.node,
      root_cid: target.rootCid,
    };
  }

  let status = 'failed';
  let analysis = null;
  let error = null;
  let stdout = '';
  let stderr = '';

  try {
    const workDir = await prepareWorkDir(target);
    const run = await runWacAnalysis({ workDir, model, timeoutMs });
    stdout = run.stdout;
    stderr = run.stderr;

    if (run.timedOut) {
      status = 'timeout';
      error = `analysis timed out after ${timeoutMs}ms`;
    } else if (run.exitCode !== 0) {
      status = 'failed';
      error = `wac opencode exited with status ${run.exitCode}`;
    } else {
      try {
        analysis = await readAnalysisJson(workDir);
        validateAnalysis(analysis);
        status = 'success';
      } catch (caught) {
        status = 'invalid_json';
        const validationError = caught instanceof Error ? caught.message : String(caught);
        const output = summarizeProcessOutput(stdout, stderr);
        error = output ? `${validationError}\n${output}` : validationError;
      }
    }
  } catch (caught) {
    status = status === 'timeout' ? status : 'failed';
    error = caught instanceof Error ? caught.message : String(caught);
  }

  const durationMs = Date.now() - startedAt;
  const record = buildAnalysisRecord({
    target,
    model,
    status,
    analyzedAt,
    durationMs,
    analysis,
    error: status === 'success' ? null : (error ?? summarizeProcessOutput(stdout, stderr)),
  });
  store.upsertAnalysis(record);

  return {
    identifier: target.identifier,
    found: true,
    analyzed: true,
    status,
    name: target.name,
    node: target.node,
    root_cid: target.rootCid,
    model,
    duration_ms: durationMs,
    category: analysis?.category ?? null,
    quality_tier: analysis?.quality?.tier ?? null,
    security_risk: analysis?.security?.risk ?? null,
    security_threat_type: analysis?.security?.threat_type ?? null,
    error: record.error,
  };
}

function buildTargetFromShownName(shown, options) {
  const name = shown.name;
  const rootCid = options.cid ?? name.root_cid;
  const latestProbe = shown.latest_probe?.root_cid === rootCid ? shown.latest_probe : null;
  const latestAnalysis = shown.latest_analysis?.root_cid === rootCid ? shown.latest_analysis : null;

  return {
    identifier: options.identifier ?? name.name,
    node: name.node,
    name: name.name,
    parentName: name.parent_name,
    isSubdomain: Boolean(name.is_subdomain),
    protocol: name.contenthash_protocol,
    rootCid,
    sourceBlock: name.source_block,
    sourceTxHash: name.source_tx_hash,
    latestProbe,
    latestAnalysis,
  };
}

function buildTargetFromListRow(row) {
  return {
    identifier: row.name,
    node: row.node,
    name: row.name,
    parentName: row.parent_name,
    isSubdomain: Boolean(row.is_subdomain),
    protocol: row.contenthash_protocol,
    rootCid: row.root_cid,
    sourceBlock: row.source_block,
    sourceTxHash: row.source_tx_hash,
    latestProbe: {
      content_type: row.probe_content_type,
      title: row.probe_title,
      icon_url: row.probe_icon_url,
      manifest_url: row.probe_manifest_url,
      eth_link_url: row.probe_content_url,
      x_ipfs_path: row.probe_ipfs_path,
    },
    latestAnalysis: null,
  };
}

async function prepareWorkDir(target) {
  const workDir = path.join(PATHS.analysisWorkDir, safePathSegment(target.rootCid));
  const rootPath = buildMountedRootPath(target);
  const metadataPath = path.join(workDir, 'analysis-context.json');
  const promptCopyPath = path.join(workDir, 'ipfs-app-analysis-system.md');
  const analysisPath = path.join(workDir, 'analysis.json');
  const rootLinkPath = path.join(workDir, 'root');

  await rm(workDir, { recursive: true, force: true });
  await mkdir(workDir, { recursive: true });
  await symlink(rootPath, rootLinkPath, 'dir');
  await writeFile(metadataPath, `${JSON.stringify(buildMetadata(target, rootPath), null, 2)}\n`);
  await copyFile(PATHS.analysisPromptPath, promptCopyPath);
  await rm(analysisPath, { force: true });

  return workDir;
}

function buildMountedRootPath(target) {
  const protocol = target.protocol === 'ipns' ? 'ipns' : 'ipfs';
  return `/${protocol}/${target.rootCid}`;
}

function buildMetadata(target, mountedRootPath) {
  return {
    analysis_target: {
      mounted_root_path: mountedRootPath,
      root_dir: 'root',
      contenthash_protocol: target.protocol,
      root_cid: target.rootCid,
    },
    name: {
      node: target.node,
      name: target.name,
      parent_name: target.parentName,
      is_subdomain: target.isSubdomain,
      source_block: target.sourceBlock,
      source_tx_hash: target.sourceTxHash,
    },
    latest_probe: target.latestProbe ? {
      content_type: target.latestProbe.content_type,
      content_length: target.latestProbe.content_length,
      title: target.latestProbe.title,
      icon_url: target.latestProbe.icon_url,
      manifest_url: target.latestProbe.manifest_url,
      content_url: target.latestProbe.eth_link_url,
      ipfs_path: target.latestProbe.x_ipfs_path,
    } : null,
  };
}

async function runWacAnalysis({ workDir, model, timeoutMs }) {
  const timeoutSeconds = Math.max(1, Math.ceil(timeoutMs / 1000));
  const args = [
    '--allow-root',
    '--mount-ipfs',
    '-n',
    'timeout',
    '--kill-after=10s',
    `${timeoutSeconds}s`,
    'opencode',
    'run',
    '--dangerously-skip-permissions',
    '-m',
    model,
    'Read ipfs-app-analysis-system.md, follow it exactly, analyze analysis-context.json and root, and write strict JSON to analysis.json.',
  ];

  return spawnProcess(PATHS.wacPath, args, { cwd: workDir });
}

function spawnProcess(command, args, options) {
  return new Promise((resolve) => {
    const child = spawn(command, args, {
      ...options,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';

    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString();
    });
    child.on('error', (error) => {
      resolve({ exitCode: 1, timedOut: false, stdout, stderr: `${stderr}${error.message}` });
    });
    child.on('close', (exitCode) => {
      resolve({ exitCode, timedOut: exitCode === 124, stdout, stderr });
    });
  });
}

async function readAnalysisJson(workDir) {
  try {
    return JSON.parse(await readFile(path.join(workDir, 'analysis.json'), 'utf8'));
  } catch (error) {
    throw new Error(`analysis.json missing or invalid: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function validateAnalysis(analysis) {
  if (!analysis || typeof analysis !== 'object' || Array.isArray(analysis)) {
    throw new Error('analysis.json must contain a JSON object');
  }
  if (analysis.schema_version !== 1) {
    throw new Error('analysis.json schema_version must be 1');
  }
  if (!CATEGORY_LABELS.has(analysis.category)) {
    throw new Error(`analysis.json category is invalid: ${analysis.category}`);
  }
  assertUnitNumber(analysis.category_confidence, 'category_confidence');
  if (typeof analysis.summary !== 'string') {
    throw new Error('analysis.json summary must be a string');
  }
  if (!Array.isArray(analysis.signals)) {
    throw new Error('analysis.json signals must be an array');
  }
  validateQuality(analysis.quality);
  validateSecurity(analysis.security);
  if (!Array.isArray(analysis.files_reviewed)) {
    throw new Error('analysis.json files_reviewed must be an array');
  }
}

function validateQuality(quality) {
  if (!quality || typeof quality !== 'object' || Array.isArray(quality)) {
    throw new Error('analysis.json quality must be an object');
  }
  if (!QUALITY_TIERS.has(quality.tier)) {
    throw new Error(`analysis.json quality.tier is invalid: ${quality.tier}`);
  }
  assertUnitNumber(quality.score, 'quality.score');
  for (const key of ['is_substantive', 'is_redirect_only', 'is_placeholder']) {
    if (typeof quality[key] !== 'boolean') {
      throw new Error(`analysis.json quality.${key} must be a boolean`);
    }
  }
  if (typeof quality.rationale !== 'string') {
    throw new Error('analysis.json quality.rationale must be a string');
  }
}

function validateSecurity(security) {
  if (!security || typeof security !== 'object' || Array.isArray(security)) {
    throw new Error('analysis.json security must be an object');
  }
  if (!SECURITY_RISKS.has(security.risk)) {
    throw new Error(`analysis.json security.risk is invalid: ${security.risk}`);
  }
  assertUnitNumber(security.risk_score, 'security.risk_score');
  if (!SECURITY_THREAT_TYPES.has(security.threat_type)) {
    throw new Error(`analysis.json security.threat_type is invalid: ${security.threat_type}`);
  }
  if (typeof security.safe_to_list !== 'boolean') {
    throw new Error('analysis.json security.safe_to_list must be a boolean');
  }
  if (!Array.isArray(security.findings)) {
    throw new Error('analysis.json security.findings must be an array');
  }
  if (security.findings.length === 0 && security.threat_type !== 'none') {
    throw new Error('analysis.json security.threat_type must be "none" when findings is empty');
  }
  if (security.findings.length > 0 && security.threat_type === 'none') {
    throw new Error('analysis.json security.threat_type must match a finding type when findings is non-empty');
  }
}

function assertUnitNumber(value, label) {
  if (typeof value !== 'number' || Number.isNaN(value) || value < 0 || value > 1) {
    throw new Error(`analysis.json ${label} must be a number between 0 and 1`);
  }
}

function buildAnalysisRecord({ target, model, status, analyzedAt, durationMs, analysis, error }) {
  return {
    node: target.node,
    name: target.name,
    root_cid: target.rootCid,
    model,
    status,
    analyzed_at: analyzedAt,
    duration_ms: durationMs,
    category: analysis?.category ?? null,
    category_confidence: analysis?.category_confidence ?? null,
    quality_tier: analysis?.quality?.tier ?? null,
    quality_score: analysis?.quality?.score ?? null,
    security_risk: analysis?.security?.risk ?? null,
    security_risk_score: analysis?.security?.risk_score ?? null,
    security_threat_type: analysis?.security?.threat_type ?? null,
    safe_to_list: analysis?.security ? Number(analysis.security.safe_to_list) : null,
    summary: analysis?.summary ?? null,
    analysis_json: analysis ? JSON.stringify(analysis) : null,
    error,
  };
}

function summarizeProcessOutput(stdout, stderr) {
  const text = `${stderr}\n${stdout}`.trim();
  return text ? text.slice(-4000) : null;
}

function safePathSegment(value) {
  return String(value).replace(/[^a-zA-Z0-9._-]/g, '_').slice(0, 200);
}
