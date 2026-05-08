import { create as createKuboRpcClient } from 'kubo-rpc-client';

const TEXT_DECODER = new TextDecoder();
const INDEX_FILENAMES = ['index.html', 'index.htm'];
const MANIFEST_FILENAMES = ['manifest.webmanifest', 'manifest.json', 'site.webmanifest'];
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
  const htmlInfo = extractBasicHtmlInfo(html, contentUrl);
  const manifestUrl = await findManifestUrl({
    client,
    html,
    contentUrl,
    kuboPath,
    rootIsDirectory: stat.type === 'directory',
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
    title: htmlInfo.title,
    iconUrl: htmlInfo.iconUrl,
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

function extractBasicHtmlInfo(html, baseUrl) {
  if (!html) {
    return {
      title: null,
      iconUrl: null,
    };
  }

  const title = firstMatch(html, /<title[^>]*>([\s\S]*?)<\/title>/i);
  const iconUrl = extractLinkUrl(html, baseUrl, (relTokens) => relTokens.some((token) => token.includes('icon')));
  return { title, iconUrl };
}

async function findManifestUrl({ client, html, contentUrl, kuboPath, rootIsDirectory, timeoutMs }) {
  const linkedManifestUrl = extractLinkUrl(html, contentUrl, (relTokens) => relTokens.includes('manifest'));
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

function extractLinkUrl(html, baseUrl, relPredicate) {
  const matches = html.matchAll(/<link\b[^>]*>/gi);
  for (const match of matches) {
    const attrs = parseHtmlAttributes(match[0]);
    const relTokens = String(attrs.rel ?? '').toLowerCase().split(/\s+/u).filter(Boolean);
    const href = attrs.href ?? '';
    if (!href || !relPredicate(relTokens)) {
      continue;
    }

    try {
      const resolved = new URL(href, baseUrl);
      if (['http:', 'https:', 'ipfs:', 'ipns:'].includes(resolved.protocol)) {
        return resolved.toString();
      }
    } catch {
      continue;
    }
  }
  return null;
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
