import process from 'node:process';
import { analyzeMissingNames, analyzeName } from './analyze.mjs';
import { DEFAULTS } from './config.mjs';
import { openStore } from './db.mjs';
import { exportIpfsTree } from './export-ipfs-tree.mjs';
import { publishIpfsRootToEns } from './publish-ens.mjs';
import { reprobeNames } from './reprobe.mjs';
import { syncName, syncNames } from './sync-names.mjs';

const BOOLEAN_FLAGS = new Set(['details', 'dry-run', 'force', 'full-backfill', 'no-pin', 'only-missing-icons', 'retry-failed', 'skip-probe']);

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});

async function main() {
  const argv = process.argv.slice(2);
  if (argv[0] === '--') {
    argv.shift();
  }
  const [command, ...rawArgs] = argv;
  const args = rawArgs[0] === '--' ? rawArgs.slice(1) : rawArgs;
  if (!command || command === '--help' || command === '-h') {
    printGeneralHelp();
    return;
  }

  if (isHelpRequested(args)) {
    printCommandHelp(command);
    return;
  }

  if (command === 'publish-ens') {
    await runPublishEns(parseFlags(args));
    return;
  }

  const store = await openStore();
  try {
    switch (command) {
      case 'sync-names':
        await runSyncNames(store, parseFlags(args));
        break;
      case 'sync-name':
        await runSyncName(store, args);
        break;
      case 'reprobe-names':
        await runReprobeNames(store, parseFlags(args));
        break;
      case 'analyze-name':
      case 'analyse-name':
        await runAnalyzeName(store, args);
        break;
      case 'analyze-names':
      case 'analyse-names':
        await runAnalyzeNames(store, parseFlags(args));
        break;
      case 'export-ipfs':
        await runExportIpfs(store);
        break;
      case 'daily':
        await runDaily(store, parseFlags(args));
        break;
      case 'db-stats':
        runDbStats(store, parseFlags(args));
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
    limit: flags['full-backfill'] ? Number.POSITIVE_INFINITY : parseIntegerFlag(flags.limit, DEFAULTS.syncLimit),
    eventBatchSize: parseIntegerFlag(flags['batch-size'], DEFAULTS.eventBatchSize),
    kuboRpcUrl: flags['kubo-rpc-url'] ?? DEFAULTS.kuboRpcUrl,
    headReplayBlocks: parseIntegerFlag(flags['head-replay-blocks'], DEFAULTS.headReplayBlocks),
    probeConcurrency: parseIntegerFlag(flags['probe-concurrency'], DEFAULTS.probeConcurrency),
    timeoutMs: parseIntegerFlag(flags['timeout-ms'], DEFAULTS.timeoutMs),
    maxBytes: parseIntegerFlag(flags['max-bytes'], DEFAULTS.maxBytes),
    logger: logInfo,
  });
  printJson(summary);
}

async function runSyncName(store, args) {
  const flags = parseFlags(args);
  const [identifier] = positionalArgs(args);
  if (!identifier) {
    throw new Error('sync-name requires name or node argument');
  }

  const summary = await syncName(store, identifier, {
    kuboRpcUrl: flags['kubo-rpc-url'] ?? DEFAULTS.kuboRpcUrl,
    timeoutMs: parseIntegerFlag(flags['timeout-ms'], DEFAULTS.timeoutMs),
    maxBytes: parseIntegerFlag(flags['max-bytes'], DEFAULTS.maxBytes),
    skipProbe: Boolean(flags['skip-probe']),
  });
  printJson(summary);
}

async function runReprobeNames(store, flags) {
  const summary = await reprobeNames(store, {
    limit: flags.limit == null ? null : parseIntegerFlag(flags.limit, null),
    onlyMissingIcons: Boolean(flags['only-missing-icons']),
    kuboRpcUrl: flags['kubo-rpc-url'] ?? DEFAULTS.kuboRpcUrl,
    probeConcurrency: parseIntegerFlag(flags['probe-concurrency'], DEFAULTS.probeConcurrency),
    timeoutMs: parseIntegerFlag(flags['timeout-ms'], DEFAULTS.timeoutMs),
    maxBytes: parseIntegerFlag(flags['max-bytes'], DEFAULTS.maxBytes),
    logger: logInfo,
  });
  printJson(summary);
}

