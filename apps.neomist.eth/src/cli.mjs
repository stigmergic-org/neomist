import process from 'node:process';
import { DEFAULTS } from './config.mjs';
import { openStore } from './db.mjs';
import { exportIpfsTree } from './export-ipfs-tree.mjs';
import { syncNames } from './sync-names.mjs';

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});

async function main() {
  const [command, ...args] = process.argv.slice(2);
  if (!command || command === '--help' || command === '-h') {
    printGeneralHelp();
    return;
  }

  if (isHelpRequested(args)) {
    printCommandHelp(command);
    return;
  }

  const store = await openStore();
  try {
    switch (command) {
      case 'sync-names':
        await runSyncNames(store, parseFlags(args));
        break;
      case 'export-ipfs':
        await runExportIpfs(store);
        break;
      case 'db-stats':
        printJson(store.getStats());
        break;
      case 'list-names':
        runListNames(store, parseFlags(args));
        break;
      case 'show-name':
        runShowName(store, args);
        break;
      case 'list-probe-failures':
        runListProbeFailures(store, parseFlags(args));
        break;
      default:
        throw new Error(`unknown command ${command}`);
    }
  } finally {
    await store.close();
  }
}

async function runSyncNames(store, flags) {
  const summary = await syncNames(store, {
    limit: parseIntegerFlag(flags.limit, DEFAULTS.syncLimit),
    eventBatchSize: parseIntegerFlag(flags['batch-size'], DEFAULTS.eventBatchSize),
    headReplayBlocks: parseIntegerFlag(flags['head-replay-blocks'], DEFAULTS.headReplayBlocks),
    probeConcurrency: parseIntegerFlag(flags['probe-concurrency'], DEFAULTS.probeConcurrency),
    timeoutMs: parseIntegerFlag(flags['timeout-ms'], DEFAULTS.timeoutMs),
    maxBytes: parseIntegerFlag(flags['max-bytes'], DEFAULTS.maxBytes),
    logger: logInfo,
  });
  printJson(summary);
}

async function runExportIpfs(store) {
  const summary = await exportIpfsTree(store);
  printJson(summary);
}

function runListNames(store, flags) {
  const limit = parseIntegerFlag(flags.limit, 50);
  const rows = store.listNames(limit);
  const versions = store.listContenthashVersionsForNodes(rows.map((row) => row.node));
  const versionsByNode = groupVersionsByNode(versions);

  printJson(rows.map((row) => ({
    node: row.node,
    name: row.name,
    parent_name: row.parent_name,
    is_subdomain: Boolean(row.is_subdomain),
    last_probe_status: row.last_probe_status,
    last_probe_success: Boolean(row.last_probe_success),
    contenthashes: versionsByNode.get(row.node) ?? [],
  })));
}

function runShowName(store, args) {
  const identifier = args.find((arg) => !arg.startsWith('--'));
  if (!identifier) {
    throw new Error('show-name requires name or node argument');
  }
  const row = store.showName(identifier);
  if (!row) {
    throw new Error(`name not found: ${identifier}`);
  }
  printJson(row);
}

function runListProbeFailures(store, flags) {
  const limit = parseIntegerFlag(flags.limit, 50);
  printJson(store.listProbeFailures(limit));
}

function parseFlags(args) {
  const flags = {};
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg.startsWith('--')) {
      continue;
    }
    const key = arg.slice(2);
    const next = args[index + 1];
    if (!next || next.startsWith('--')) {
      flags[key] = true;
      continue;
    }
    flags[key] = next;
    index += 1;
  }
  return flags;
}

function parseIntegerFlag(value, fallback) {
  if (value == null) {
    return fallback;
  }
  const parsed = Number.parseInt(value, 10);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`invalid integer flag value: ${value}`);
  }
  return parsed;
}

