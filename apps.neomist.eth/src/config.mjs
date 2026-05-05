import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SRC_DIR = path.dirname(fileURLToPath(import.meta.url));
export const PACKAGE_ROOT = path.resolve(SRC_DIR, '..');

export const PATHS = {
  packageRoot: PACKAGE_ROOT,
  schemaPath: path.join(PACKAGE_ROOT, 'sql', 'schema.sql'),
  stateDir: path.join(PACKAGE_ROOT, 'state'),
  dbPath: process.env.APPS_NEOMIST_DB_PATH || path.join(PACKAGE_ROOT, 'state', 'index.sqlite'),
  ipfsRootDir: path.join(PACKAGE_ROOT, 'ipfs-root'),
  analysisWorkDir: path.join(PACKAGE_ROOT, 'state', 'analysis-work'),
  analysisPromptPath: path.join(PACKAGE_ROOT, 'prompts', 'ipfs-app-analysis-system.md'),
  wacPath: path.join(PACKAGE_ROOT, 'wac'),
};

export const DEFAULTS = {
  ensnodeUrl: process.env.APPS_NEOMIST_ENSNODE_URL || process.env.ENSNODE_URL || 'https://api.alpha.ensnode.io/subgraph',
  kuboRpcUrl: process.env.APPS_NEOMIST_KUBO_RPC_URL || process.env.KUBO_RPC_URL || process.env.IPFS_API || 'http://127.0.0.1:5001/api/v0',
  eventBatchSize: 100,
  syncLimit: 200,
  headReplayBlocks: 100,
  probeConcurrency: 5,
  timeoutMs: 20_000,
  analysisTimeoutMs: 300_000,
  analysisModel: process.env.APPS_NEOMIST_ANALYSIS_MODEL || 'openai/gpt-5.4-mini',
  analysisMemoryLimit: process.env.APPS_NEOMIST_ANALYSIS_MEMORY_LIMIT || '1g',
  maxBytes: 5 * 1024 * 1024,
  excludedNamespaceSuffixes: ['base.eth', 'linea.eth'],
};

export const SYNC_STATE_KEYS = {
  headCursorBlockInclusive: 'contenthash_head_cursor_block_inclusive',
  backfillCursorBlockExclusive: 'contenthash_backfill_cursor_block_exclusive',
  legacyCursorBlockExclusive: 'contenthash_cursor_block_exclusive',
};
