import { decodeContenthash } from './contenthash.mjs';
import { DEFAULTS } from './config.mjs';
import {
  fetchAllContenthashEventsForBlock,
  fetchContenthashEventPage,
  fetchContenthashEventsSince,
  fetchDomainsByIds,
  fetchLatestContenthashBlock,
} from './ensnode.mjs';
import { isMainnetEnsName, isSubdomain, parentName } from './filters.mjs';
import { probeEthLinkName } from './probe.mjs';

export async function syncNames(store, options = {}) {
  const ensnodeUrl = options.ensnodeUrl ?? DEFAULTS.ensnodeUrl;
  const backfillLimit = options.limit ?? DEFAULTS.syncLimit;
  const eventBatchSize = options.eventBatchSize ?? DEFAULTS.eventBatchSize;
  const headReplayBlocks = options.headReplayBlocks ?? DEFAULTS.headReplayBlocks;
  const probeConcurrency = options.probeConcurrency ?? DEFAULTS.probeConcurrency;
  const timeoutMs = options.timeoutMs ?? DEFAULTS.timeoutMs;
  const maxBytes = options.maxBytes ?? DEFAULTS.maxBytes;
  const logger = options.logger ?? (() => {});

  const latestHeadBlock = await fetchLatestContenthashBlock({ ensnodeUrl });
  if (latestHeadBlock == null) {
    logInfo(logger, 'No contenthash events found on ENSNode');
    return {
      latestHeadBlock: null,
      head: emptySyncSummary(),
      backfill: emptySyncSummary(),
      headCursorBlockInclusive: store.getHeadCursorBlockInclusive(),
      backfillCursorBlockExclusive: store.getBackfillCursorBlockExclusive(),
    };
  }

  const existingHeadCursor = store.getHeadCursorBlockInclusive();
  let backfillCursorBlockExclusive = store.getBackfillCursorBlockExclusive();
  const backfillCursorWasMissing = backfillCursorBlockExclusive == null;

  const headBaseBlock = existingHeadCursor == null
    ? latestHeadBlock
    : Math.min(existingHeadCursor, latestHeadBlock);
  const headStartInclusive = Math.max(0, headBaseBlock - headReplayBlocks);

  logInfo(
    logger,
    `Head sync from block ${headStartInclusive} to ${latestHeadBlock} (cursor=${existingHeadCursor ?? 'none'}, replay=${headReplayBlocks})`,
  );

  const headEvents = await fetchContenthashEventsSince({
    ensnodeUrl,
    fromBlockInclusive: headStartInclusive,
    first: eventBatchSize,
  });
  const headSummary = await processEventSet({
    store,
    ensnodeUrl,
    events: headEvents,
    probeConcurrency,
    timeoutMs,
    maxBytes,
    onlyUnknownNodes: false,
  });

  logInfo(
    logger,
    `Head sync done: events=${headSummary.scannedEvents} blocks=${headSummary.scannedBlocks} names=${headSummary.currentNames} upserted=${headSummary.upserted} probed=${headSummary.probed}`,
  );

  store.setHeadCursorBlockInclusive(latestHeadBlock);

  if (backfillCursorWasMissing) {
    backfillCursorBlockExclusive = headStartInclusive;
    store.setBackfillCursorBlockExclusive(backfillCursorBlockExclusive);
    logInfo(logger, `Initialized backfill cursor to ${backfillCursorBlockExclusive}`);
  }
  await store.flush?.();

  logInfo(logger, `Backfill sync starting from cursor ${backfillCursorBlockExclusive} with target ${backfillLimit} names`);

  const backfillSummary = await runBackfillSync({
    store,
    ensnodeUrl,
    backfillLimit,
    eventBatchSize,
    probeConcurrency,
    timeoutMs,
    maxBytes,
    cursorBlockExclusive: backfillCursorBlockExclusive,
    logger,
  });

  logInfo(
    logger,
    `Backfill sync done: names=${backfillSummary.processedCurrentNames} events=${backfillSummary.scannedEvents} blocks=${backfillSummary.scannedBlocks} next_cursor=${backfillSummary.cursorBlockExclusive}`,
  );

  return {
    latestHeadBlock,
    head: {
      ...headSummary,
      startBlockInclusive: headStartInclusive,
      latestProcessedBlockInclusive: latestHeadBlock,
    },
    backfill: backfillSummary,
    headCursorBlockInclusive: latestHeadBlock,
    backfillCursorBlockExclusive: backfillSummary.cursorBlockExclusive,
  };
}