async function runAnalyzeName(store, args) {
  const flags = parseFlags(args);
  const [identifier] = positionalArgs(args);
  if (!identifier) {
    throw new Error('analyze-name requires name or node argument');
  }

  const summary = await analyzeName(store, identifier, {
    identifier,
    cid: flags.cid,
    model: flags.model ?? DEFAULTS.analysisModel,
    memoryLimit: flags['memory-limit'] ?? DEFAULTS.analysisMemoryLimit,
    timeoutMs: parseIntegerFlag(flags['timeout-ms'], DEFAULTS.analysisTimeoutMs),
    force: Boolean(flags.force),
  });
  printJson(summary);
}

async function runAnalyzeNames(store, flags) {
  const summary = await analyzeMissingNames(store, {
    limit: parseIntegerFlag(flags.limit, 10),
    model: flags.model ?? DEFAULTS.analysisModel,
    memoryLimit: flags['memory-limit'] ?? DEFAULTS.analysisMemoryLimit,
    timeoutMs: parseIntegerFlag(flags['timeout-ms'], DEFAULTS.analysisTimeoutMs),
    force: Boolean(flags.force),
    retryFailed: Boolean(flags['retry-failed']),
    logger: logInfo,
  });
  printJson(summary);
}

async function runExportIpfs(store) {
  const summary = await exportIpfsTree(store);
  printJson(summary);
}

async function runPublishEns(flags) {
  const summary = await publishIpfsRootToEns(buildPublishEnsOptions(flags));
  printJson(summary);
}

async function runDaily(store, flags) {
  logInfo('daily: sync-names');
  const sync = await syncNames(store, {
    limit: flags['full-backfill'] ? Number.POSITIVE_INFINITY : parseIntegerFlag(flags['sync-limit'], DEFAULTS.syncLimit),
    eventBatchSize: parseIntegerFlag(flags['batch-size'], DEFAULTS.eventBatchSize),
    kuboRpcUrl: flags['kubo-rpc-url'] ?? DEFAULTS.kuboRpcUrl,
    headReplayBlocks: parseIntegerFlag(flags['head-replay-blocks'], DEFAULTS.headReplayBlocks),
    probeConcurrency: parseIntegerFlag(flags['probe-concurrency'], DEFAULTS.probeConcurrency),
    timeoutMs: parseIntegerFlag(flags['sync-timeout-ms'] ?? flags['timeout-ms'], DEFAULTS.timeoutMs),
    maxBytes: parseIntegerFlag(flags['max-bytes'], DEFAULTS.maxBytes),
    logger: logInfo,
  });

  logInfo('daily: analyze-names');
  const analysis = await analyzeMissingNames(store, {
    limit: parseIntegerFlag(flags['analysis-limit'], 500),
    model: flags.model ?? DEFAULTS.analysisModel,
    memoryLimit: flags['memory-limit'] ?? DEFAULTS.analysisMemoryLimit,
    timeoutMs: parseIntegerFlag(flags['analysis-timeout-ms'], DEFAULTS.analysisTimeoutMs),
    force: Boolean(flags.force),
    retryFailed: Boolean(flags['retry-failed']),
    logger: logInfo,
  });

  logInfo('daily: export-ipfs');
  const exported = await exportIpfsTree(store);

  logInfo('daily: publish-ens');
  const publish = await publishIpfsRootToEns(buildPublishEnsOptions(flags));

  printJson({ sync, analysis, export: exported, publish });
}

function buildPublishEnsOptions(flags) {
  return {
    name: parseOptionalStringFlag(flags.name, 'name'),
    rootCid: parseOptionalStringFlag(flags.cid, 'cid'),
    rpcUrl: parseOptionalStringFlag(flags['rpc-url'], 'rpc-url'),
    kuboRpcUrl: parseOptionalStringFlag(flags['kubo-rpc-url'], 'kubo-rpc-url'),
    envPath: parseOptionalStringFlag(flags['env-file'], 'env-file'),
    maxGasPriceMwei: parseOptionalStringFlag(flags['max-gas-price-mwei'], 'max-gas-price-mwei'),
    publishCooldownDays: parseOptionalStringFlag(flags['publish-cooldown-days'], 'publish-cooldown-days'),
    pin: !flags['no-pin'],
    dryRun: Boolean(flags['dry-run']),
    logger: logInfo,
  };
}

function runDbStats(store, flags) {
  printDbStats(store.getStats({
    category: parseOptionalStringFlag(flags.category, 'category'),
  }));
}