function printJson(value) {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

function groupVersionsByNode(rows) {
  const grouped = new Map();
  for (const row of rows) {
    const entries = grouped.get(row.node) ?? [];
    entries.push({
      contenthash_protocol: row.contenthash_protocol,
      root_cid: row.root_cid,
      contenthash_set_block: row.source_block,
      contenthash_set_tx_hash: row.source_tx_hash,
    });
    grouped.set(row.node, entries);
  }
  return grouped;
}

function logInfo(message) {
  process.stderr.write(`${new Date().toISOString()} ${message}\n`);
}

function isHelpRequested(args) {
  return args.includes('--help') || args.includes('-h');
}

function printCommandHelp(command) {
  switch (command) {
    case 'sync-names':
      printSyncNamesHelp();
      return;
    case 'export-ipfs':
      printExportIpfsHelp();
      return;
    case 'db-stats':
      printDbStatsHelp();
      return;
    case 'list-names':
      printListNamesHelp();
      return;
    case 'show-name':
      printShowNameHelp();
      return;
    case 'list-probe-failures':
      printListProbeFailuresHelp();
      return;
    default:
      printGeneralHelp();
  }
}

function printGeneralHelp() {
  process.stdout.write(`Usage: node src/cli.mjs <command> [options]\n\n`);
  process.stdout.write(`Commands:\n`);
  process.stdout.write(`  sync-names            head sync recent ENSNode events, then backfill older ones, store current names, probe eth.link\n`);
  process.stdout.write(`  export-ipfs           export successful current names into ipfs-root\n`);
  process.stdout.write(`  db-stats              print SQLite stats\n`);
  process.stdout.write(`  list-names            print stored names (default limit 50)\n`);
  process.stdout.write(`  show-name             print one name or node plus latest probe\n`);
  process.stdout.write(`  list-probe-failures   print latest failed probes (default limit 50)\n\n`);
  process.stdout.write(`Run \`<command> -h\` for command-specific help.\n`);
}

function printSyncNamesHelp() {
  process.stdout.write(`Usage: node src/cli.mjs sync-names [options]\n\n`);
  process.stdout.write(`Head sync recent ENSNode events, then backfill older ones.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --limit N                 max historical names to backfill this run (default ${DEFAULTS.syncLimit})\n`);
  process.stdout.write(`  --batch-size N            ENSNode event page size (default ${DEFAULTS.eventBatchSize})\n`);
  process.stdout.write(`  --head-replay-blocks N    recent block replay window for head sync (default ${DEFAULTS.headReplayBlocks})\n`);
  process.stdout.write(`  --probe-concurrency N     concurrent eth.link probes (default ${DEFAULTS.probeConcurrency})\n`);
  process.stdout.write(`  --timeout-ms N            probe timeout (default ${DEFAULTS.timeoutMs})\n`);
  process.stdout.write(`  --max-bytes N             max probe body bytes (default ${DEFAULTS.maxBytes})\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printExportIpfsHelp() {
  process.stdout.write(`Usage: node src/cli.mjs export-ipfs\n\n`);
  process.stdout.write(`Export current names with successful latest probe into ipfs-root.\n`);
  process.stdout.write(`Existing files in ipfs-root are not deleted.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printDbStatsHelp() {
  process.stdout.write(`Usage: node src/cli.mjs db-stats\n\n`);
  process.stdout.write(`Print SQLite stats and sync cursors.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printListNamesHelp() {
  process.stdout.write(`Usage: node src/cli.mjs list-names [options]\n\n`);
  process.stdout.write(`Print stored names from SQLite.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --limit N                 number of rows to print (default 50)\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printShowNameHelp() {
  process.stdout.write(`Usage: node src/cli.mjs show-name <name|node>\n\n`);
  process.stdout.write(`Print one stored name record plus latest probe.\n\n`);
  process.stdout.write(`Arguments:\n`);
  process.stdout.write(`  <name|node>               ENS name like vitalik.eth or node like 0x...\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printListProbeFailuresHelp() {
  process.stdout.write(`Usage: node src/cli.mjs list-probe-failures [options]\n\n`);
  process.stdout.write(`Print latest failed eth.link probes.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --limit N                 number of rows to print (default 50)\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}