async function runBackfillSync({
  store,
  ensnodeUrl,
  backfillLimit,
  eventBatchSize,
  probeConcurrency,
  timeoutMs,
  maxBytes,
  cursorBlockExclusive,
  logger,
}) {
  let processedCurrentNames = 0;
  let scannedEvents = 0;
  let scannedBlocks = 0;
  let currentCursor = cursorBlockExclusive;
  const seenNodesThisRun = new Set();

  while (processedCurrentNames < backfillLimit) {
    const page = await fetchContenthashEventPage({
      ensnodeUrl,
      cursorBlockExclusive: currentCursor,
      first: eventBatchSize,
    });

    if (page.length === 0) {
      break;
    }

    const boundaryBlock = Math.min(...page.map((event) => Number(event.blockNumber)));
    logInfo(logger, `Backfill batch: page=${page.length} boundary_block=${boundaryBlock}`);
    const boundaryEvents = await fetchAllContenthashEventsForBlock({
      ensnodeUrl,
      blockNumber: boundaryBlock,
      first: eventBatchSize,
    });
    const olderEvents = page.filter((event) => Number(event.blockNumber) > boundaryBlock);
    const combinedEvents = [...olderEvents, ...boundaryEvents];

    scannedEvents += combinedEvents.length;
    scannedBlocks += 1;
    currentCursor = boundaryBlock;

    const latestEventByNode = buildLatestEventByNode(combinedEvents, seenNodesThisRun);
    const candidateNodes = [...latestEventByNode.keys()];
    for (const node of candidateNodes) {
      seenNodesThisRun.add(node);
    }

    const existingNodes = store.getExistingNodes(candidateNodes);
    const unseenNodes = candidateNodes.filter((node) => !existingNodes.has(node));
    const currentNames = await hydrateCurrentNameRecords({
      ensnodeUrl,
      candidateNodes: unseenNodes,
      latestEventByNode,
    });
    const applySummary = await applyCurrentNameRecords(store, currentNames, {
      probeConcurrency,
      timeoutMs,
      maxBytes,
    });

    logInfo(
      logger,
      `Backfill batch done: names=${applySummary.currentNames} upserted=${applySummary.upserted} probed=${applySummary.probed} cursor=${currentCursor}`,
    );

    processedCurrentNames += applySummary.currentNames;
    store.setBackfillCursorBlockExclusive(currentCursor);
    await store.flush?.();
  }

  return {
    processedCurrentNames,
    scannedEvents,
    scannedBlocks,
    cursorBlockExclusive: currentCursor,
  };
}

async function processEventSet({
  store,
  ensnodeUrl,
  events,
  probeConcurrency,
  timeoutMs,
  maxBytes,
  onlyUnknownNodes,
}) {
  const latestEventByNode = buildLatestEventByNode(events);
  const candidateNodes = [...latestEventByNode.keys()];
  const existingNodes = onlyUnknownNodes ? store.getExistingNodes(candidateNodes) : null;
  const targetNodes = onlyUnknownNodes
    ? candidateNodes.filter((node) => !existingNodes.has(node))
    : candidateNodes;
  const currentNames = await hydrateCurrentNameRecords({
    ensnodeUrl,
    candidateNodes: targetNodes,
    latestEventByNode,
  });
  const applySummary = await applyCurrentNameRecords(store, currentNames, {
    probeConcurrency,
    timeoutMs,
    maxBytes,
  });

  return {
    scannedEvents: events.length,
    scannedBlocks: countDistinctBlocks(events),
    ...applySummary,
  };
}

function buildLatestEventByNode(events, skipNodes = new Set()) {
  const latestEventByNode = new Map();
  for (const event of events) {
    const node = event?.resolver?.domain?.id;
    if (!node || skipNodes.has(node)) {
      continue;
    }
    const blockNumber = Number(event.blockNumber);
    const previous = latestEventByNode.get(node) ?? null;
    if (!previous || blockNumber > previous.source_block) {
      latestEventByNode.set(node, {
        source_block: blockNumber,
        source_tx_hash: event.transactionID ?? null,
        source_event_id: event.id,
      });
    }
  }
  return latestEventByNode;
}

function countDistinctBlocks(events) {
  return new Set(events.map((event) => Number(event.blockNumber))).size;
}

