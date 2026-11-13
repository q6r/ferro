// Corpo do Web Crypto (crypto.subtle): normalização dos algoritmos (Blink:
// normalize_algorithm.cc), JWK, os algoritmos e a validação dos argumentos.
// Compilado só no primeiro uso de crypto.subtle (comum.js, `metodos()`); a
// matemática fica em Rust (script/crypto.rs). Devolve os métodos:
// nome → [mínimo de argumentos, corpo].
(function (N, DOMException, KEYS, novaChave, erroDe, ctxErro, TA_TAG) {
'use strict';
const str = (v) => `${v}`;
const opErro = (nome, msg = '') => erroDe(nome, msg);
const juntar = (...partes) => partes.flat().join(': ');
const VAZIO = new Uint8Array(0);

const AB_LEN = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
const ehBuffer = (v) => { try { AB_LEN.call(v); return true; } catch { return false; } };
const ehBufferSource = (v) => ArrayBuffer.isView(v) || ehBuffer(v);
// cópia dos bytes (buffer desanexado vira vazio, como no Chrome)
function copia(v) {
  try {
    return ArrayBuffer.isView(v) ? new Uint8Array(v.buffer, v.byteOffset, v.byteLength).slice() : new Uint8Array(v).slice();
  } catch { return new Uint8Array(0); }
}
const iguais = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);
// TruncateToBitLength: ceil(bits/8) bytes, bits que sobram zerados
function corta(b, bits) {
  const n = Math.ceil(bits / 8);
  const r = b.slice(0, n);
  if (bits % 8) r[n - 1] &= (0xff00 >> (bits % 8)) & 0xff;
  return r;
}

// conversões do WebIDL
function sequencia(v, ctx = '') {
  if (v === null || (typeof v !== 'object' && typeof v !== 'function') || typeof v[Symbol.iterator] !== 'function')
    throw ctxErro('TypeError', `${ctx}The provided value cannot be converted to a sequence.`);
  return Array.from(v, (x) => str(x));
}
const argAlg = (v) => (v !== null && (typeof v === 'object' || typeof v === 'function') ? v : str(v));
function argChave(v, i) {
  if (!KEYS.has(v)) throw ctxErro('TypeError', `parameter ${i} is not of type 'CryptoKey'.`);
}
function argBuffer(v) {
  if (!ehBufferSource(v)) throw ctxErro('TypeError', "The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.");
}
function uint32(v) {
  const n = +v;
  if (!Number.isFinite(n)) throw ctxErro('TypeError', `Value is${Number.isNaN(n) ? '' : ' infinite and'} not of type 'unsigned long'.`);
  const t = Math.trunc(n);
  if (t < 0 || t > 0xffffffff) throw ctxErro('TypeError', "Value is outside the 'unsigned long' value range.");
  return t;
}

// usos na ordem canônica do Chrome (é a ordem de CryptoKey.usages)
const USOS = ['encrypt', 'decrypt', 'sign', 'verify', 'deriveKey', 'deriveBits', 'wrapKey', 'unwrapKey'];
function validaUsos(lista) {
  if (!lista.every((u) => USOS.includes(u))) throw ctxErro('TypeError', 'Invalid keyUsages argument');
  return USOS.filter((u) => lista.includes(u));
}
const FORMATOS = ['raw', 'pkcs8', 'spki', 'jwk', 'raw-public', 'raw-private', 'raw-seed', 'raw-secret'];
function formato(f) {
  if (!FORMATOS.includes(f)) throw ctxErro('TypeError', `Invalid keyFormat argument: ${f}`);
}

// ---- normalização dos algoritmos (Blink: normalize_algorithm.cc)
const maiusc = (s) => s.replace(/[a-z]+/g, (c) => c.toUpperCase());
const POR_NOME = new Map(['AES-CBC', 'AES-CTR', 'AES-GCM', 'AES-KW', 'HMAC', 'RSASSA-PKCS1-v1_5', 'RSA-PSS', 'RSA-OAEP',
  'ECDSA', 'ECDH', 'Ed25519', 'X25519', 'HKDF', 'PBKDF2', 'SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'].map((n) => [maiusc(n), n]));
// por operação: algoritmo → dicionário dos parâmetros ('' = nenhum)
const OPS = (() => {
  const t = (nomes, p) => Object.fromEntries(nomes.map((n) => [n, p]));
  const AES = ['AES-CBC', 'AES-CTR', 'AES-GCM'], RSA = ['RSASSA-PKCS1-v1_5', 'RSA-PSS', 'RSA-OAEP'];
  const cifra = { 'AES-CBC': 'AesCbcParams', 'AES-CTR': 'AesCtrParams', 'AES-GCM': 'AeadParams', 'RSA-OAEP': 'RsaOaepParams' };
  const assina = { HMAC: '', 'RSASSA-PKCS1-v1_5': '', 'RSA-PSS': 'RsaPssParams', ECDSA: 'EcdsaParams', Ed25519: '' };
  return {
    encrypt: cifra, decrypt: cifra, sign: assina, verify: assina,
    digest: t(['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'], ''),
    generateKey: { ...t([...AES, 'AES-KW'], 'AesKeyGenParams'), HMAC: 'HmacKeyGenParams', ...t(RSA, 'RsaHashedKeyGenParams'),
      ...t(['ECDSA', 'ECDH'], 'EcKeyGenParams'), Ed25519: '', X25519: '' },
    importKey: { ...t([...AES, 'AES-KW', 'HKDF', 'PBKDF2', 'Ed25519', 'X25519'], ''), HMAC: 'HmacImportParams',
      ...t(RSA, 'RsaHashedImportParams'), ...t(['ECDSA', 'ECDH'], 'EcKeyImportParams') },
    'get key length': { ...t([...AES, 'AES-KW'], 'AesDerivedKeyParams'), HMAC: 'HmacImportParams', HKDF: '', PBKDF2: '' },
    deriveBits: { ECDH: 'EcdhKeyDeriveParams', X25519: 'EcdhKeyDeriveParams', HKDF: 'HkdfParams', PBKDF2: 'Pbkdf2Params' },
    wrapKey: { ...cifra, 'AES-KW': '' }, unwrapKey: { ...cifra, 'AES-KW': '' },
    exportKey: t([...AES, 'AES-KW', 'HMAC', ...RSA, 'ECDSA', 'ECDH', 'Ed25519', 'X25519'], ''),
  };
})();
// `ctx` é a pilha das mensagens ("HmacImportParams: hash: Algorithm: Unrecognized name")
function normaliza(alg, op, ctx = []) {
  let dic = {}, nome = alg;
  if (typeof alg !== 'string') {
    dic = alg;
    const v = alg.name;
    if (v === undefined) throw ctxErro('TypeError', juntar(ctx, 'Algorithm', 'name', 'Missing or not a string'));
    nome = str(v);
  }
  const canon = POR_NOME.get(maiusc(nome));
  if (!canon) throw ctxErro('NotSupportedError', juntar(ctx, 'Algorithm', 'Unrecognized name'));
  const tipo = OPS[op][canon];
  if (tipo === undefined) throw ctxErro('NotSupportedError', juntar(ctx, canon, 'Unsupported operation', op));
  const r = { name: canon };
  if (tipo) PARAMS[tipo](dic, [...ctx, tipo], r);
  return r;
}
function membroBuffer(d, k, c, obrigatorio) {
  const v = d[k];
  if (v === undefined) {
    if (obrigatorio) throw ctxErro('TypeError', juntar(c, k, 'Missing required property'));
    return undefined;
  }
  if (!ehBufferSource(v)) throw ctxErro('TypeError', juntar(c, k, 'Not a BufferSource'));
  return copia(v);
}
// inteiro com [EnforceRange]
function membroInteiro(d, k, c, max, obrigatorio) {
  const v = d[k];
  if (v === undefined) {
    if (obrigatorio) throw ctxErro('TypeError', juntar(c, k, 'Missing required property'));
    return undefined;
  }
  let n;
  try { n = +v; } catch { n = NaN; }
  if (Number.isNaN(n)) throw ctxErro('TypeError', juntar(c, k, 'Is not a number'));
  n = Math.trunc(n);
  if (!Number.isFinite(n) || n < 0 || n > max) throw ctxErro('TypeError', juntar(c, k, 'Outside of numeric range'));
  return n;
}
function membroHash(d, c) {
  let v = d.hash;
  if (v === null || (typeof v !== 'object' && typeof v !== 'function')) {
    if (v === undefined) throw ctxErro('TypeError', juntar(c, 'hash', 'Missing or not an AlgorithmIdentifier'));
    v = str(v);
  }
  return normaliza(v, 'digest', [...c, 'hash']).name;
}
function membroCurva(d, c) {
  const v = d.namedCurve;
  if (v === undefined) throw ctxErro('TypeError', juntar(c, 'namedCurve', 'Missing or not a string'));
  const s = str(v);
  if (s !== 'P-256' && s !== 'P-384' && s !== 'P-521') throw ctxErro('NotSupportedError', juntar(c, 'Unrecognized namedCurve'));
  return s;
}
const PARAMS = {
  AesCbcParams(d, c, r) { r.iv = membroBuffer(d, 'iv', c, true); },
  AesKeyGenParams(d, c, r) { r.length = membroInteiro(d, 'length', c, 0xffff, true); },
  AesDerivedKeyParams(d, c, r) { r.length = membroInteiro(d, 'length', c, 0xffff, true); },
  HmacImportParams(d, c, r) { r.hash = membroHash(d, c); r.length = membroInteiro(d, 'length', c, 0xffffffff, false); },
  HmacKeyGenParams(d, c, r) { r.hash = membroHash(d, c); r.length = membroInteiro(d, 'length', c, 0xffffffff, false); },
  RsaHashedKeyGenParams(d, c, r) {
    r.modulusLength = membroInteiro(d, 'modulusLength', c, 0xffffffff, true);
    const e = d.publicExponent;
    if (TA_TAG.call(e) !== 'Uint8Array') throw ctxErro('TypeError', juntar(c, 'publicExponent', 'Missing or not a Uint8Array'));
    r.publicExponent = e.length ? copia(e) : new Uint8Array(1);
    r.hash = membroHash(d, c);
  },
  RsaHashedImportParams(d, c, r) { r.hash = membroHash(d, c); },
  AesCtrParams(d, c, r) { r.counter = membroBuffer(d, 'counter', c, true); r.length = membroInteiro(d, 'length', c, 0xff, true); },
  AeadParams(d, c, r) {
    r.iv = membroBuffer(d, 'iv', c, true);
    r.additionalData = membroBuffer(d, 'additionalData', c, false);
    r.tagLength = membroInteiro(d, 'tagLength', c, 0xff, false);
  },
  RsaOaepParams(d, c, r) { r.label = membroBuffer(d, 'label', c, false); },
  RsaPssParams(d, c, r) { r.saltLength = membroInteiro(d, 'saltLength', c, 0xffffffff, true); },
  EcdsaParams(d, c, r) { r.hash = membroHash(d, c); },
  EcKeyGenParams(d, c, r) { r.namedCurve = membroCurva(d, c); },
  EcKeyImportParams(d, c, r) { r.namedCurve = membroCurva(d, c); },
  EcdhKeyDeriveParams(d, c, r) {
    const v = d.public;
    if (v === undefined) throw ctxErro('TypeError', juntar(c, 'public', 'Missing required property'));
    if (!KEYS.has(v)) throw ctxErro('TypeError', juntar(c, 'public', 'Must be a CryptoKey'));
    r.public = v;
  },
  Pbkdf2Params(d, c, r) {
    r.salt = membroBuffer(d, 'salt', c, true);
    r.iterations = membroInteiro(d, 'iterations', c, 0xffffffff, true);
    r.hash = membroHash(d, c);
  },
  HkdfParams(d, c, r) { r.hash = membroHash(d, c); r.salt = membroBuffer(d, 'salt', c, true); r.info = membroBuffer(d, 'info', c, true); },
};

// CryptoKeyPair é um dicionário: membros em ordem lexicográfica
const parChaves = (pub, priv) => ({ privateKey: priv, publicKey: pub });
function podeUsar(k, alg, uso) {
  const s = KEYS.get(k);
  if (s.alg.name !== alg.name) throw opErro('InvalidAccessError', 'key.algorithm does not match that of operation');
  if (!s.usos.includes(uso)) throw opErro('InvalidAccessError', 'key.usages does not permit this operation');
  return s;
}
function checaUsos(todos, usos) {
  if (usos.some((u) => !todos.includes(u))) throw opErro('SyntaxError', 'Cannot create a key using the specified key usages.');
}
function semUsos(usos) {
  if (!usos.length) throw opErro('SyntaxError', 'Usages cannot be empty when creating a key.');
}
const formatoImport = () => opErro('NotSupportedError', 'Unsupported import key format for algorithm');
const formatoExport = () => opErro('NotSupportedError', 'Unsupported export key format for algorithm');
function tipoChave(s, tipo) {
  if (s.tipo !== tipo) throw opErro('InvalidAccessError', 'The key is not of the expected type');
}

// ---- JWK (o JwkReader/JwkWriter do Chrome)
const b64u = (b) => b.toBase64({ alphabet: 'base64url', omitPadding: true });
function de64u(s) {
  if (!/^[A-Za-z0-9_-]*$/.test(s) || s.length % 4 === 1) return null;
  try { return Uint8Array.fromBase64(s, { alphabet: 'base64url', lastChunkHandling: 'loose' }); } catch { return null; }
}
// o dicionário JsonWebKey do importKey, com os membros convertidos pelo WebIDL
function jwkDicionario(v) {
  if (v !== null && v !== undefined && typeof v !== 'object' && typeof v !== 'function')
    throw ctxErro('TypeError', "The provided value is not of type '(ArrayBuffer or ArrayBufferView or JsonWebKey)'.");
  const r = {};
  if (v === null || v === undefined) return r;
  for (const k of ['alg', 'crv', 'd', 'dp', 'dq', 'e', 'ext', 'k', 'key_ops', 'kty', 'n', 'p', 'priv', 'pub', 'q', 'qi', 'use', 'x', 'y']) {
    const x = v[k];
    if (x === undefined) continue;
    if (k === 'ext') r.ext = !!x;
    else if (k === 'key_ops') r.key_ops = sequencia(x, "Failed to read the 'key_ops' property from 'JsonWebKey': ");
    else r[k] = str(x);
  }
  return r;
}
const USO_ENC = ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey', 'deriveKey', 'deriveBits'];
const jwkErro = (msg) => opErro('DataError', msg);
const jwkTipo = (nome, tipo) => jwkErro(`The JWK member "${nome}" must be a ${tipo}`);
const tem = (j, k) => Object.prototype.hasOwnProperty.call(j, k);
function jwkOpcional(j, nome, tipo) {
  if (!tem(j, nome)) return undefined;
  if (typeof j[nome] !== tipo) throw jwkTipo(nome, tipo);
  return j[nome];
}
function jwkTexto(j, nome) {
  if (!tem(j, nome)) throw jwkErro(`The required JWK member "${nome}" was missing`);
  if (typeof j[nome] !== 'string') throw jwkTipo(nome, 'string');
  return j[nome];
}
function jwkBytes(j, nome) {
  const b = de64u(jwkTexto(j, nome));
  if (!b) throw jwkErro(`The JWK member "${nome}" could not be base64url decoded or contained padding`);
  return b;
}
function jwkInteiro(j, nome) {
  const b = jwkBytes(j, nome);
  if (!b.length) throw jwkErro(`The JWK "${nome}" member was empty.`);
  if (b.length > 1 && !b[0]) throw jwkErro(`The JWK "${nome}" member contained a leading zero.`);
  return b;
}
function jwkTamanho(j, nome, n) {
  const b = jwkBytes(j, nome);
  if (b.length !== n) throw jwkErro(`The JWK's "${nome}" member defines an octet string of length ${b.length} bytes but should be ${n}`);
  return b;
}
function jwkAlg(j, esperado) {
  const a = jwkOpcional(j, 'alg', 'string');
  if (a !== undefined && a !== esperado) throw jwkErro('The JWK "alg" member was inconsistent with that specified by the Web Crypto call');
}
// JwkReader::Init: kty, ext, key_ops/use e alg; no unwrapKey o JWK chega como texto JSON
function jwkInicia(j, ext, usos, kty, alg) {
  if (j instanceof Uint8Array) {
    try { j = JSON.parse(N.utf8Decode(j)); } catch { j = null; }
    if (j === null || typeof j !== 'object' || Array.isArray(j)) throw jwkErro('JWK input could not be parsed to a JSON dictionary');
  }
  const k = jwkTexto(j, 'kty');
  if (k !== kty) throw jwkErro(`The JWK "kty" member was not "${kty}"`);
  if (jwkOpcional(j, 'ext', 'boolean') === false && ext)
    throw jwkErro('The "ext" member of the JWK dictionary is inconsistent what that specified by the Web Crypto call');
  let ops;
  if (tem(j, 'key_ops')) {
    if (!Array.isArray(j.key_ops)) throw jwkTipo('key_ops', 'list');
    ops = [];
    j.key_ops.forEach((op, i) => {
      if (typeof op !== 'string') throw jwkTipo(`key_ops[${i}]`, 'string');
      if (j.key_ops.indexOf(op) !== i) throw jwkErro('The "key_ops" member of the JWK dictionary contains duplicate usages.');
      if (USOS.includes(op)) ops.push(op);
    });
    if (usos.some((u) => !ops.includes(u)))
      throw jwkErro('The JWK "key_ops" member was inconsistent with that specified by the Web Crypto call. The JWK usage must be a superset of those requested');
  }
  const use = jwkOpcional(j, 'use', 'string');
  if (use !== undefined) {
    const m = use === 'enc' ? USO_ENC : use === 'sig' ? ['sign', 'verify'] : null;
    if (!m) throw jwkErro('The JWK "use" member could not be parsed');
    if (usos.some((u) => !m.includes(u)))
      throw jwkErro('The JWK "use" member was inconsistent with that specified by the Web Crypto call. The JWK usage must be a superset of those requested');
    if (ops && ops.some((u) => !m.includes(u)))
      throw jwkErro('The JWK "use" and "key_ops" properties were both found but are inconsistent with each other.');
  }
  if (alg) jwkAlg(j, alg);
  return j;
}
// membros em ordem lexicográfica, como no dicionário JsonWebKey do Chrome
function jwkSaida(s, campos) {
  const o = { ...campos, ext: s.ext, key_ops: s.usos.slice() };
  const r = {};
  for (const k of Object.keys(o).sort()) if (o[k] !== undefined) r[k] = o[k];
  return r;
}

// ---- algoritmos: as chaves secretas guardam os bytes; EC {d, pub}; RSA
// [n, e(, d, p, q, dp, dq, qi)]; Ed25519/X25519 {priv, pub}
const U_AES = ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey'];
const U_KDF = ['deriveKey', 'deriveBits'];
const BLOCO = { 'SHA-1': 512, 'SHA-256': 512, 'SHA-384': 1024, 'SHA-512': 1024 };
const bruto = (f) => f === 'raw' || f === 'raw-secret';
function aes(sufixo, todos, extra) {
  const nomeJwk = (n) => (n === 16 || n === 24 || n === 32 ? `A${n * 8}${sufixo}` : '');
  function importaBruta(b, alg, ext, usos) {
    checaUsos(todos, usos);
    if (b.length === 24) throw opErro('OperationError', '192-bit AES keys are not supported');
    if (b.length !== 16 && b.length !== 32) throw opErro('DataError', 'AES key data must be 128 or 256 bits');
    return novaChave('secret', ext, { name: alg.name, length: b.length * 8 }, usos, b);
  }
  function tamanho(n, erro) {
    if (n === 192) throw opErro('OperationError', '192-bit AES keys are not supported');
    if (n !== 128 && n !== 256) throw opErro('OperationError', erro);
  }
  return {
    generate(alg, ext, usos) {
      checaUsos(todos, usos);
      tamanho(alg.length, 'AES key length must be 128 or 256 bits');
      semUsos(usos);
      return novaChave('secret', ext, { name: alg.name, length: alg.length }, usos, N.cryptoRandom(alg.length / 8));
    },
    import(fmt, dado, alg, ext, usos) {
      if (bruto(fmt)) return importaBruta(dado, alg, ext, usos);
      if (fmt !== 'jwk') throw formatoImport();
      checaUsos(todos, usos);
      const j = jwkInicia(dado, ext, usos, 'oct');
      const k = jwkBytes(j, 'k');
      const a = jwkOpcional(j, 'alg', 'string');
      if (a !== undefined && a !== nomeJwk(k.length)) {
        if ([16, 24, 32].some((n) => a === nomeJwk(n)))
          throw jwkErro('The JWK "k" member did not include the right length of key data for the given algorithm.');
        throw jwkErro('The JWK "alg" member was inconsistent with that specified by the Web Crypto call');
      }
      return importaBruta(k, alg, ext, usos);
    },
    export(fmt, s) {
      if (bruto(fmt)) return s.m.slice();
      if (fmt !== 'jwk') throw formatoExport();
      return jwkSaida(s, { alg: nomeJwk(s.m.length), k: b64u(s.m), kty: 'oct' });
    },
    keyLength(alg) {
      tamanho(alg.length, 'AES key length must be 128 or 256 bits');
      return alg.length;
    },
    ...extra,
  };
}
function hmacJwk(hash) { return `HS${hash.slice(4)}`; }
const HMAC = {
  generate(alg, ext, usos) {
    checaUsos(['sign', 'verify'], usos);
    let bits = alg.length;
    if (bits === undefined) bits = BLOCO[alg.hash];
    else if (!bits) throw opErro('OperationError', 'HMAC key length must not be zero');
    semUsos(usos);
    const m = corta(N.cryptoRandom(Math.ceil(bits / 8)), bits);
    return novaChave('secret', ext, { name: 'HMAC', hash: alg.hash, length: bits }, usos, m);
  },
  import(fmt, dado, alg, ext, usos) {
    if (fmt === 'jwk') {
      checaUsos(['sign', 'verify'], usos);
      const j = jwkInicia(dado, ext, usos, 'oct');
      dado = jwkBytes(j, 'k');
      jwkAlg(j, hmacJwk(alg.hash));
    } else if (!bruto(fmt)) throw formatoImport();
    checaUsos(['sign', 'verify'], usos);
    if (!dado.length) throw opErro('DataError', 'HMAC key data must not be empty');
    let bits = dado.length * 8;
    if (alg.length !== undefined) {
      if (Math.ceil(alg.length / 8) !== dado.length)
        throw opErro('DataError', 'The optional HMAC key length must be shorter than the key data, and by no more than 7 bits.');
      bits = alg.length;
    }
    return novaChave('secret', ext, { name: 'HMAC', hash: alg.hash, length: bits }, usos, corta(dado, bits));
  },
  export(fmt, s) {
    if (bruto(fmt)) return s.m.slice();
    if (fmt !== 'jwk') throw formatoExport();
    return jwkSaida(s, { alg: hmacJwk(s.alg.hash), k: b64u(s.m), kty: 'oct' });
  },
  sign: (a, s, d) => N.cryptoHmac(s.alg.hash, s.m, d),
  verify: (a, s, sig, d) => N.cryptoHmac(s.alg.hash, s.m, d, sig),
  keyLength(alg) {
    if (alg.length === undefined) return BLOCO[alg.hash];
    if (!alg.length) throw opErro('TypeError', 'HMAC key length must not be zero');
    return alg.length;
  },
};
// PBKDF2 e HKDF: só "raw", nunca extraíveis
function kdf(nome, derivar) {
  return {
    import(fmt, dado, alg, ext, usos) {
      if (!bruto(fmt)) throw formatoImport();
      checaUsos(U_KDF, usos);
      if (ext) throw opErro('SyntaxError', 'KDF keys must set extractable=false');
      return novaChave('secret', false, { name: nome }, usos, dado);
    },
    keyLength: () => null,
    deriveBits: derivar,
  };
}
const PBKDF2 = kdf('PBKDF2', (a, s, n) => {
  if (n === null) throw opErro('OperationError', 'No length was specified for the PBKDF2 Derive Bits operation.');
  if (n % 8) throw opErro('OperationError', 'Length for PBKDF2 key derivation must be a multiple of 8 bits.');
  if (!a.iterations) throw opErro('OperationError', 'PBKDF2 requires iterations > 0');
  return N.cryptoPbkdf2(a.hash, s.m, a.salt, a.iterations, n / 8);
});
const HKDF = kdf('HKDF', (a, s, n) => {
  if (n === null) throw opErro('OperationError', 'No length was specified for the HKDF Derive Bits operation.');
  if (n % 8) throw opErro('OperationError', 'The length provided for HKDF is not a multiple of 8 bits.');
  return N.cryptoHkdf(a.hash, s.m, a.salt, a.info, n / 8);
});
// ECDSA e ECDH (P-256 e P-384; a P-521 é reconhecida mas não implementada)
function ec(nome, pub, priv, algJwk, extra) {
  const todos = [...pub, ...priv];
  function curva(c) {
    if (c === 'P-521') throw opErro('NotSupportedError', 'The requested operation is unsupported');
    return c;
  }
  const alg = (c) => ({ name: nome, namedCurve: c });
  const publica = (c, ext, usos, p) => novaChave('public', ext, alg(c), usos, { d: null, pub: p });
  const privada = (c, ext, usos, d, p) => novaChave('private', ext, alg(c), usos, { d, pub: p });
  return {
    generate(a, ext, usos) {
      checaUsos(todos, usos);
      const c = curva(a.namedCurve);
      const uv = usos.filter((u) => priv.includes(u));
      semUsos(uv);
      const [d, p] = N.cryptoEc('gerar', c);
      return parChaves(publica(c, true, usos.filter((u) => pub.includes(u)), p), privada(c, ext, uv, d, p));
    },
    import(fmt, dado, a, ext, usos) {
      const c = a.namedCurve;
      if (fmt === 'raw' || fmt === 'raw-public' || fmt === 'spki') {
        checaUsos(pub, usos);
        return publica(c, ext, usos, N.cryptoEc(fmt === 'spki' ? 'spki' : 'raw', curva(c), dado));
      }
      if (fmt === 'pkcs8') {
        checaUsos(priv, usos);
        const [d, p] = N.cryptoEc('pkcs8', curva(c), dado);
        return privada(c, ext, usos, d, p);
      }
      if (fmt !== 'jwk') throw formatoImport();
      const j = jwkInicia(dado, ext, usos, 'EC', algJwk && algJwk[c]);
      if (jwkTexto(j, 'crv') !== c) throw jwkErro('The JWK\'s "crv" member specifies a different curve than requested');
      const ehPrivada = tem(j, 'd');
      checaUsos(ehPrivada ? priv : pub, usos);
      curva(c);
      const n = c === 'P-256' ? 32 : 48;
      const ponto = new Uint8Array(2 * n + 1);
      ponto[0] = 4;
      ponto.set(jwkTamanho(j, 'x', n), 1);
      ponto.set(jwkTamanho(j, 'y', n), n + 1);
      const p = N.cryptoEc('raw', c, ponto);
      if (!ehPrivada) return publica(c, ext, usos, p);
      const [d] = N.cryptoEc('jwk', c, ponto.subarray(1), jwkTamanho(j, 'd', n));
      return privada(c, ext, usos, d, p);
    },
    export(fmt, s) {
      const c = s.alg.namedCurve;
      if (fmt === 'raw' || fmt === 'raw-public') { tipoChave(s, 'public'); return s.m.pub.slice(); }
      if (fmt === 'spki') { tipoChave(s, 'public'); return N.cryptoEc('para-spki', c, s.m.pub); }
      if (fmt === 'pkcs8') { tipoChave(s, 'private'); return N.cryptoEc('para-pkcs8', c, s.m.d); }
      if (fmt !== 'jwk') throw formatoExport();
      const n = (s.m.pub.length - 1) / 2;
      return jwkSaida(s, { crv: c, d: s.m.d ? b64u(s.m.d) : undefined, kty: 'EC',
        x: b64u(s.m.pub.subarray(1, n + 1)), y: b64u(s.m.pub.subarray(n + 1)) });
    },
    ...extra,
  };
}
const ECDSA = ec('ECDSA', ['verify'], ['sign'], { 'P-256': 'ES256', 'P-384': 'ES384', 'P-521': 'ES512' }, {
  sign(a, s, d) { tipoChave(s, 'private'); return N.cryptoEc('assina', s.alg.namedCurve, a.hash, s.m.d, d); },
  verify(a, s, sig, d) { tipoChave(s, 'public'); return N.cryptoEc('verifica', s.alg.namedCurve, a.hash, s.m.pub, d, sig); },
});
const ECDH = ec('ECDH', [], U_KDF, null, {
  deriveBits(a, s, n) {
    tipoChave(s, 'private');
    const p = KEYS.get(a.public);
    if (p.tipo !== 'public' || !p.alg.namedCurve)
      throw opErro('InvalidAccessError', 'The public parameter for ECDH key derivation is not a public EC key');
    if (p.alg.name !== 'ECDH') throw opErro('InvalidAccessError', 'The public parameter for ECDH key derivation must be for ECDH');
    const c = s.alg.namedCurve;
    if (p.alg.namedCurve !== c)
      throw opErro('InvalidAccessError', 'The public parameter for ECDH key derivation is for a different named curve');
    const max = c === 'P-256' ? 256 : 384;
    if (n === null) n = max;
    if (!n) return new Uint8Array(0);
    if (n > max) throw opErro('OperationError', `Length specified for ECDH key derivation is too large. Maximum allowed is ${max} bits`);
    return corta(N.cryptoEc('ecdh', c, s.m.d, p.m.pub), n);
  },
});
// RSASSA-PKCS1-v1_5, RSA-PSS e RSA-OAEP
const bitsDe = (n) => (n.length - 1) * 8 + 32 - Math.clz32(n[0]);
function expoente(b) {
  let v = 0;
  for (const x of b) if ((v = v * 256 + x) > 0xffffffff) return -1;
  return v;
}
function rsa(nome, pub, priv, jwkAlgs, extra) {
  const todos = [...pub, ...priv];
  const alg = (h, c) => ({ name: nome, hash: h, modulusLength: bitsDe(c[0]), publicExponent: c[1].slice() });
  const publica = (h, ext, usos, c) => novaChave('public', ext, alg(h, c), usos, c.slice(0, 2));
  const privada = (h, ext, usos, c) => novaChave('private', ext, alg(h, c), usos, c);
  return {
    generate(a, ext, usos) {
      checaUsos(todos, usos);
      const bits = a.modulusLength;
      if (bits < 256 || bits > 8192 || bits % 8)
        throw opErro('OperationError', 'The modulus length must be a multiple of 8 bits and >= 256 and <= 8192');
      const e = expoente(a.publicExponent);
      if (e !== 3 && e !== 65537) throw opErro('OperationError', 'The "publicExponent" must be either 3 or 65537');
      const uv = usos.filter((u) => priv.includes(u));
      semUsos(uv);
      const c = N.cryptoRsa('gerar', bits, e);
      return parChaves(publica(a.hash, true, usos.filter((u) => pub.includes(u)), c), privada(a.hash, ext, uv, c));
    },
    import(fmt, dado, a, ext, usos) {
      if (fmt === 'spki') { checaUsos(pub, usos); return publica(a.hash, ext, usos, N.cryptoRsa('spki', dado)); }
      if (fmt === 'pkcs8') { checaUsos(priv, usos); return privada(a.hash, ext, usos, N.cryptoRsa('pkcs8', dado)); }
      if (fmt !== 'jwk') throw formatoImport();
      const j = jwkInicia(dado, ext, usos, 'RSA', jwkAlgs[a.hash]);
      const c = [jwkInteiro(j, 'n'), jwkInteiro(j, 'e')];
      const ehPrivada = tem(j, 'd');
      if (ehPrivada) for (const k of ['d', 'p', 'q', 'dp', 'dq', 'qi']) c.push(jwkInteiro(j, k));
      checaUsos(ehPrivada ? priv : pub, usos);
      N.cryptoRsa('jwk', c);
      return (ehPrivada ? privada : publica)(a.hash, ext, usos, c);
    },
    export(fmt, s) {
      if (fmt === 'spki') { tipoChave(s, 'public'); return N.cryptoRsa('para-spki', s.m); }
      if (fmt === 'pkcs8') { tipoChave(s, 'private'); return N.cryptoRsa('para-pkcs8', s.m); }
      if (fmt !== 'jwk') throw formatoExport();
      const campos = { alg: jwkAlgs[s.alg.hash], kty: 'RSA' };
      s.m.forEach((b, i) => { campos[['n', 'e', 'd', 'p', 'q', 'dp', 'dq', 'qi'][i]] = b64u(b); });
      return jwkSaida(s, campos);
    },
    ...extra,
  };
}
const algsJwk = (f) => ({ 'SHA-1': f('1'), 'SHA-256': f('256'), 'SHA-384': f('384'), 'SHA-512': f('512') });
function assinaturaRsa(esquema) {
  const sal = (a) => (esquema === 'pss' ? a.saltLength : 0);
  return {
    sign(a, s, d) { tipoChave(s, 'private'); return N.cryptoRsa('assina', s.m, esquema, s.alg.hash, d, sal(a)); },
    verify(a, s, sig, d) { tipoChave(s, 'public'); return N.cryptoRsa('verifica', s.m, esquema, s.alg.hash, d, sal(a), sig); },
  };
}
const RSASSA = rsa('RSASSA-PKCS1-v1_5', ['verify'], ['sign'], algsJwk((h) => `RS${h}`), assinaturaRsa('pkcs1'));
const RSA_PSS = rsa('RSA-PSS', ['verify'], ['sign'], algsJwk((h) => `PS${h}`), assinaturaRsa('pss'));
const RSA_OAEP = rsa('RSA-OAEP', ['encrypt', 'wrapKey'], ['decrypt', 'unwrapKey'],
  algsJwk((h) => (h === '1' ? 'RSA-OAEP' : `RSA-OAEP-${h}`)), {
    encrypt(a, s, d) { tipoChave(s, 'public'); return N.cryptoRsa('oaep-e', s.m, s.alg.hash, a.label || VAZIO, d); },
    decrypt(a, s, d) { tipoChave(s, 'private'); return N.cryptoRsa('oaep-d', s.m, s.alg.hash, a.label || VAZIO, d); },
  });
// Ed25519 e X25519
function okp(nome, pub, priv, extra) {
  const todos = [...pub, ...priv];
  const publica = (ext, usos, p) => novaChave('public', ext, { name: nome }, usos, { priv: null, pub: p });
  const privada = (ext, usos, s) => novaChave('private', ext, { name: nome }, usos, { priv: s, pub: N.cryptoOkp('publica', nome, s) });
  return {
    generate(a, ext, usos) {
      checaUsos(todos, usos);
      const uv = usos.filter((u) => priv.includes(u));
      semUsos(uv);
      const k = privada(ext, uv, N.cryptoOkp('gerar', nome));
      return parChaves(publica(true, usos.filter((u) => pub.includes(u)), KEYS.get(k).m.pub), k);
    },
    import(fmt, dado, a, ext, usos) {
      if (fmt === 'raw' || fmt === 'raw-public') {
        checaUsos(pub, usos);
        if (dado.length !== 32) throw opErro('DataError', `${nome} key data must be 256 bits`);
        return publica(ext, usos, dado);
      }
      if (fmt === 'spki') { checaUsos(pub, usos); return publica(ext, usos, N.cryptoOkp('spki', nome, dado)); }
      if (fmt === 'pkcs8') { checaUsos(priv, usos); return privada(ext, usos, N.cryptoOkp('pkcs8', nome, dado)); }
      if (fmt !== 'jwk') throw formatoImport();
      const j = jwkInicia(dado, ext, usos, 'OKP');
      if (jwkTexto(j, 'crv') !== nome) throw jwkErro('The JWK\'s "crv" member specifies a different curve than requested');
      if (nome === 'Ed25519') {
        const al = jwkOpcional(j, 'alg', 'string');
        if (al !== undefined && al !== 'EdDSA' && al !== 'Ed25519')
          throw jwkErro('The JWK "alg" member was inconsistent with that specified by the Web Crypto call');
      }
      const ehPrivada = tem(j, 'd');
      checaUsos(ehPrivada ? priv : pub, usos);
      const x = jwkTamanho(j, 'x', 32);
      if (!ehPrivada) return publica(ext, usos, x);
      const k = privada(ext, usos, jwkTamanho(j, 'd', 32));
      if (!iguais(KEYS.get(k).m.pub, x)) throw opErro('DataError');
      return k;
    },
    export(fmt, s) {
      if (fmt === 'raw' || fmt === 'raw-public') { tipoChave(s, 'public'); return s.m.pub.slice(); }
      if (fmt === 'spki') { tipoChave(s, 'public'); return N.cryptoOkp('para-spki', nome, s.m.pub); }
      if (fmt === 'pkcs8') { tipoChave(s, 'private'); return N.cryptoOkp('para-pkcs8', nome, s.m.priv); }
      if (fmt !== 'jwk') throw formatoExport();
      return jwkSaida(s, { alg: nome === 'Ed25519' ? nome : undefined, crv: nome, d: s.m.priv ? b64u(s.m.priv) : undefined,
        kty: 'OKP', x: b64u(s.m.pub) });
    },
    ...extra,
  };
}
const ED25519 = okp('Ed25519', ['verify'], ['sign'], {
  sign(a, s, d) { tipoChave(s, 'private'); return N.cryptoOkp('assina', 'Ed25519', s.m.priv, d); },
  verify(a, s, sig, d) { tipoChave(s, 'public'); return N.cryptoOkp('verifica', 'Ed25519', s.m.pub, d, sig); },
});
const X25519 = okp('X25519', [], U_KDF, {
  deriveBits(a, s, n) {
    tipoChave(s, 'private');
    const p = KEYS.get(a.public);
    if (p.tipo !== 'public') throw opErro('InvalidAccessError', 'The public parameter for X25519 key derivation is not a public key');
    if (p.alg.name !== 'X25519') throw opErro('InvalidAccessError', 'The public parameter for X25519 key derivation must be for X25519');
    const b = N.cryptoOkp('deriva', 'X25519', s.m.priv, p.m.pub);
    if (n === null) return b;
    if (n > 256) throw opErro('OperationError', 'The length provided for X25519 is too large.');
    return corta(b, n);
  },
});
const IMPL = {
  'AES-CBC': aes('CBC', U_AES, {
    encrypt: (a, s, d) => N.cryptoAes('cbc-e', s.m, d, a.iv),
    decrypt: (a, s, d) => N.cryptoAes('cbc-d', s.m, d, a.iv),
  }),
  'AES-CTR': aes('CTR', U_AES, {
    encrypt: (a, s, d) => N.cryptoAes('ctr', s.m, d, a.counter, a.length),
    decrypt: (a, s, d) => N.cryptoAes('ctr', s.m, d, a.counter, a.length),
  }),
  'AES-GCM': aes('GCM', U_AES, {
    encrypt: (a, s, d) => N.cryptoAes('gcm-e', s.m, d, a.iv, a.additionalData || VAZIO, a.tagLength ?? 128),
    decrypt: (a, s, d) => N.cryptoAes('gcm-d', s.m, d, a.iv, a.additionalData || VAZIO, a.tagLength ?? 128),
  }),
  // no AES-KW, "cifrar" é o wrapKey e "decifrar" é o unwrapKey
  'AES-KW': aes('KW', ['wrapKey', 'unwrapKey'], {
    encrypt: (a, s, d) => N.cryptoAes('kw-e', s.m, d),
    decrypt: (a, s, d) => N.cryptoAes('kw-d', s.m, d),
  }),
  HMAC, 'RSASSA-PKCS1-v1_5': RSASSA, 'RSA-PSS': RSA_PSS, 'RSA-OAEP': RSA_OAEP, ECDSA, ECDH,
  Ed25519: ED25519, X25519, HKDF, PBKDF2,
};
// importKey (e o fim de unwrapKey/deriveKey): chave secreta ou privada sem usos não vale
function criaChave(fmt, dado, alg, ext, usos) {
  const k = IMPL[alg.name].import(fmt, dado, alg, ext, usos);
  const s = KEYS.get(k);
  if (s.tipo !== 'public') semUsos(s.usos);
  return k;
}

// ---- SubtleCrypto: os métodos validam os argumentos como o WebIDL e
// devolvem sempre uma Promise (os erros viram rejeição)
const saida = (b) => b.buffer;
function cifraOuDecifra(op, alg, k, d) {
  alg = argAlg(alg); argChave(k, 2); argBuffer(d);
  const a = normaliza(alg, op);
  const b = copia(d);
  return saida(IMPL[a.name][op](a, podeUsar(k, a, op), b));
}
const METODOS = {
  encrypt: [3, (alg, k, d) => cifraOuDecifra('encrypt', alg, k, d)],
  decrypt: [3, (alg, k, d) => cifraOuDecifra('decrypt', alg, k, d)],
  sign: [3, (alg, k, d) => {
    alg = argAlg(alg); argChave(k, 2); argBuffer(d);
    const a = normaliza(alg, 'sign');
    const b = copia(d);
    return saida(IMPL[a.name].sign(a, podeUsar(k, a, 'sign'), b));
  }],
  verify: [4, (alg, k, sig, d) => {
    alg = argAlg(alg); argChave(k, 2); argBuffer(sig); argBuffer(d);
    const a = normaliza(alg, 'verify');
    const bs = copia(sig), bd = copia(d);
    return IMPL[a.name].verify(a, podeUsar(k, a, 'verify'), bs, bd);
  }],
  digest: [2, (alg, d) => {
    alg = argAlg(alg); argBuffer(d);
    const a = normaliza(alg, 'digest');
    return saida(N.cryptoDigest(a.name, copia(d)));
  }],
  generateKey: [3, (alg, ext, usos) => {
    alg = argAlg(alg); ext = !!ext;
    const u = validaUsos(sequencia(usos));
    const a = normaliza(alg, 'generateKey');
    return IMPL[a.name].generate(a, ext, u);
  }],
  deriveKey: [5, (alg, k, tipo, ext, usos) => {
    alg = argAlg(alg); argChave(k, 2); tipo = argAlg(tipo); ext = !!ext;
    const u = validaUsos(sequencia(usos));
    const a = normaliza(alg, 'deriveBits');
    const ai = normaliza(tipo, 'importKey');
    const al = normaliza(tipo, 'get key length');
    const s = podeUsar(k, a, 'deriveKey');
    const bits = IMPL[a.name].deriveBits(a, s, IMPL[al.name].keyLength(al));
    return criaChave('raw-secret', bits, ai, ext, u);
  }],
  deriveBits: [2, (alg, k, n) => {
    alg = argAlg(alg); argChave(k, 2);
    n = n === undefined || n === null ? null : uint32(n);
    const a = normaliza(alg, 'deriveBits');
    return saida(IMPL[a.name].deriveBits(a, podeUsar(k, a, 'deriveBits'), n));
  }],
  importKey: [5, (fmt, dado, alg, ext, usos) => {
    fmt = str(fmt);
    const jwk = ehBufferSource(dado) ? null : jwkDicionario(dado);
    alg = argAlg(alg); ext = !!ext;
    const lista = sequencia(usos);
    formato(fmt);
    const u = validaUsos(lista);
    const a = normaliza(alg, 'importKey');
    if (fmt === 'jwk' && !jwk) throw ctxErro('TypeError', 'Key data must be an object for JWK import');
    if (fmt !== 'jwk' && jwk) throw ctxErro('TypeError', 'Key data must be a BufferSource for non-JWK formats');
    return criaChave(fmt, jwk || copia(dado), a, ext, u);
  }],
  exportKey: [2, (fmt, k) => {
    fmt = str(fmt); argChave(k, 2);
    formato(fmt);
    const s = KEYS.get(k);
    if (!(s.alg.name in OPS.exportKey)) throw ctxErro('NotSupportedError', `${s.alg.name}: Unsupported operation: exportKey`);
    if (!s.ext) throw ctxErro('InvalidAccessError', 'key is not extractable');
    const r = IMPL[s.alg.name].export(fmt, s);
    return r instanceof Uint8Array ? saida(r) : r;
  }],
  wrapKey: [4, (fmt, k, kw, alg) => {
    fmt = str(fmt); argChave(k, 2); argChave(kw, 3); alg = argAlg(alg);
    formato(fmt);
    const a = normaliza(alg, 'wrapKey');
    const sw = podeUsar(kw, a, 'wrapKey');
    const s = KEYS.get(k);
    if (!(s.alg.name in OPS.exportKey)) throw opErro('NotSupportedError', `${s.alg.name}: Unsupported operation: exportKey`);
    if (!s.ext) throw opErro('InvalidAccessError', 'key is not extractable');
    let b = IMPL[s.alg.name].export(fmt, s);
    if (!(b instanceof Uint8Array)) b = N.utf8Encode(JSON.stringify(b));
    return saida(IMPL[a.name].encrypt(a, sw, b));
  }],
  unwrapKey: [7, (fmt, w, kw, alg, algChave, ext, usos) => {
    fmt = str(fmt); argBuffer(w); argChave(kw, 3); alg = argAlg(alg); algChave = argAlg(algChave); ext = !!ext;
    const lista = sequencia(usos);
    formato(fmt);
    const u = validaUsos(lista);
    const a = normaliza(alg, 'unwrapKey');
    const ak = normaliza(algChave, 'importKey');
    const b = copia(w);
    const claro = IMPL[a.name].decrypt(a, podeUsar(kw, a, 'unwrapKey'), b);
    return criaChave(fmt, claro, ak, ext, u);
  }],
};
return METODOS;
})
