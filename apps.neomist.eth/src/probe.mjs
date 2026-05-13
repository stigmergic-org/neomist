import { create as createKuboRpcClient } from 'kubo-rpc-client';

const TEXT_DECODER = new TextDecoder();
const INDEX_FILENAMES = ['index.html', 'index.htm'];
const MANIFEST_FILENAMES = ['manifest.webmanifest', 'manifest.json', 'site.webmanifest'];
const COMMON_ICON_FILENAMES = ['favicon.svg', 'favicon.png', 'favicon.ico', 'apple-touch-icon.png', 'icon.svg', 'icon.png'];
const MAX_DATA_ICON_URL_LENGTH = 16_384;
const MAX_DATA_MANIFEST_URL_LENGTH = 65_536;
const MAX_MANIFEST_BYTES = 65_536;
const WEB_URL_PROTOCOLS = new Set(['http:', 'https:', 'ipfs:', 'ipns:']);
const EXTENSION_CONTENT_TYPES = new Map([
  ['.css', 'text/css'],
  ['.gif', 'image/gif'],
  ['.htm', 'text/html'],
  ['.html', 'text/html'],
  ['.ico', 'image/x-icon'],
  ['.jpeg', 'image/jpeg'],
  ['.jpg', 'image/jpeg'],
  ['.js', 'text/javascript'],
  ['.json', 'application/json'],
  ['.mjs', 'text/javascript'],
  ['.pdf', 'application/pdf'],
  ['.png', 'image/png'],
  ['.svg', 'image/svg+xml'],
  ['.txt', 'text/plain; charset=utf-8'],
  ['.wasm', 'application/wasm'],
  ['.webmanifest', 'application/manifest+json'],
  ['.webp', 'image/webp'],
  ['.xml', 'application/xml'],
]);

export function createKuboProbeClient({ kuboRpcUrl } = {}) {
  return kuboRpcUrl ? createKuboRpcClient(kuboRpcUrl) : createKuboRpcClient();
}

export async function probeKuboName(nameRecord, { timeoutMs, maxBytes, kuboClient }) {
  const contentUrl = buildContenthashUrl(nameRecord.contenthash_protocol, nameRecord.root_cid);
  const kuboPath = buildKuboPath(nameRecord.contenthash_protocol, nameRecord.root_cid);
  if (!contentUrl || !kuboPath) {
    return failedProbe({
      name: nameRecord.name,
      contentUrl: null,
      fetchError: 'name contenthash could not be converted into kubo path',
    });
  }

  const client = kuboClient ?? createKuboProbeClient();
  let stat;
  try {
    stat = await client.files.stat(kuboPath, { timeout: timeoutMs });
  } catch (error) {
    return failedProbe({
      name: nameRecord.name,
      contentUrl,
      fetchError: describeError(error),
    });
  }

  const bodyData = await readProbeBody(client, kuboPath, stat, { timeoutMs, maxBytes });
  if (bodyData.error && stat.type !== 'directory') {
    return failedProbe({
      name: nameRecord.name,
      contentUrl,
      fetchError: bodyData.error,
      bodyBytes: bodyData.bodyBytes,
    });
  }

  const html = bodyData.buffer.length > 0 ? TEXT_DECODER.decode(bodyData.buffer) : '';
  const title = extractHtmlTitle(html);
  const manifestUrl = await findManifestUrl({
    client,
    html,
    contentUrl,
    kuboPath,
    rootIsDirectory: stat.type === 'directory',
    timeoutMs,
  });
  const iconUrl = await findIconUrl({
    client,
    html,
    contentUrl,
    kuboPath,
    rootIsDirectory: stat.type === 'directory',
    manifestUrl,
    timeoutMs,
  });

  return {
    name: nameRecord.name,
    ethLinkUrl: contentUrl,
    success: true,
    probedAt: new Date().toISOString(),
    httpStatus: 200,
    contentType: bodyData.contentType,
    contentLength: statSize(stat),
    locationHeader: null,
    xIpfsPath: bodyData.path ?? kuboPath,
    xIpfsRoots: [nameRecord.root_cid],
    title,
    iconUrl,
    manifestUrl,
    fetchError: null,
    bodyBytes: bodyData.bodyBytes,
  };
}

