import { execFile } from 'node:child_process';
import { access, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { promisify } from 'node:util';
import { Contract, JsonRpcProvider, Wallet, formatUnits, namehash, parseUnits } from 'ethers';
import { CID } from 'multiformats/cid';
import { PATHS } from './config.mjs';

const DEFAULT_ENS_NAME = 'apps.neomist.eth';
const DEFAULT_ETH_RPC_URL = 'https://ethereum-rpc.publicnode.com';
const PRIVATE_KEY_ENV_KEYS = ['APPS_NEOMIST_ENS_PRIVATE_KEY', 'ENS_PRIVATE_KEY', 'PRIVATE_KEY'];
const ETH_RPC_ENV_KEYS = ['APPS_NEOMIST_ETH_RPC_URL', 'ETH_RPC_URL', 'MAINNET_RPC_URL', 'RPC_URL'];
const KUBO_RPC_ENV_KEYS = ['APPS_NEOMIST_KUBO_RPC_URL', 'KUBO_RPC_URL', 'IPFS_API'];
const IPFS_NS_CODEC = 0xe3;
const DEFAULT_MAX_GAS_PRICE_MWEI = '400';
const DEFAULT_PUBLISH_COOLDOWN_DAYS = 3;
const DAY_MS = 24 * 60 * 60 * 1000;
const DEFAULT_PUBLISH_STATE_PATH = path.join(PATHS.stateDir, 'ens-publish.json');
const execFileAsync = promisify(execFile);

const PUBLIC_RESOLVER_ABI = [
  'function contenthash(bytes32 node) view returns (bytes)',
  'function setContenthash(bytes32 node, bytes hash)',
];

export async function publishIpfsRootToEns(options = {}) {
  await loadDotEnvFiles(options.envPath);

  const name = options.name ?? process.env.APPS_NEOMIST_ENS_NAME ?? DEFAULT_ENS_NAME;
  const rpcUrl = options.rpcUrl ?? firstEnv(ETH_RPC_ENV_KEYS) ?? DEFAULT_ETH_RPC_URL;
  const kuboRpcUrl = options.kuboRpcUrl ?? firstEnv(KUBO_RPC_ENV_KEYS);
  const privateKey = normalizeOptionalPrivateKey(options.privateKey ?? firstEnv(PRIVATE_KEY_ENV_KEYS));
  const maxGasPriceWei = parseMweiOption(options.maxGasPriceMwei ?? DEFAULT_MAX_GAS_PRICE_MWEI, 'maxGasPriceMwei');
  const publishCooldownDays = parsePositiveNumberOption(options.publishCooldownDays ?? DEFAULT_PUBLISH_COOLDOWN_DAYS, 'publishCooldownDays');
  const publishCooldownMs = publishCooldownDays * DAY_MS;
  const signer = privateKey ? new Wallet(privateKey) : null;
  options.logger?.(signer ? `signer ${signer.address}` : 'signer not configured');

  const rootCid = options.rootCid ?? await addIpfsRoot({
    kuboRpcUrl,
    onlyHash: true,
    pin: options.pin !== false,
    logger: options.logger,
  });
  const contenthash = encodeIpfsContenthash(rootCid);

  const provider = new JsonRpcProvider(rpcUrl);
  const network = await provider.getNetwork();
  if (network.chainId !== 1n) {
    throw new Error(`expected Ethereum mainnet RPC, got chainId ${network.chainId}`);
  }

  const resolver = await provider.getResolver(name);
  if (!resolver) {
    throw new Error(`ENS name has no resolver: ${name}`);
  }

  const node = namehash(name);
  const readResolverContract = new Contract(resolver.address, PUBLIC_RESOLVER_ABI, provider);
  const currentContenthash = await readCurrentContenthash(readResolverContract, node);
  const alreadyCurrent = currentContenthash?.toLowerCase() === contenthash.toLowerCase();
  const publishConditions = await getPublishConditions({
    provider,
    maxGasPriceWei,
    publishCooldownDays,
    publishCooldownMs,
    statePath: options.publishStatePath ?? DEFAULT_PUBLISH_STATE_PATH,
  });
  logPublishConditions(publishConditions, options.logger);
  const skipReasons = buildSkipReasons({ alreadyCurrent, publishConditions });

  const summary = {
    name,
    rootCid,
    contentUrl: `ipfs://${rootCid}/`,
    contenthash,
    resolver: resolver.address,
    signer: signer?.address ?? null,
    currentContenthash,
    alreadyCurrent,
    publishConditions,
  };

  if (options.dryRun || skipReasons.length > 0) {
    return {
      ...summary,
      dryRun: Boolean(options.dryRun),
      publishAllowed: skipReasons.length === 0,
      skipped: !options.dryRun && skipReasons.length > 0,
      skipReasons,
    };
  }

  if (!privateKey) {
    throw new Error(`missing private key env var (${PRIVATE_KEY_ENV_KEYS.join(', ')})`);
  }

  const wallet = signer ? signer.connect(provider) : null;

  if (!options.rootCid) {
    const addedRootCid = await addIpfsRoot({
      kuboRpcUrl,
      onlyHash: false,
      pin: options.pin !== false,
      logger: options.logger,
    });
    if (addedRootCid !== rootCid) {
      throw new Error(`ipfs-root changed while publishing: hashed ${rootCid}, added ${addedRootCid}`);
    }
  }

  const resolverContract = new Contract(resolver.address, PUBLIC_RESOLVER_ABI, wallet);
  const tx = await resolverContract.setContenthash(node, contenthash);
  const receipt = await tx.wait();
  const result = {
    ...summary,
    previousContenthash: currentContenthash,
    transactionHash: receipt.hash,
    blockNumber: receipt.blockNumber,
    publishedAt: new Date().toISOString(),
  };
  await writePublishState(publishConditions.publishStatePath, result);
  return result;
}

async function addIpfsRoot({ kuboRpcUrl, onlyHash, pin, logger }) {
  await assertDirectoryExists(PATHS.ipfsRootDir);
  logger?.(`${onlyHash ? 'hashing' : 'adding'} ${PATHS.ipfsRootDir} with ipfs CLI`);

  const args = [
    'add',
    '--quieter',
    '--recursive',
    '--cid-version=1',
    '--raw-leaves',
  ];

  const apiMultiaddr = kuboRpcUrl ? kuboRpcUrlToApiMultiaddr(kuboRpcUrl) : null;
  if (apiMultiaddr && !onlyHash) {
    args.unshift(`--api=${apiMultiaddr}`);
  }
  if (onlyHash) {
    args.push('--only-hash');
  } else if (!pin) {
    args.push('--pin=false');
  }
  args.push(PATHS.ipfsRootDir);

  try {
    const { stdout } = await execFileAsync('ipfs', args, {
      maxBuffer: 1024 * 1024,
    });
    const rootCid = stdout.trim().split(/\s+/).at(-1);
    if (!rootCid) {
      throw new Error('ipfs add did not print root CID');
    }
    return rootCid;
  } catch (error) {
    throw new Error(`failed to ${onlyHash ? 'hash' : 'add'} ipfs-root with ipfs CLI: ${describeExecError(error)}`);
  }
}

function encodeIpfsContenthash(rootCid) {
  const cid = CID.parse(rootCid);
  const cidBytes = cid.version === 0 ? cid.toV1().bytes : cid.bytes;
  return `0x${Buffer.concat([encodeUnsignedVarint(IPFS_NS_CODEC), Buffer.from(cidBytes)]).toString('hex')}`;
}

function encodeUnsignedVarint(value) {
  const bytes = [];
  let remaining = value;
  while (remaining >= 0x80) {
    bytes.push((remaining & 0x7f) | 0x80);
    remaining >>= 7;
  }
  bytes.push(remaining);
  return Buffer.from(bytes);
}

async function readCurrentContenthash(contract, node) {
  try {
    const value = await contract.contenthash(node);
    return value === '0x' ? null : value;
  } catch {
    return null;
  }
}

async function getPublishConditions({ provider, maxGasPriceWei, publishCooldownDays, publishCooldownMs, statePath }) {
  const [feeData, lastLocalPublish] = await Promise.all([
    provider.getFeeData(),
    readPublishState(statePath),
  ]);
  const gasPriceWei = feeData.gasPrice ?? feeData.maxFeePerGas ?? null;
  const gasPriceBelowMax = gasPriceWei != null && gasPriceWei < maxGasPriceWei;
  const latestPublish = isWithinCooldown(lastLocalPublish?.publishedAt, publishCooldownMs) ? lastLocalPublish : null;

  return {
    publishStatePath: statePath,
    maxGasPriceMwei: formatWeiAsMwei(maxGasPriceWei),
    gasPriceMwei: gasPriceWei == null ? null : formatWeiAsMwei(gasPriceWei),
    gasPriceWei: gasPriceWei?.toString() ?? null,
    gasPriceBelowMax,
    publishCooldownDays,
    lastLocalPublish,
    latestPublish,
    notPublishedRecently: latestPublish == null,
  };
}

function buildSkipReasons({ alreadyCurrent, publishConditions }) {
  const reasons = [];
  if (alreadyCurrent) {
    reasons.push('contenthash already current');
  }
  if (!publishConditions.gasPriceBelowMax) {
    reasons.push(`gas price ${publishConditions.gasPriceMwei ?? 'unknown'} Mwei >= ${publishConditions.maxGasPriceMwei} Mwei`);
  }
  if (!publishConditions.notPublishedRecently) {
    reasons.push(`contenthash published within last ${publishConditions.publishCooldownDays} days`);
  }
  return reasons;
}

function logPublishConditions(conditions, logger) {
  if (!logger) {
    return;
  }
  logger(`gas price ${conditions.gasPriceMwei ?? 'unknown'} Mwei (max ${conditions.maxGasPriceMwei} Mwei)`);
  if (conditions.latestPublish) {
    logger(`local contenthash publish ${conditions.latestPublish.publishedAt} ${conditions.latestPublish.transactionHash}`);
  } else {
    logger(`no local contenthash publish in last ${conditions.publishCooldownDays} days`);
  }
}

async function readPublishState(statePath) {
  let source;
  try {
    source = await readFile(statePath, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') {
      return null;
    }
    throw error;
  }

  try {
    const state = JSON.parse(source);
    return state && typeof state === 'object' ? state : null;
  } catch (error) {
    throw new Error(`invalid ENS publish state file ${statePath}: ${describeError(error)}`);
  }
}

async function writePublishState(statePath, state) {
  await mkdir(path.dirname(statePath), { recursive: true });
  await writeFile(statePath, `${JSON.stringify({
    name: state.name,
    rootCid: state.rootCid,
    contentUrl: state.contentUrl,
    contenthash: state.contenthash,
    resolver: state.resolver,
    signer: state.signer,
    transactionHash: state.transactionHash,
    blockNumber: state.blockNumber,
    publishedAt: state.publishedAt,
  }, null, 2)}\n`);
}

function isWithinCooldown(publishedAt, publishCooldownMs) {
  const timestamp = Date.parse(String(publishedAt ?? ''));
  return Number.isFinite(timestamp) && Date.now() - timestamp < publishCooldownMs;
}

function formatWeiAsMwei(value) {
  return trimTrailingDecimalZeros(formatUnits(value, 6));
}

function trimTrailingDecimalZeros(value) {
  return value.includes('.') ? value.replace(/0+$/, '').replace(/\.$/, '') : value;
}

function parseMweiOption(value, label) {
  const text = String(value ?? '').trim();
  if (!/^\d+(?:\.\d{1,6})?$/.test(text)) {
    throw new Error(`${label} must be positive Mwei value`);
  }
  const wei = parseUnits(text, 6);
  if (wei <= 0n) {
    throw new Error(`${label} must be greater than zero`);
  }
  return wei;
}

function parsePositiveNumberOption(value, label) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    throw new Error(`${label} must be positive number`);
  }
  return parsed;
}

