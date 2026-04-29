const TEXT_DECODER = new TextDecoder();

export async function probeEthLinkName(name, { timeoutMs, maxBytes }) {
  const ethLinkUrl = buildEthLinkUrl(name);
  if (!ethLinkUrl) {
    return failedProbe({
      name,
      ethLinkUrl: null,
      fetchError: 'name could not be converted into eth.link URL',
    });
  }

  const responseData = await fetchUrlWithCap(ethLinkUrl, { timeoutMs, maxBytes });
  if (responseData.error) {
    return failedProbe({
      name,
      ethLinkUrl,
      fetchError: responseData.error,
      bodyBytes: responseData.bodyBytes,
    });
  }

  const htmlInfo = extractBasicHtmlInfo(responseData.text || '', ethLinkUrl);
  return {
    name,
    ethLinkUrl,
    success: responseData.status === 200,
    probedAt: new Date().toISOString(),
    httpStatus: responseData.status,
    contentType: responseData.contentType,
    contentLength: responseData.contentLength,
    locationHeader: responseData.locationHeader,
    xIpfsPath: responseData.xIpfsPath,
    xIpfsRoots: responseData.xIpfsRoots,
    title: htmlInfo.title,
    iconUrl: htmlInfo.iconUrl,
    fetchError: null,
    bodyBytes: responseData.bodyBytes,
  };
}

function failedProbe({ name, ethLinkUrl, fetchError, bodyBytes = 0 }) {
  return {
    name,
    ethLinkUrl,
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

function buildEthLinkUrl(name) {
  if (String(name).includes('[') || String(name).includes(']') || String(name).includes(' ') || String(name).includes('/')) {
    return null;
  }

  try {
    return new URL(`https://${name}.link/`).toString();
  } catch {
    return null;
  }
}

async function fetchUrlWithCap(url, { timeoutMs, maxBytes }) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);

  try {
    const response = await fetch(url, {
      method: 'GET',
      redirect: 'manual',
      signal: controller.signal,
      headers: {
        accept: 'text/html,application/xhtml+xml;q=0.9,text/plain;q=0.7,*/*;q=0.2',
        'user-agent': 'apps.neomist.eth/1.0',
      },
    });

    const body = await readResponseBody(response, maxBytes);
    return {
      status: response.status,
      contentType: response.headers.get('content-type'),
      contentLength: parseHeaderNumber(response.headers.get('content-length')),
      locationHeader: response.headers.get('location'),
      xIpfsPath: response.headers.get('x-ipfs-path'),
      xIpfsRoots: parseRootsHeader(response.headers.get('x-ipfs-roots')),
      bodyBytes: body.bytesRead,
      text: shouldDecodeText(response.headers.get('content-type')) ? TEXT_DECODER.decode(body.buffer) : null,
      error: null,
    };
  } catch (error) {
    if (error?.name === 'AbortError') {
      return {
        bodyBytes: 0,
        error: `fetch timed out after ${timeoutMs}ms`,
      };
    }
    return {
      bodyBytes: 0,
      error: describeFetchError(error),
    };
  } finally {
    clearTimeout(timer);
  }
}

function describeFetchError(error) {
  if (!(error instanceof Error)) {
    return String(error);
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

async function readResponseBody(response, maxBytes) {
  if (!response.body) {
    return {
      buffer: Buffer.alloc(0),
      bytesRead: 0,
    };
  }

  const reader = response.body.getReader();
  const chunks = [];
  let bytesRead = 0;

  while (true) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }
    const chunk = Buffer.from(value);
    const remaining = maxBytes - Math.min(bytesRead, maxBytes);
    bytesRead += chunk.length;
    if (bytesRead > maxBytes) {
      if (remaining > 0) {
        chunks.push(chunk.subarray(0, remaining));
      }
      await reader.cancel('body exceeded maxBytes');
      break;
    }
    chunks.push(chunk);
  }

  return {
    buffer: Buffer.concat(chunks),
    bytesRead,
  };
}

function parseHeaderNumber(value) {
  if (!value) {
    return null;
  }
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) ? parsed : null;
}

function parseRootsHeader(value) {
  if (!value) {
    return [];
  }
  return value.split(',').map((item) => item.trim()).filter(Boolean);
}

function shouldDecodeText(contentType) {
  if (!contentType) {
    return true;
  }
  return /text\//i.test(contentType) || /html/i.test(contentType);
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
      if (resolved.protocol === 'http:' || resolved.protocol === 'https:') {
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