function printDbStats(stats) {
  const lines = [];
  const overview = stats.overview;
  const coverage = stats.coverage;
  const appScope = stats.app_scope;

  lines.push('DB Stats');
  if (stats.filter?.category) {
    lines.push('');
    lines.push('Filter');
    lines.push(`- Category: ${stats.filter.category}`);
  }

  lines.push('');
  lines.push('Overview');
  lines.push(`- Names: ${formatNumber(overview.names)} total`);
  lines.push(`- Probe success: ${formatNumber(overview.successful_names)} successful, ${formatNumber(overview.failed_or_unprobed_names)} failed/unprobed`);
  lines.push(`- Versions: ${formatNumber(overview.name_versions)}`);
  lines.push(`- Probes: ${formatNumber(overview.probes)}`);
  lines.push(`- Cursors: head ${formatOptionalNumber(overview.head_cursor_block_inclusive)}, backfill ${formatOptionalNumber(overview.backfill_cursor_block_exclusive)}`);

  lines.push('');
  lines.push('Analysis Coverage');
  lines.push(`- Covered: ${formatNumber(coverage.current_successful_analyzed_names)}/${formatNumber(coverage.current_successful_names)} current successful names (${formatPercent(coverage.current_successful_analysis_coverage)})`);
  lines.push(`- Missing: ${formatNumber(coverage.missing_current_successful_analysis_names)}`);
  lines.push(`- Failed/incomplete attempts: ${formatNumber(coverage.failed_or_incomplete_analyses)}`);
  lines.push(`- Timed out attempts: ${formatNumber(coverage.timed_out_analyses)}`);

  lines.push('');
  lines.push('Apps');
  lines.push(`- Analyzed current names: ${formatNumber(appScope.current_successful_analyzed_apps)}`);
  lines.push(`- Unique CIDs: ${formatNumber(appScope.unique_root_cids)}`);

  addListSection(lines, 'Categories', stats.apps_by_category, 'category', 'apps', 10);
  addListSection(lines, 'Quality', stats.quality_tiers, 'quality_tier', 'apps');
  addListSection(lines, 'Security Risk', stats.security_risks, 'security_risk', 'apps');

  lines.push('');
  lines.push('Safety');
  lines.push(`- Safe: ${formatNumber(stats.safe_to_list.safe)}`);
  lines.push(`- Unsafe: ${formatNumber(stats.safe_to_list.unsafe)}`);
  lines.push(`- Unknown: ${formatNumber(stats.safe_to_list.unknown)}`);

  lines.push('');
  lines.push('Threats');
  lines.push(`- Threatened apps: ${formatNumber(stats.threats.threatened_apps)}`);
  lines.push(`- High/critical risk apps: ${formatNumber(stats.threats.high_or_critical_risk_apps)}`);
  addListRows(lines, stats.threat_types, 'threat_type', 'apps', 5);

  addListSection(lines, 'Domain Shape', stats.domain_shape, 'shape', 'apps');
  addListSection(lines, 'Contenthash Protocol', stats.contenthash_protocols, 'contenthash_protocol', 'apps');
  process.stdout.write(`${lines.join('\n')}\n`);
}

function addListSection(lines, title, rows, labelKey, countKey, limit = Number.POSITIVE_INFINITY) {
  lines.push('');
  lines.push(title);
  addListRows(lines, rows, labelKey, countKey, limit);
}

function addListRows(lines, rows, labelKey, countKey, limit = Number.POSITIVE_INFINITY) {
  if (!rows || rows.length === 0) {
    lines.push('- None');
    return;
  }
  for (const row of rows.slice(0, limit)) {
    lines.push(`- ${row[labelKey] ?? 'unknown'}: ${formatNumber(row[countKey])}`);
  }
}

function formatNumber(value) {
  return Number(value ?? 0).toLocaleString('en-US');
}

function formatOptionalNumber(value) {
  return value == null ? 'none' : formatNumber(value);
}

function formatPercent(value) {
  return `${((value ?? 0) * 100).toFixed(1)}%`;
}

function runListNames(store, flags) {
  const limit = parseIntegerFlag(flags.limit, 50);
  const sort = parseListNamesSort(flags.sort);
  const rows = store.listNames(limit, { sort });
  if (!flags.details) {
    printJson(rows.map((row) => row.name));
    return;
  }

  const versions = store.listContenthashVersionsForNodes(rows.map((row) => row.node));
  const versionsByNode = groupVersionsByNode(versions);

  printJson(rows.map((row) => ({
    node: row.node,
    name: row.name,
    parent_name: row.parent_name,
    is_subdomain: Boolean(row.is_subdomain),
    last_probe_status: row.last_probe_status,
    last_probe_success: Boolean(row.last_probe_success),
    analysis: buildAnalysisListSummary(row),
    contenthashes: versionsByNode.get(row.node) ?? [],
  })));
}