function normalizeOptionalPrivateKey(value) {
  const privateKey = String(value ?? '').trim();
  if (!privateKey) {
    return null;
  }
  const normalized = privateKey.startsWith('0x') ? privateKey : `0x${privateKey}`;
  if (!/^0x[0-9a-fA-F]{64}$/.test(normalized)) {
    throw new Error('private key must be 32-byte hex');
  }
  return normalized;
}

async function loadDotEnvFiles(explicitPath) {
  const envPaths = uniquePaths([
    explicitPath,
    path.join(PATHS.packageRoot, '.env'),
    path.resolve(process.cwd(), '.env'),
    path.resolve(PATHS.packageRoot, '..', '.env'),
  ].filter(Boolean));

  for (const envPath of envPaths) {
    await loadDotEnvFile(envPath);
  }
}

async function loadDotEnvFile(envPath) {
  let source;
  try {
    source = await readFile(envPath, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') {
      return;
    }
    throw error;
  }

  for (const [key, value] of parseDotEnv(source)) {
    if (process.env[key] == null) {
      process.env[key] = value;
    }
  }
}

function parseDotEnv(source) {
  const entries = [];
  for (const rawLine of source.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) {
      continue;
    }

    const match = /^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/.exec(line);
    if (!match) {
      continue;
    }

    entries.push([match[1], parseDotEnvValue(match[2])]);
  }
  return entries;
}

