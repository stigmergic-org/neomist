import { create as createKuboRpcClient } from 'kubo-rpc-client';

const TEXT_DECODER = new TextDecoder();
const INDEX_FILENAMES = ['index.html', 'index.htm'];

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

  const htmlInfo = extractBasicHtmlInfo(
    bodyData.buffer.length > 0 ? TEXT_DECODER.decode(bodyData.buffer) : '',
    contentUrl,
  );
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

  return {
    buffer: Buffer.concat(chunks),
    bodyBytes,
    path: kuboPath,
    contentType: inferContentType(kuboPath),
    error: null,
  };
}

function inferContentType(kuboPath) {
  return /\.html?$/iu.test(kuboPath) ? 'text/html' : null;
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
  const iconUrl = extractIconUrl(html, baseUrl);
  return { title, iconUrl };
}

function extractIconUrl(html, baseUrl) {
  const matches = [...html.matchAll(/<link\b[^>]*rel=(?:"([^"]*)"|'([^']*)'|([^\s>]+))[^>]*href=(?:"([^"]*)"|'([^']*)'|([^\s>]+))[^>]*>/gi)];
  for (const match of matches) {
    const rel = (match[1] || match[2] || match[3] || '').toLowerCase();
    const href = match[4] || match[5] || match[6] || '';
    if (!rel.includes('icon') || !href) {
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
