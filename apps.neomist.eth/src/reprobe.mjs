import { DEFAULTS } from './config.mjs';
import { createKuboProbeClient, probeKuboName } from './probe.mjs';

const FLUSH_BATCH_SIZE_MIN = 100;
const ERROR_SAMPLE_LIMIT = 20;

export async function reprobeNames(store, options = {}) {
  const onlyMissingIcons = Boolean(options.onlyMissingIcons);
  const limit = options.limit ?? null;
  const probeConcurrency = Math.max(1, options.probeConcurrency ?? DEFAULTS.probeConcurrency);
  const timeoutMs = options.timeoutMs ?? DEFAULTS.timeoutMs;
  const maxBytes = options.maxBytes ?? DEFAULTS.maxBytes;
  const logger = options.logger ?? (() => {});
  const kuboClient = options.kuboClient ?? createKuboProbeClient({
    kuboRpcUrl: options.kuboRpcUrl ?? DEFAULTS.kuboRpcUrl,
  });
  const targets = store.listNamesForReprobe(limit, { onlyMissingIcons });
  const summary = {
    requestedLimit: limit,
    onlyMissingIcons,
    targets: targets.length,
    attempted: 0,
    inserted: 0,
    successful: 0,
    failed: 0,
    iconsFound: 0,
    iconsMissing: 0,
    errors: [],
  };

  const batchSize = Math.max(FLUSH_BATCH_SIZE_MIN, probeConcurrency * 20);
  for (let offset = 0; offset < targets.length; offset += batchSize) {
    const batch = targets.slice(offset, offset + batchSize);
    const results = await mapLimit(batch, probeConcurrency, async (target, batchIndex) => reprobeTarget(store, target, {
      kuboClient,
      index: offset + batchIndex,
      logger,
      timeoutMs,
      total: targets.length,
      maxBytes,
    }));

    for (const result of results) {
      summary.attempted += 1;
      if (result.error) {
        summary.failed += 1;
        addErrorSample(summary.errors, result.target, result.error);
        continue;
      }

      summary.inserted += 1;
      if (result.probe.success) {
        summary.successful += 1;
      } else {
        summary.failed += 1;
      }
      if (result.probe.iconUrl) {
        summary.iconsFound += 1;
      } else {
        summary.iconsMissing += 1;
      }
    }

    await store.flush?.();
    logger(`[reprobe-names] probed ${summary.attempted}/${summary.targets} success=${summary.successful} failed=${summary.failed} icons=${summary.iconsFound}`);
  }

  return summary;
}

async function reprobeTarget(store, target, options) {
  try {
    options.logger(`[reprobe-names] probing ${options.index + 1}/${options.total} ${target.name} ${target.root_cid}`);
    const probe = await probeKuboName(target, options);
    store.insertProbe(target, probe);
    return { target, probe, error: null };
  } catch (error) {
    return { target, probe: null, error: describeError(error) };
  }
}

function addErrorSample(errors, target, error) {
  if (errors.length >= ERROR_SAMPLE_LIMIT) {
    return;
  }
  errors.push({
    name: target.name,
    node: target.node,
    root_cid: target.root_cid,
    error,
  });
}

function describeError(error) {
  return error instanceof Error ? error.message : String(error);
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