function parseDotEnvValue(rawValue) {
  const value = rawValue.trim();
  if (value.startsWith('"') && value.endsWith('"')) {
    return value.slice(1, -1).replace(/\\n/g, '\n').replace(/\\r/g, '\r').replace(/\\t/g, '\t');
  }
  if (value.startsWith("'") && value.endsWith("'")) {
    return value.slice(1, -1);
  }
  return value.replace(/\s+#.*$/, '').trim();
}

function firstEnv(keys) {
  for (const key of keys) {
    const value = process.env[key];
    if (value && value.trim()) {
      return value.trim();
    }
  }
  return undefined;
}

function describeError(error) {
  return error instanceof Error ? error.message : String(error);
}

function describeExecError(error) {
  const stderr = typeof error?.stderr === 'string' ? error.stderr.trim() : '';
  return stderr || describeError(error);
}

function kuboRpcUrlToApiMultiaddr(value) {
  const raw = String(value).trim();
  if (!raw) {
    return null;
  }
  if (raw.startsWith('/')) {
    return raw;
  }

  const url = new URL(raw);
  const port = url.port || (url.protocol === 'https:' ? '443' : '80');
  const host = url.hostname;
  const hostPart = isIpv4(host)
    ? `/ip4/${host}`
    : host.includes(':')
      ? `/ip6/${host}`
      : `/dns4/${host}`;
  const tlsPart = url.protocol === 'https:' ? '/https' : '';
  return `${hostPart}/tcp/${port}${tlsPart}`;
}

function isIpv4(value) {
  return /^(?:\d{1,3}\.){3}\d{1,3}$/.test(value);
}

async function assertDirectoryExists(dir) {
  try {
    await access(dir);
  } catch (error) {
    if (error.code === 'ENOENT') {
      throw new Error(`missing ipfs-root export: ${dir}`);
    }
    throw error;
  }
}

function uniquePaths(paths) {
  return [...new Set(paths.map((entry) => path.resolve(entry)))];
}