function buildAnalysisListSummary(row) {
  if (row.analysis_quality_score == null) {
    return null;
  }
  return {
    category: row.analysis_category,
    quality_tier: row.analysis_quality_tier,
    quality_score: row.analysis_quality_score,
    security_risk: row.analysis_security_risk,
    security_threat_type: row.analysis_security_threat_type,
    safe_to_list: row.analysis_safe_to_list == null ? null : Boolean(row.analysis_safe_to_list),
  };
}

function runShowName(store, args) {
  const [identifier] = positionalArgs(args);
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
    if (BOOLEAN_FLAGS.has(key)) {
      flags[key] = true;
      continue;
    }
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

function positionalArgs(args) {
  const positionals = [];
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg.startsWith('--')) {
      const key = arg.slice(2);
      const next = args[index + 1];
      if (!BOOLEAN_FLAGS.has(key) && next && !next.startsWith('--')) {
        index += 1;
      }
      continue;
    }
    positionals.push(arg);
  }
  return positionals;
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

function parseOptionalStringFlag(value, label) {
  if (value == null) {
    return undefined;
  }
  if (value === true || String(value).trim() === '') {
    throw new Error(`--${label} requires value`);
  }
  return String(value);
}

function parseListNamesSort(value) {
  const sort = parseOptionalStringFlag(value, 'sort') ?? 'name';
  switch (sort) {
    case 'name':
      return 'name';
    case 'score':
    case 'quality-score':
    case 'quality_score':
      return 'score';
    default:
      throw new Error(`invalid --sort value: ${sort}`);
  }
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
    case 'sync-name':
      printSyncNameHelp();
      return;
    case 'reprobe-names':
      printReprobeNamesHelp();
      return;
    case 'analyze-name':
    case 'analyse-name':
      printAnalyzeNameHelp();
      return;
    case 'analyze-names':
    case 'analyse-names':
      printAnalyzeNamesHelp();
      return;
    case 'export-ipfs':
      printExportIpfsHelp();
      return;
    case 'publish-ens':
      printPublishEnsHelp();
      return;
    case 'daily':
      printDailyHelp();
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
  process.stdout.write(`  sync-names            head sync recent ENSNode events, then backfill older ones, store current names, probe via Kubo RPC\n`);
  process.stdout.write(`  sync-name             sync one ENS name or node, then probe via Kubo RPC\n`);
  process.stdout.write(`  reprobe-names         refresh Kubo probe rows for stored names\n`);
  process.stdout.write(`  analyze-name          analyze one synced name through WAC/OpenCode\n`);
  process.stdout.write(`  analyze-names         analyze synced names with unattempted current CID\n`);
  process.stdout.write(`  export-ipfs           export successful current names into ipfs-root\n`);
  process.stdout.write(`  publish-ens           add ipfs-root to IPFS and publish its CID to apps.neomist.eth\n`);
  process.stdout.write(`  daily                 sync, analyze, export ipfs-root, and conditionally publish ENS\n`);
  process.stdout.write(`  db-stats              print SQLite stats\n`);
  process.stdout.write(`  list-names            print stored names (default limit 50)\n`);
  process.stdout.write(`  show-name             print one name or node plus latest probe\n`);
  process.stdout.write(`  list-probe-failures   print latest failed probes (default limit 50)\n\n`);
  process.stdout.write(`Run \`<command> -h\` for command-specific help.\n`);
}