async function hydrateCurrentNameRecords({ ensnodeUrl, candidateNodes, latestEventByNode }) {
  if (candidateNodes.length === 0) {
    return [];
  }

  const domains = await fetchDomainsByIds({ ensnodeUrl, ids: candidateNodes });
  const domainByNode = new Map(domains.map((domain) => [domain.id, domain]));
  const currentNames = [];

  for (const node of candidateNodes) {
    const domain = domainByNode.get(node);
    if (!domain) {
      continue;
    }
    const currentName = buildCurrentNameRecord(domain, latestEventByNode.get(node));
    if (currentName) {
      currentNames.push(currentName);
    }
  }

  return currentNames;
}

async function applyCurrentNameRecords(store, currentNames, { probeConcurrency, timeoutMs, maxBytes }) {
  const existingRowsByNode = store.getNameRowsByNodes(currentNames.map((row) => row.node));
  let upserted = 0;
  const probeTargets = [];

  for (const currentName of currentNames) {
    const existing = existingRowsByNode.get(currentName.node) ?? null;
    const stateChanged = hasNameStateChanged(existing, currentName);
    store.insertNameVersion(currentName);
    if (!existing || stateChanged) {
      store.upsertName(currentName);
      upserted += 1;
    }
    if (shouldProbe(existing, currentName, stateChanged)) {
      probeTargets.push(currentName);
    }
  }

  await mapLimit(probeTargets, probeConcurrency, async (currentName) => {
    const probe = await probeEthLinkName(currentName.name, { timeoutMs, maxBytes });
    store.insertProbe(currentName, probe);
  });

  return {
    currentNames: currentNames.length,
    upserted,
    probed: probeTargets.length,
  };
}

function hasNameStateChanged(existing, currentName) {
  if (!existing) {
    return true;
  }

  return existing.name !== currentName.name
    || existing.parent_name !== currentName.parent_name
    || Number(existing.is_subdomain) !== Number(currentName.is_subdomain)
    || existing.contenthash_hex !== currentName.contenthash_hex
    || existing.contenthash_protocol !== currentName.contenthash_protocol
    || existing.root_cid !== currentName.root_cid
    || Number(existing.source_block ?? 0) !== Number(currentName.source_block ?? 0)
    || (existing.source_tx_hash ?? null) !== (currentName.source_tx_hash ?? null)
    || (existing.source_event_id ?? null) !== (currentName.source_event_id ?? null);
}

function shouldProbe(existing, currentName, stateChanged) {
  if (!existing) {
    return true;
  }
  if (!existing.last_probe_success || existing.last_probe_status == null) {
    return true;
  }
  if (!stateChanged) {
    return false;
  }

  return existing.name !== currentName.name
    || existing.contenthash_hex !== currentName.contenthash_hex
    || existing.root_cid !== currentName.root_cid;
}

function buildCurrentNameRecord(domain, eventMeta) {
  if (!domain?.id || !domain?.name || !domain?.resolver?.contentHash) {
    return null;
  }
  if (!isMainnetEnsName(domain.name)) {
    return null;
  }

  const decoded = decodeContenthash(domain.resolver.contentHash);
  if ((decoded.protocol !== 'ipfs' && decoded.protocol !== 'ipns') || !decoded.cid) {
    return null;
  }

  return {
    node: domain.id,
    name: domain.name,
    parent_name: domain.parent?.name ?? parentName(domain.name),
    is_subdomain: isSubdomain(domain.name) ? 1 : 0,
    contenthash_hex: domain.resolver.contentHash,
    contenthash_protocol: decoded.protocol,
    root_cid: decoded.cid,
    source_block: eventMeta?.source_block ?? null,
    source_tx_hash: eventMeta?.source_tx_hash ?? null,
    source_event_id: eventMeta?.source_event_id ?? `${domain.id}:${eventMeta?.source_block ?? 'unknown'}`,
    seen_at: new Date().toISOString(),
  };
}

function emptySyncSummary() {
  return {
    scannedEvents: 0,
    scannedBlocks: 0,
    currentNames: 0,
    upserted: 0,
    probed: 0,
  };
}

async function mapLimit(items, limit, worker) {
  const results = new Array(items.length);
  let nextIndex = 0;

  async function runWorker() {
    while (true) {
      const currentIndex = nextIndex;
      nextIndex += 1;
      if (currentIndex >= items.length) {
        return;
      }
      results[currentIndex] = await worker(items[currentIndex], currentIndex);
    }
  }

  const workers = [];
  const workerCount = Math.min(limit, items.length);
  for (let index = 0; index < workerCount; index += 1) {
    workers.push(runWorker());
  }
  await Promise.all(workers);
  return results;
}

function logInfo(logger, message) {
  logger(`[sync-names] ${message}`);
}