function failedProbe({ name, contentUrl, fetchError, bodyBytes = 0 }) {
  return {
    name,
    ethLinkUrl: contentUrl,
    success: false,
    probedAt: new Date().toISOString(),
    httpStatus: null,
    contentType: null,
    contentLength: null,
    locationHeader: null,
    xIpfsPath: null,
    xIpfsRoots: [],
    title: null,
    iconUrl: null,
    manifestUrl: null,
    fetchError,
    bodyBytes,
  };
}

function buildContenthashUrl(protocol, rootCid) {
  if (!isSupportedProtocol(protocol) || !rootCid) {
    return null;
  }
  return `${protocol}://${rootCid}/`;
}

function buildKuboPath(protocol, rootCid) {
  if (!isSupportedProtocol(protocol) || !rootCid) {
    return null;
  }
  return `/${protocol}/${rootCid}`;
}

function isSupportedProtocol(protocol) {
  return protocol === 'ipfs' || protocol === 'ipns';
}

async function readProbeBody(client, kuboPath, stat, { timeoutMs, maxBytes }) {
  if (stat.type !== 'directory') {
    return readKuboFileWithCap(client, kuboPath, { timeoutMs, maxBytes });
  }

  for (const fileName of INDEX_FILENAMES) {
    const indexPath = joinKuboPath(kuboPath, fileName);
    const body = await tryReadKuboFileWithCap(client, indexPath, { timeoutMs, maxBytes });
    if (!body.error) {
      return {
        ...body,
        path: indexPath,
      };
    }
  }

  return {
    buffer: Buffer.alloc(0),
    bodyBytes: 0,
    path: kuboPath,
    contentType: null,
    error: null,
  };
}

async function tryReadKuboFileWithCap(client, kuboPath, options) {
  try {
    return await readKuboFileWithCap(client, kuboPath, options);
  } catch (error) {
    return {
      buffer: Buffer.alloc(0),
      bodyBytes: 0,
      path: kuboPath,
      contentType: null,
      error: describeError(error),
    };
  }
}

async function readKuboFileWithCap(client, kuboPath, { timeoutMs, maxBytes }) {
  const chunks = [];
  let bodyBytes = 0;

  for await (const value of client.cat(kuboPath, { length: maxBytes, timeout: timeoutMs })) {
    const chunk = Buffer.from(value);
    const remaining = maxBytes - Math.min(bodyBytes, maxBytes);
    bodyBytes += chunk.length;
    if (bodyBytes > maxBytes) {
      if (remaining > 0) {
        chunks.push(chunk.subarray(0, remaining));
      }
      break;
    }
    chunks.push(chunk);
  }

  const buffer = Buffer.concat(chunks);

  return {
    buffer,
    bodyBytes,
    path: kuboPath,
    contentType: inferContentType(kuboPath, buffer),
    error: null,
  };
}

function inferContentType(kuboPath, buffer) {
  const lowerPath = String(kuboPath).toLowerCase();
  for (const [extension, contentType] of EXTENSION_CONTENT_TYPES) {
    if (lowerPath.endsWith(extension)) {
      return contentType;
    }
  }

  return sniffContentType(buffer);
}

