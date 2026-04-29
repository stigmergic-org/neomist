const IPFS_CODEC = 0xe3;
const IPNS_CODEC = 0xe5;

export function decodeContenthash(contenthashHex) {
  if (typeof contenthashHex !== 'string' || !contenthashHex.startsWith('0x')) {
    return { protocol: 'unknown', cid: null, error: 'invalid hex string' };
  }

  let bytes;
  try {
    bytes = Buffer.from(contenthashHex.slice(2), 'hex');
  } catch {
    return { protocol: 'unknown', cid: null, error: 'invalid hex bytes' };
  }

  const codec = decodeVarint(bytes, 0);
  if (!codec) {
    return { protocol: 'unknown', cid: null, error: 'invalid multicodec prefix' };
  }

  const cidBytes = bytes.subarray(codec.nextOffset);
  if (cidBytes.length === 0) {
    return { protocol: mapProtocol(codec.value), cid: null, error: 'missing cid bytes' };
  }

  return {
    protocol: mapProtocol(codec.value),
    cid: stringifyCid(cidBytes),
    error: null,
  };
}

function mapProtocol(codec) {
  if (codec === IPFS_CODEC) {
    return 'ipfs';
  }
  if (codec === IPNS_CODEC) {
    return 'ipns';
  }
  return 'unknown';
}

function decodeVarint(bytes, offset) {
  let value = 0n;
  let shift = 0n;

  for (let index = offset; index < bytes.length; index += 1) {
    const byte = BigInt(bytes[index]);
    value |= (byte & 0x7fn) << shift;
    if ((byte & 0x80n) === 0n) {
      const numberValue = Number(value);
      if (!Number.isSafeInteger(numberValue)) {
        return null;
      }
      return {
        value: numberValue,
        nextOffset: index + 1,
      };
    }
    shift += 7n;
    if (shift > 63n) {
      return null;
    }
  }

  return null;
}

function stringifyCid(cidBytes) {
  if (cidBytes.length === 34 && cidBytes[0] === 0x12 && cidBytes[1] === 0x20) {
    return base58btcEncode(cidBytes);
  }
  return `b${base32LowerEncode(cidBytes)}`;
}

function base32LowerEncode(bytes) {
  const alphabet = 'abcdefghijklmnopqrstuvwxyz234567';
  let bits = 0;
  let value = 0n;
  let output = '';

  for (const byte of bytes) {
    value = (value << 8n) | BigInt(byte);
    bits += 8;
    while (bits >= 5) {
      output += alphabet[Number((value >> BigInt(bits - 5)) & 31n)];
      bits -= 5;
    }
  }

  if (bits > 0) {
    output += alphabet[Number((value << BigInt(5 - bits)) & 31n)];
  }

  return output;
}

function base58btcEncode(bytes) {
  const alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
  if (bytes.length === 0) {
    return '';
  }

  const digits = [0];
  for (const byte of bytes) {
    let carry = byte;
    for (let index = 0; index < digits.length; index += 1) {
      const result = digits[index] * 256 + carry;
      digits[index] = result % 58;
      carry = Math.floor(result / 58);
    }
    while (carry > 0) {
      digits.push(carry % 58);
      carry = Math.floor(carry / 58);
    }
  }

  let output = '';
  for (const byte of bytes) {
    if (byte !== 0) {
      break;
    }
    output += alphabet[0];
  }

  for (let index = digits.length - 1; index >= 0; index -= 1) {
    output += alphabet[digits[index]];
  }

  return output;
}