function printAnalyzeNameHelp() {
  process.stdout.write(`Usage: node src/cli.mjs analyze-name <name|node> [options]\n\n`);
  process.stdout.write(`Analyze one synced name via WAC/OpenCode and store result in SQLite.\n\n`);
  process.stdout.write(`Arguments:\n`);
  process.stdout.write(`  <name|node>               ENS name like vitalik.eth or node like 0x...\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --cid CID                 analyze this CID instead of the current name root CID\n`);
  process.stdout.write(`  --model MODEL             opencode model (default ${DEFAULTS.analysisModel})\n`);
  process.stdout.write(`  --memory-limit N          WAC container memory limit (default ${DEFAULTS.analysisMemoryLimit})\n`);
  process.stdout.write(`  --timeout-ms N            analysis timeout (default ${DEFAULTS.analysisTimeoutMs})\n`);
  process.stdout.write(`  --force                   re-run even if successful analysis exists\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printAnalyzeNamesHelp() {
  process.stdout.write(`Usage: node src/cli.mjs analyze-names [options]\n\n`);
  process.stdout.write(`Analyze synced successful names whose current CID has not been attempted.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --limit N                 max names to analyze (default 10)\n`);
  process.stdout.write(`  --model MODEL             opencode model (default ${DEFAULTS.analysisModel})\n`);
  process.stdout.write(`  --memory-limit N          WAC container memory limit (default ${DEFAULTS.analysisMemoryLimit})\n`);
  process.stdout.write(`  --timeout-ms N            per-name analysis timeout (default ${DEFAULTS.analysisTimeoutMs})\n`);
  process.stdout.write(`  --retry-failed            include CIDs with failed, timeout, or invalid prior analysis\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printSyncNameHelp() {
  process.stdout.write(`Usage: node src/cli.mjs sync-name <name|node> [options]\n\n`);
  process.stdout.write(`Fetch one current ENS name from ENSNode, store it, and probe via Kubo RPC.\n\n`);
  process.stdout.write(`Arguments:\n`);
  process.stdout.write(`  <name|node>               ENS name like vitalik.eth or node like 0x...\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --kubo-rpc-url URL        Kubo RPC API URL or multiaddr (default ${DEFAULTS.kuboRpcUrl})\n`);
  process.stdout.write(`  --timeout-ms N            probe timeout (default ${DEFAULTS.timeoutMs})\n`);
  process.stdout.write(`  --max-bytes N             max probe body bytes (default ${DEFAULTS.maxBytes})\n`);
  process.stdout.write(`  --skip-probe              store ENSNode data without Kubo probe\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printSyncNamesHelp() {
  process.stdout.write(`Usage: node src/cli.mjs sync-names [options]\n\n`);
  process.stdout.write(`Head sync recent ENSNode events, then backfill older ones.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --limit N                 max historical names to backfill this run (default ${DEFAULTS.syncLimit})\n`);
  process.stdout.write(`  --full-backfill           backfill until no older contenthash events remain\n`);
  process.stdout.write(`  --batch-size N            ENSNode event page size (default ${DEFAULTS.eventBatchSize})\n`);
  process.stdout.write(`  --kubo-rpc-url URL        Kubo RPC API URL or multiaddr (default ${DEFAULTS.kuboRpcUrl})\n`);
  process.stdout.write(`  --head-replay-blocks N    recent block replay window for head sync (default ${DEFAULTS.headReplayBlocks})\n`);
  process.stdout.write(`  --probe-concurrency N     concurrent Kubo probes (default ${DEFAULTS.probeConcurrency})\n`);
  process.stdout.write(`  --timeout-ms N            probe timeout (default ${DEFAULTS.timeoutMs})\n`);
  process.stdout.write(`  --max-bytes N             max probe body bytes (default ${DEFAULTS.maxBytes})\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printReprobeNamesHelp() {
  process.stdout.write(`Usage: node src/cli.mjs reprobe-names [options]\n\n`);
  process.stdout.write(`Re-run Kubo probes for current stored names and insert fresh probe rows.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --only-missing-icons      only probe names whose latest probe has no icon URL\n`);
  process.stdout.write(`  --limit N                 max names to reprobe (default all matching names)\n`);
  process.stdout.write(`  --probe-concurrency N     concurrent Kubo probes (default ${DEFAULTS.probeConcurrency})\n`);
  process.stdout.write(`  --kubo-rpc-url URL        Kubo RPC API URL or multiaddr (default ${DEFAULTS.kuboRpcUrl})\n`);
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

function printPublishEnsHelp() {
  process.stdout.write(`Usage: node src/cli.mjs publish-ens [options]\n\n`);
  process.stdout.write(`Add ipfs-root through Kubo RPC, then set ENS contenthash for apps.neomist.eth.\n`);
  process.stdout.write(`Publishing is skipped unless gas price is below --max-gas-price-mwei and no local publish marker exists within --publish-cooldown-days.\n`);
  process.stdout.write(`Reads private key from .env using APPS_NEOMIST_ENS_PRIVATE_KEY, ENS_PRIVATE_KEY, or PRIVATE_KEY.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --name NAME               ENS name to update (default apps.neomist.eth)\n`);
  process.stdout.write(`  --cid CID                 publish an existing IPFS root CID instead of adding ipfs-root\n`);
  process.stdout.write(`  --rpc-url URL             Ethereum mainnet RPC URL (env APPS_NEOMIST_ETH_RPC_URL, ETH_RPC_URL, MAINNET_RPC_URL, or RPC_URL)\n`);
  process.stdout.write(`  --kubo-rpc-url URL        Kubo RPC API URL or multiaddr (default ${DEFAULTS.kuboRpcUrl})\n`);
  process.stdout.write(`  --env-file PATH           dotenv file to load before defaults\n`);
  process.stdout.write(`  --max-gas-price-mwei N    max gas price before skipping publish (default 400)\n`);
  process.stdout.write(`  --publish-cooldown-days N days after local publish marker before publishing again (default 3)\n`);
  process.stdout.write(`  --no-pin                  add ipfs-root without pinning blocks\n`);
  process.stdout.write(`  --dry-run                 compute CID and contenthash without sending transaction\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printDailyHelp() {
  process.stdout.write(`Usage: node src/cli.mjs daily [options]\n\n`);
  process.stdout.write(`Run sync-names, analyze-names, export-ipfs, then publish-ens.\n`);
  process.stdout.write(`Publishing remains conditional on gas price and local publish cooldown.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --sync-limit N            max historical names to backfill during sync (default ${DEFAULTS.syncLimit})\n`);
  process.stdout.write(`  --analysis-limit N        max names to analyze (default 500)\n`);
  process.stdout.write(`  --full-backfill           backfill until no older contenthash events remain\n`);
  process.stdout.write(`  --batch-size N            ENSNode event page size (default ${DEFAULTS.eventBatchSize})\n`);
  process.stdout.write(`  --kubo-rpc-url URL        Kubo RPC API URL or multiaddr (default ${DEFAULTS.kuboRpcUrl})\n`);
  process.stdout.write(`  --head-replay-blocks N    recent block replay window for head sync (default ${DEFAULTS.headReplayBlocks})\n`);
  process.stdout.write(`  --probe-concurrency N     concurrent Kubo probes (default ${DEFAULTS.probeConcurrency})\n`);
  process.stdout.write(`  --sync-timeout-ms N       probe timeout during sync (default ${DEFAULTS.timeoutMs})\n`);
  process.stdout.write(`  --analysis-timeout-ms N   per-name analysis timeout (default ${DEFAULTS.analysisTimeoutMs})\n`);
  process.stdout.write(`  --max-bytes N             max probe body bytes (default ${DEFAULTS.maxBytes})\n`);
  process.stdout.write(`  --model MODEL             opencode model (default ${DEFAULTS.analysisModel})\n`);
  process.stdout.write(`  --memory-limit N          WAC container memory limit (default ${DEFAULTS.analysisMemoryLimit})\n`);
  process.stdout.write(`  --retry-failed            include CIDs with failed, timeout, or invalid prior analysis\n`);
  process.stdout.write(`  --name NAME               ENS name to update (default apps.neomist.eth)\n`);
  process.stdout.write(`  --rpc-url URL             Ethereum mainnet RPC URL\n`);
  process.stdout.write(`  --env-file PATH           dotenv file to load before defaults\n`);
  process.stdout.write(`  --max-gas-price-mwei N    max gas price before skipping publish (default 400)\n`);
  process.stdout.write(`  --publish-cooldown-days N days after local publish marker before publishing again (default 3)\n`);
  process.stdout.write(`  --no-pin                  add ipfs-root without pinning blocks\n`);
  process.stdout.write(`  --dry-run                 do not send ENS transaction\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printDbStatsHelp() {
  process.stdout.write(`Usage: node src/cli.mjs db-stats [options]\n\n`);
  process.stdout.write(`Print compact SQLite stats, sync cursors, and app analysis breakdowns.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --category CATEGORY       filter app breakdowns to one analysis category (case-insensitive)\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}

function printListNamesHelp() {
  process.stdout.write(`Usage: node src/cli.mjs list-names [options]\n\n`);
  process.stdout.write(`Print stored names from SQLite.\n\n`);
  process.stdout.write(`Example: node src/cli.mjs list-names --sort score --limit 10 --details\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --limit N                 number of rows to print (default 50)\n`);
  process.stdout.write(`  --sort name|score         sort by name (default) or analysis quality score descending\n`);
  process.stdout.write(`  --details                 print full name records and contenthashes\n`);
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
  process.stdout.write(`Print latest failed contenthash probes.\n\n`);
  process.stdout.write(`Options:\n`);
  process.stdout.write(`  --limit N                 number of rows to print (default 50)\n`);
  process.stdout.write(`  -h, --help                show this help\n`);
}
