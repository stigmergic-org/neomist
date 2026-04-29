import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SRC_DIR = path.dirname(fileURLToPath(import.meta.url));
export const PACKAGE_ROOT = path.resolve(SRC_DIR, '..');

export const PATHS = {
  packageRoot: PACKAGE_ROOT,
  schemaPath: path.join(PACKAGE_ROOT, 'sql', 'schema.sql'),
  stateDir: path.join(PACKAGE_ROOT, 'state'),
  dbPath: path.join(PACKAGE_ROOT, 'state', 'index.sqlite'),
  ipfsRootDir: path.join(PACKAGE_ROOT, 'ipfs-root'),
};

export const DEFAULTS = {
  ensnodeUrl: process.env.APPS_NEOMIST_ENSNODE_URL || process.env.ENSNODE_URL || 'https://api.alpha.ensnode.io/subgraph',
  eventBatchSize: 100,
  syncLimit: 200,
  headReplayBlocks: 100,
  probeConcurrency: 5,
  timeoutMs: 20_000,
  maxBytes: 5 * 1024 * 1024,
  excludedNamespaceSuffixes: ['base.eth', 'linea.eth'],
};

export const SYNC_STATE_KEYS = {
  headCursorBlockInclusive: 'contenthash_head_cursor_block_inclusive',
  backfillCursorBlockExclusive: 'contenthash_backfill_cursor_block_exclusive',
  legacyCursorBlockExclusive: 'contenthash_cursor_block_exclusive',
};