function sniffContentType(buffer) {
  if (buffer.length === 0) {
    return null;
  }

  if (hasPrefix(buffer, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) {
    return 'image/png';
  }
  if (hasPrefix(buffer, [0xff, 0xd8, 0xff])) {
    return 'image/jpeg';
  }
  if (hasAsciiPrefix(buffer, 'GIF87a') || hasAsciiPrefix(buffer, 'GIF89a')) {
    return 'image/gif';
  }
  if (hasAsciiPrefix(buffer, '%PDF-')) {
    return 'application/pdf';
  }
  if (buffer.length >= 12 && hasAsciiPrefix(buffer, 'RIFF') && buffer.subarray(8, 12).toString('ascii') === 'WEBP') {
    return 'image/webp';
  }

  const text = TEXT_DECODER.decode(buffer.subarray(0, Math.min(buffer.length, 4096))).replace(/^\uFEFF/u, '').trimStart();
  if (/^[{[]/u.test(text)) {
    return 'application/json';
  }
  if (/^(?:<!doctype\s+html|<html[\s>])/iu.test(text)) {
    return 'text/html';
  }
  if (/^(?:<svg[\s>]|<\?xml[\s\S]{0,512}<svg[\s>])/iu.test(text)) {
    return 'image/svg+xml';
  }
  if (/^<\?xml/iu.test(text)) {
    return 'application/xml';
  }
  if (isLikelyUtf8Text(buffer)) {
    return 'text/plain; charset=utf-8';
  }

  return null;
}

function hasPrefix(buffer, bytes) {
  if (buffer.length < bytes.length) {
    return false;
  }
  return bytes.every((byte, index) => buffer[index] === byte);
}

function hasAsciiPrefix(buffer, value) {
  return buffer.length >= value.length && buffer.subarray(0, value.length).toString('ascii') === value;
}

function isLikelyUtf8Text(buffer) {
  const sample = buffer.subarray(0, Math.min(buffer.length, 512));
  for (const byte of sample) {
    if (byte === 0 || (byte < 0x08) || (byte > 0x0d && byte < 0x20)) {
      return false;
    }
  }
  return true;
}

function statSize(stat) {
  if (stat?.type === 'directory' && Number.isFinite(stat?.cumulativeSize)) {
    return stat.cumulativeSize;
  }
  if (Number.isFinite(stat?.size)) {
    return stat.size;
  }
  if (Number.isFinite(stat?.cumulativeSize)) {
    return stat.cumulativeSize;
  }
  return null;
}

function joinKuboPath(basePath, fileName) {
  return `${basePath.replace(/\/+$/u, '')}/${fileName}`;
}

function describeError(error) {
  if (!(error instanceof Error)) {
    return String(error);
  }
  const code = typeof error.code === 'string' ? error.code : null;
  if (code) {
    return `${error.message}: ${code}`;
  }
  if (error.cause && typeof error.cause === 'object') {
    const causeCode = typeof error.cause.code === 'string' ? error.cause.code : null;
    const causeMessage = typeof error.cause.message === 'string' ? error.cause.message : null;
    if (causeCode && causeMessage) {
      return `${error.message}: ${causeCode} ${causeMessage}`;
    }
    if (causeMessage) {
      return `${error.message}: ${causeMessage}`;
    }
  }
  return error.message;
}

function extractHtmlTitle(html) {
  return html ? firstMatch(html, /<title[^>]*>([\s\S]*?)<\/title>/i) : null;
}

async function findIconUrl({ client, html, contentUrl, kuboPath, rootIsDirectory, manifestUrl, timeoutMs }) {
  const linkedIconUrl = extractBestLinkUrl(html, contentUrl, (relTokens) => relTokens.some((token) => token.includes('icon')), {
    allowData: true,
    maxDataUrlLength: MAX_DATA_ICON_URL_LENGTH,
    dataMediaPattern: /^data:image\//iu,
    scoreCandidate: scoreLinkIconCandidate,
  });
  if (linkedIconUrl) {
    return linkedIconUrl;
  }

  const manifestIconUrl = await findManifestIconUrl({ client, manifestUrl, timeoutMs });
  if (manifestIconUrl) {
    return manifestIconUrl;
  }

  return findCommonIconUrl({ client, contentUrl, kuboPath, rootIsDirectory, timeoutMs });
}

async function findManifestUrl({ client, html, contentUrl, kuboPath, rootIsDirectory, timeoutMs }) {
  const linkedManifestUrl = extractBestLinkUrl(html, contentUrl, (relTokens) => relTokens.includes('manifest'), {
    allowData: true,
    maxDataUrlLength: MAX_DATA_MANIFEST_URL_LENGTH,
    dataMediaPattern: /^data:(?:application\/(?:manifest\+json|json)|text\/json)(?:[;,]|$)/iu,
  });
  if (linkedManifestUrl) {
    return linkedManifestUrl;
  }

  if (!rootIsDirectory) {
    return null;
  }

  for (const fileName of MANIFEST_FILENAMES) {
    const manifestPath = joinKuboPath(kuboPath, fileName);
    try {
      await client.files.stat(manifestPath, { timeout: timeoutMs });
      return new URL(fileName, contentUrl).toString();
    } catch {
      continue;
    }
  }

  return null;
}

async function findManifestIconUrl({ client, manifestUrl, timeoutMs }) {
  const manifest = await readManifestJson(client, manifestUrl, timeoutMs);
  if (!manifest || !Array.isArray(manifest.icons)) {
    return null;
  }

  const candidates = [];
  for (const icon of manifest.icons) {
    if (!icon || typeof icon !== 'object') {
      continue;
    }
    const url = resolveMetadataUrl(icon.src, manifestUrl, {
      allowData: true,
      maxDataUrlLength: MAX_DATA_ICON_URL_LENGTH,
      dataMediaPattern: /^data:image\//iu,
    });
    if (!url) {
      continue;
    }
    candidates.push({
      url,
      score: scoreManifestIconCandidate(icon, url),
      index: candidates.length,
    });
  }

  return bestCandidateUrl(candidates);
}

async function findCommonIconUrl({ client, contentUrl, kuboPath, rootIsDirectory, timeoutMs }) {
  if (!rootIsDirectory) {
    return null;
  }

  for (const fileName of COMMON_ICON_FILENAMES) {
    const iconPath = joinKuboPath(kuboPath, fileName);
    try {
      await client.files.stat(iconPath, { timeout: timeoutMs });
      return new URL(fileName, contentUrl).toString();
    } catch {
      continue;
    }
  }

  return null;
}

async function readManifestJson(client, manifestUrl, timeoutMs) {
  if (!manifestUrl) {
    return null;
  }

  try {
    if (/^data:/iu.test(manifestUrl)) {
      return parseJsonDataUrl(manifestUrl);
    }

    const kuboPath = kuboPathFromIpfsUrl(manifestUrl);
    if (!kuboPath) {
      return null;
    }
    const body = await readKuboFileWithCap(client, kuboPath, { timeoutMs, maxBytes: MAX_MANIFEST_BYTES });
    return parseJsonText(TEXT_DECODER.decode(body.buffer));
  } catch {
    return null;
  }
}

function parseJsonDataUrl(dataUrl) {
  if (dataUrl.length > MAX_DATA_MANIFEST_URL_LENGTH) {
    return null;
  }

  const commaIndex = dataUrl.indexOf(',');
  if (commaIndex === -1) {
    return null;
  }
  const metadata = dataUrl.slice(0, commaIndex).toLowerCase();
  const body = dataUrl.slice(commaIndex + 1);
  const text = metadata.includes(';base64')
    ? Buffer.from(body, 'base64').toString('utf8')
    : decodeURIComponent(body);
  return parseJsonText(text);
}

function parseJsonText(text) {
  return JSON.parse(text.replace(/^\uFEFF/u, ''));
}

function kuboPathFromIpfsUrl(value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    return null;
  }

  const protocol = url.protocol === 'ipns:' ? 'ipns' : (url.protocol === 'ipfs:' ? 'ipfs' : null);
  if (!protocol) {
    return null;
  }

  const root = url.hostname;
  if (!root) {
    return null;
  }
  const suffix = url.pathname === '/' ? '' : url.pathname;
  return `/${protocol}/${root}${suffix}`;
}

function extractBestLinkUrl(html, baseUrl, relPredicate, options = {}) {
  const candidates = [];
  for (const tag of extractHtmlTags(html, 'link')) {
    const attrs = parseHtmlAttributes(tag);
    const relTokens = String(attrs.rel ?? '').toLowerCase().split(/\s+/u).filter(Boolean);
    if (!attrs.href || !relPredicate(relTokens)) {
      continue;
    }

    const url = resolveMetadataUrl(attrs.href, baseUrl, options);
    if (!url) {
      continue;
    }

    candidates.push({
      url,
      score: options.scoreCandidate?.({ attrs, relTokens, url }) ?? 0,
      index: candidates.length,
    });
  }
  return bestCandidateUrl(candidates);
}

function bestCandidateUrl(candidates) {
  return candidates
    .sort((left, right) => (right.score - left.score) || (left.index - right.index))[0]
    ?.url ?? null;
}

function resolveMetadataUrl(rawUrl, baseUrl, options = {}) {
  const value = decodeHtmlEntities(String(rawUrl ?? '')).trim();
  if (!value) {
    return null;
  }

  if (/^data:/iu.test(value)) {
    if (!options.allowData || value.length > (options.maxDataUrlLength ?? 0)) {
      return null;
    }
    if (options.dataMediaPattern && !options.dataMediaPattern.test(value)) {
      return null;
    }
    return value;
  }

  try {
    const resolved = new URL(value, baseUrl);
    return WEB_URL_PROTOCOLS.has(resolved.protocol) ? resolved.toString() : null;
  } catch {
    return null;
  }
}

function scoreLinkIconCandidate({ attrs, relTokens, url }) {
  let score = 0;
  if (relTokens.includes('icon')) {
    score += 80;
  }
  if (relTokens.some((token) => token === 'shortcut')) {
    score += 4;
  }
  if (relTokens.some((token) => token.startsWith('apple-touch-icon'))) {
    score += 45;
  }
  if (relTokens.includes('mask-icon')) {
    score += 20;
  }
  return score + scoreIconSize(attrs.sizes) + scoreIconType(attrs.type, url);
}

function scoreManifestIconCandidate(icon, url) {
  let score = scoreIconSize(icon.sizes) + scoreIconType(icon.type, url);
  const purpose = String(icon.purpose ?? '').toLowerCase();
  if (!purpose || purpose.split(/\s+/u).includes('any')) {
    score += 8;
  }
  return score;
}

function scoreIconSize(sizes) {
  const parsedSizes = String(sizes ?? '')
    .toLowerCase()
    .split(/\s+/u)
    .map((size) => size.match(/^(\d+)x(\d+)$/u))
    .filter(Boolean)
    .map((match) => Math.max(Number(match[1]), Number(match[2])));
  if (parsedSizes.length === 0) {
    return 10;
  }

  const bestDistance = Math.min(...parsedSizes.map((size) => Math.abs(size - 32)));
  return Math.max(0, 35 - bestDistance);
}

function scoreIconType(type, url) {
  const value = `${type ?? ''} ${url}`.toLowerCase();
  if (value.includes('image/svg')) {
    return 16;
  }
  if (value.includes('image/png') || value.endsWith('.png')) {
    return 12;
  }
  if (value.includes('image/x-icon') || value.includes('image/vnd.microsoft.icon') || value.endsWith('.ico')) {
    return 10;
  }
  if (value.includes('image/webp') || value.endsWith('.webp')) {
    return 8;
  }
  return 0;
}

function extractHtmlTags(html, tagName) {
  if (!html) {
    return [];
  }

  const tags = [];
  const regex = new RegExp(`<${tagName}\\b`, 'giu');
  let match;
  while ((match = regex.exec(html))) {
    let quote = null;
    let index = regex.lastIndex;
    for (; index < html.length; index += 1) {
      const char = html[index];
      if (quote) {
        if (char === quote) {
          quote = null;
        }
      } else if (char === '"' || char === "'") {
        quote = char;
      } else if (char === '>') {
        tags.push(html.slice(match.index, index + 1));
        regex.lastIndex = index + 1;
        break;
      }
    }
    if (index >= html.length) {
      break;
    }
  }
  return tags;
}

function parseHtmlAttributes(tag) {
  const attrs = {};
  const matches = tag.matchAll(/([a-zA-Z_:][-a-zA-Z0-9_:.]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))/g);
  for (const match of matches) {
    attrs[match[1].toLowerCase()] = match[2] ?? match[3] ?? match[4] ?? '';
  }
  return attrs;
}

function firstMatch(value, regex) {
  const match = value.match(regex);
  if (!match || match.length < 2) {
    return null;
  }
  return collapseWhitespace(decodeHtmlEntities(match[1].replace(/<[^>]+>/g, ' ')));
}

function collapseWhitespace(value) {
  return value.replace(/\s+/g, ' ').trim();
}

function decodeHtmlEntities(value) {
  return value
    .replace(/&nbsp;/gi, ' ')
    .replace(/&amp;/gi, '&')
    .replace(/&quot;/gi, '"')
    .replace(/&#39;/gi, "'")
    .replace(/&apos;/gi, "'")
    .replace(/&lt;/gi, '<')
    .replace(/&gt;/gi, '>');
}
