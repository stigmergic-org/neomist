import { DEFAULTS } from './config.mjs';

export function isMainnetEnsName(name, excludedSuffixes = DEFAULTS.excludedNamespaceSuffixes) {
  if (typeof name !== 'string') {
    return false;
  }

  const lower = name.toLowerCase();
  if (!lower.endsWith('.eth')) {
    return false;
  }

  for (const suffix of excludedSuffixes) {
    const lowerSuffix = suffix.toLowerCase();
    if (lower !== lowerSuffix && lower.endsWith(`.${lowerSuffix}`)) {
      return false;
    }
  }

  return true;
}

export function isSubdomain(name) {
  return String(name).split('.').length > 2;
}

export function parentName(name) {
  const parts = String(name).split('.');
  if (parts.length <= 1) {
    return null;
  }
  return parts.slice(1).join('.');
}

export function nodeShard(node) {
  return [node.slice(2, 4), node.slice(4, 6)];
}

export function nameShards(name) {
  const key = encodedNameKey(name).slice(0, -5).padEnd(4, '_');
  return [key.slice(0, 2), key.slice(2, 4)];
}

export function encodedNameFile(name) {
  return `${encodedNameKey(name)}.json`;
}

function encodedNameKey(name) {
  return encodeURIComponent(String(name).toLowerCase());
}
