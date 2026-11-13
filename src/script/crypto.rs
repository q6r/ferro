//! Web Crypto (`crypto.subtle`): a matemática, em Rust, sobre as crates da
//! RustCrypto que o TLS já traz. Aqui só há funções puras sobre bytes; o
//! prelúdio guarda o material das chaves (num WeakMap, longe da página) e
//! cuida da normalização dos algoritmos, do JWK e das checagens de uso, com
//! as mensagens do Chrome.
//!
//! SHA-1 e PBKDF2 são feitos aqui (não estão no Cargo.lock), assim como os
//! modos do AES: o Web Crypto pede contador com `length` bits no CTR, tags
//! curtas e IV de qualquer tamanho no GCM, que as crates prontas não cobrem.

use digest::Digest;
use ferro_js::value::{CallArgs, ObjRef};
use ferro_js::{Value, Vm};
use getrandom::SysRng;
use getrandom::rand_core::UnwrapErr;
use hmac::{Hmac, KeyInit, Mac};
use p256::pkcs8::der::{Decode, Encode, Tagged};
use p256::pkcs8::{ObjectIdentifier, PrivateKeyInfoRef, SubjectPublicKeyInfoRef};
use rsa::BoxedUint;
use rsa::traits::{PrivateKeyParts, PublicKeyParts};

use super::web::{bytes_of, uint8_array};

type R = Result<Value, Value>;

/// Erro de uma operação: nome do DOMException e mensagem (as do Chrome).
#[derive(Debug, PartialEq)]
struct Falha(&'static str, String);

fn falha(nome: &'static str, msg: &str) -> Falha {
    Falha(nome, msg.to_string())
}

/// OperationError sem mensagem (o `Status::OperationError()` do Chrome).
fn operacao() -> Falha {
    falha("OperationError", "")
}

/// DataError sem mensagem (DER inválido, chave inconsistente...).
fn dados() -> Falha {
    falha("DataError", "")
}

fn nao_suportado() -> Falha {
    falha("NotSupportedError", "The requested operation is unsupported")
}

// ------------------------------------------------------------- aleatório

/// Bytes do gerador do sistema operacional.
fn aleatorio(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    getrandom::fill(&mut v).expect("gerador aleatório do sistema");
    v
}

fn rng() -> UnwrapErr<SysRng> {
    UnwrapErr(SysRng)
}

/// Comparação em tempo constante (MACs e tags).
fn iguais(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ----------------------------------------------------------------- SHA-1

/// SHA-1 com as traits da `digest` (para HMAC, HKDF e RSA genéricos).
mod sha1 {
    use core::fmt;
    use digest::block_api::{
        AlgorithmName, Block, BlockSizeUser, Buffer, BufferKindUser, Eager, FixedOutputCore, OutputSizeUser, Reset,
        UpdateCore,
    };
    use digest::typenum::{U20, U64};
    use digest::{HashMarker, Output};

    const H0: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];

    fn comprime(h: &mut [u32; 5], bloco: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (i, c) in bloco.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = *h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i / 20 {
                0 => ((b & c) | (!b & d), 0x5A827999),
                1 => (b ^ c ^ d, 0x6ED9EBA1),
                2 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }

    #[derive(Clone)]
    pub struct Sha1Core {
        h: [u32; 5],
        blocos: u64,
    }

    impl HashMarker for Sha1Core {}

    impl BlockSizeUser for Sha1Core {
        type BlockSize = U64;
    }

    impl BufferKindUser for Sha1Core {
        type BufferKind = Eager;
    }

    impl OutputSizeUser for Sha1Core {
        type OutputSize = U20;
    }

    impl UpdateCore for Sha1Core {
        fn update_blocks(&mut self, blocos: &[Block<Self>]) {
            self.blocos += blocos.len() as u64;
            for b in blocos {
                comprime(&mut self.h, &b.0);
            }
        }
    }

    impl FixedOutputCore for Sha1Core {
        fn finalize_fixed_core(&mut self, buffer: &mut Buffer<Self>, saida: &mut Output<Self>) {
            let bits = 8 * (buffer.get_pos() as u64 + 64 * self.blocos);
            let mut h = self.h;
            buffer.len64_padding_be(bits, |b| comprime(&mut h, &b.0));
            for (c, v) in saida.chunks_exact_mut(4).zip(h) {
                c.copy_from_slice(&v.to_be_bytes());
            }
        }
    }

    impl Default for Sha1Core {
        fn default() -> Self {
            Sha1Core { h: H0, blocos: 0 }
        }
    }

    impl Reset for Sha1Core {
        fn reset(&mut self) {
            *self = Self::default();
        }
    }

    impl AlgorithmName for Sha1Core {
        fn write_alg_name(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Sha1")
        }
    }

    digest::buffer_fixed!(
        /// SHA-1 (só para o Web Crypto: digest, HMAC, PBKDF2, HKDF e RSA).
        pub struct Sha1(Sha1Core);
        impl: BaseFixedTraits AlgorithmName Default Clone HashMarker Reset FixedOutputReset;
    );

    impl digest::const_oid::AssociatedOid for Sha1 {
        const OID: digest::const_oid::ObjectIdentifier =
            digest::const_oid::ObjectIdentifier::new_unwrap("1.3.14.3.2.26");
    }
}

// ---------------------------------------------------------------- hashes

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    fn de(nome: &str) -> Result<Hash, Falha> {
        Ok(match nome {
            "SHA-1" => Hash::Sha1,
            "SHA-256" => Hash::Sha256,
            "SHA-384" => Hash::Sha384,
            "SHA-512" => Hash::Sha512,
            _ => return Err(nao_suportado()),
        })
    }
}

/// Roda `$e` com `$d` sendo o tipo do hash escolhido.
macro_rules! com_hash {
    ($h:expr, $d:ident => $e:expr) => {
        match $h {
            Hash::Sha1 => {
                type $d = sha1::Sha1;
                $e
            }
            Hash::Sha256 => {
                type $d = sha2::Sha256;
                $e
            }
            Hash::Sha384 => {
                type $d = sha2::Sha384;
                $e
            }
            Hash::Sha512 => {
                type $d = sha2::Sha512;
                $e
            }
        }
    };
}

fn resumo(h: Hash, dados: &[u8]) -> Vec<u8> {
    com_hash!(h, D => D::digest(dados).to_vec())
}

fn hmac(h: Hash, chave: &[u8], dados: &[u8]) -> Vec<u8> {
    com_hash!(h, D => {
        let mut m = <Hmac<D> as KeyInit>::new_from_slice(chave).expect("HMAC aceita chave de qualquer tamanho");
        m.update(dados);
        m.finalize().into_bytes().to_vec()
    })
}

/// PBKDF2 (RFC 8018) sobre um HMAC qualquer.
fn pbkdf2_com<M: Mac + KeyInit + Clone>(senha: &[u8], sal: &[u8], iteracoes: u32, saida: &mut [u8]) {
    let prf = <M as KeyInit>::new_from_slice(senha).expect("HMAC aceita chave de qualquer tamanho");
    for (i, pedaco) in saida.chunks_mut(M::output_size()).enumerate() {
        let mut m = prf.clone();
        m.update(sal);
        m.update(&(i as u32 + 1).to_be_bytes());
        let mut u = m.finalize().into_bytes();
        let mut t = u.clone();
        for _ in 1..iteracoes {
            let mut m = prf.clone();
            m.update(&u);
            u = m.finalize().into_bytes();
            for (a, b) in t.iter_mut().zip(u.iter()) {
                *a ^= b;
            }
        }
        pedaco.copy_from_slice(&t[..pedaco.len()]);
    }
}

fn pbkdf2(h: Hash, senha: &[u8], sal: &[u8], iteracoes: u32, n: usize) -> Vec<u8> {
    let mut saida = vec![0u8; n];
    com_hash!(h, D => pbkdf2_com::<Hmac<D>>(senha, sal, iteracoes, &mut saida));
    saida
}

fn hkdf(h: Hash, ikm: &[u8], sal: &[u8], info: &[u8], n: usize) -> Result<Vec<u8>, Falha> {
    let longo = || falha("OperationError", "The length provided for HKDF is too large.");
    // o máximo do HKDF é 255 blocos do hash: confere antes de alocar
    if n > 255 * resumo(h, b"").len() {
        return Err(longo());
    }
    let mut saida = vec![0u8; n];
    com_hash!(h, D => hkdf::Hkdf::<D>::new(Some(sal), ikm).expand(info, &mut saida)).map_err(|_| longo())?;
    Ok(saida)
}

// ------------------------------------------------------------------- AES

enum Aes {
    A128(aes::Aes128),
    A192(aes::Aes192),
    A256(aes::Aes256),
}

impl Aes {
    fn nova(chave: &[u8]) -> Result<Aes, Falha> {
        let erro = |_| falha("DataError", "AES key data must be 128 or 256 bits");
        Ok(match chave.len() {
            16 => Aes::A128(aes::Aes128::new_from_slice(chave).map_err(erro)?),
            24 => Aes::A192(aes::Aes192::new_from_slice(chave).map_err(erro)?),
            32 => Aes::A256(aes::Aes256::new_from_slice(chave).map_err(erro)?),
            _ => return Err(falha("DataError", "AES key data must be 128 or 256 bits")),
        })
    }

    fn cifra(&self, b: [u8; 16]) -> [u8; 16] {
        use aes::cipher::BlockCipherEncrypt;
        let mut bloco = aes::Block::from(b);
        match self {
            Aes::A128(c) => c.encrypt_block(&mut bloco),
            Aes::A192(c) => c.encrypt_block(&mut bloco),
            Aes::A256(c) => c.encrypt_block(&mut bloco),
        }
        bloco.0
    }

    fn decifra(&self, b: [u8; 16]) -> [u8; 16] {
        use aes::cipher::BlockCipherDecrypt;
        let mut bloco = aes::Block::from(b);
        match self {
            Aes::A128(c) => c.decrypt_block(&mut bloco),
            Aes::A192(c) => c.decrypt_block(&mut bloco),
            Aes::A256(c) => c.decrypt_block(&mut bloco),
        }
        bloco.0
    }
}

fn bloco16(b: &[u8]) -> [u8; 16] {
    let mut r = [0u8; 16];
    r.copy_from_slice(b);
    r
}

fn xor16(a: [u8; 16], b: [u8; 16]) -> [u8; 16] {
    std::array::from_fn(|i| a[i] ^ b[i])
}

/// AES-CBC com preenchimento PKCS#7.
fn aes_cbc(chave: &[u8], iv: &[u8], dados: &[u8], cifrar: bool) -> Result<Vec<u8>, Falha> {
    let aes = Aes::nova(chave)?;
    if iv.len() != 16 {
        return Err(falha("OperationError", "The \"iv\" has an unexpected length -- must be 16 bytes"));
    }
    let mut anterior = bloco16(iv);
    if cifrar {
        let pad = 16 - dados.len() % 16;
        let mut v = dados.to_vec();
        v.resize(dados.len() + pad, pad as u8);
        for bloco in v.chunks_exact_mut(16) {
            anterior = aes.cifra(xor16(bloco16(bloco), anterior));
            bloco.copy_from_slice(&anterior);
        }
        return Ok(v);
    }
    if dados.is_empty() || !dados.len().is_multiple_of(16) {
        return Err(operacao());
    }
    let mut v = Vec::with_capacity(dados.len());
    for bloco in dados.chunks_exact(16) {
        let c = bloco16(bloco);
        v.extend_from_slice(&xor16(aes.decifra(c), anterior));
        anterior = c;
    }
    // preenchimento conferido sem desvio por byte
    let n = v.len();
    let pad = v[n - 1];
    let mut ruim = u8::from(pad == 0) | u8::from(pad > 16);
    for i in 0..16 {
        ruim |= u8::from(i < pad as usize) & u8::from(v[n - 1 - i] != pad);
    }
    if ruim != 0 {
        return Err(operacao());
    }
    v.truncate(n - pad as usize);
    Ok(v)
}

/// AES-CTR do Web Crypto: só os `bits` da direita do bloco contam (e dão a volta).
fn aes_ctr(chave: &[u8], contador: &[u8], bits: u32, dados: &[u8]) -> Result<Vec<u8>, Falha> {
    let aes = Aes::nova(chave)?;
    if contador.len() != 16 {
        return Err(falha("OperationError", "The \"counter\" has an unexpected length -- must be 16 bytes"));
    }
    if !(1..=128).contains(&bits) {
        return Err(falha("OperationError", "The \"length\" member must be >= 1 and <= 128"));
    }
    let inicial = u128::from_be_bytes(bloco16(contador));
    let mascara = if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 };
    if bits < 128 && dados.len().div_ceil(16) as u128 > 1u128 << bits {
        return Err(falha("DataError", "The input is too large for the counter length."));
    }
    let fixo = inicial & !mascara;
    let mut c = inicial & mascara;
    let mut v = dados.to_vec();
    for pedaco in v.chunks_mut(16) {
        let fluxo = aes.cifra((fixo | c).to_be_bytes());
        for (a, k) in pedaco.iter_mut().zip(fluxo) {
            *a ^= k;
        }
        c = c.wrapping_add(1) & mascara;
    }
    Ok(v)
}

/// AES-GCM (NIST SP 800-38D) com IV de qualquer tamanho e tag de 4 a 16 bytes.
fn aes_gcm(chave: &[u8], iv: &[u8], aad: &[u8], tag_bits: u32, dados: &[u8], cifrar: bool) -> Result<Vec<u8>, Falha> {
    use ghash::universal_hash::UniversalHash;
    if ![32, 64, 96, 104, 112, 120, 128].contains(&tag_bits) {
        return Err(falha(
            "OperationError",
            "The tag length is invalid: Must be 32, 64, 96, 104, 112, 120, or 128 bits",
        ));
    }
    let tag = tag_bits as usize / 8;
    let aes = Aes::nova(chave)?;
    if !cifrar && dados.len() < tag {
        return Err(falha("OperationError", "The provided data is too small"));
    }
    if iv.is_empty() {
        return Err(operacao());
    }
    let ghash = ghash::GHash::new(&ghash::Key::from(aes.cifra([0; 16])));
    let j0 = if iv.len() == 12 {
        let mut b = [0u8; 16];
        b[..12].copy_from_slice(iv);
        b[15] = 1;
        b
    } else {
        let mut g = ghash.clone();
        g.update_padded(iv);
        let mut b = [0u8; 16];
        b[8..].copy_from_slice(&(iv.len() as u64 * 8).to_be_bytes());
        g.update(&[ghash::Block::from(b)]);
        g.finalize().0
    };
    // GCTR a partir de inc32(J0)
    let gctr = |entrada: &[u8]| {
        let mut v = entrada.to_vec();
        let mut cb = j0;
        for pedaco in v.chunks_mut(16) {
            let n = u32::from_be_bytes([cb[12], cb[13], cb[14], cb[15]]).wrapping_add(1);
            cb[12..].copy_from_slice(&n.to_be_bytes());
            for (a, k) in pedaco.iter_mut().zip(aes.cifra(cb)) {
                *a ^= k;
            }
        }
        v
    };
    let calcula_tag = |cifrado: &[u8]| {
        let mut g = ghash.clone();
        g.update_padded(aad);
        g.update_padded(cifrado);
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&(aad.len() as u64 * 8).to_be_bytes());
        b[8..].copy_from_slice(&(cifrado.len() as u64 * 8).to_be_bytes());
        g.update(&[ghash::Block::from(b)]);
        xor16(g.finalize().0, aes.cifra(j0))
    };
    if cifrar {
        let mut v = gctr(dados);
        let t = calcula_tag(&v);
        v.extend_from_slice(&t[..tag]);
        return Ok(v);
    }
    let (cifrado, recebida) = dados.split_at(dados.len() - tag);
    if !iguais(&calcula_tag(cifrado)[..tag], recebida) {
        return Err(operacao());
    }
    Ok(gctr(cifrado))
}

const KW_IV: [u8; 8] = [0xA6; 8];

/// AES Key Wrap (RFC 3394), com o IV padrão.
fn aes_kw(chave: &[u8], dados: &[u8], embrulhar: bool) -> Result<Vec<u8>, Falha> {
    let aes = Aes::nova(chave)?;
    if dados.len() < if embrulhar { 16 } else { 24 } {
        return Err(falha("OperationError", "The provided data is too small"));
    }
    if !dados.len().is_multiple_of(8) {
        return Err(falha("DataError", "The AES-KW input data length is invalid: not a multiple of 8 bytes"));
    }
    let junta = |a: [u8; 8], r: &[u8]| {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&a);
        b[8..].copy_from_slice(r);
        b
    };
    if embrulhar {
        let n = dados.len() / 8;
        let mut a = KW_IV;
        let mut r = dados.to_vec();
        for j in 0..6 {
            for i in 0..n {
                let b = aes.cifra(junta(a, &r[i * 8..i * 8 + 8]));
                let t = (n * j + i + 1) as u64;
                for (k, x) in t.to_be_bytes().into_iter().enumerate() {
                    a[k] = b[k] ^ x;
                }
                r[i * 8..i * 8 + 8].copy_from_slice(&b[8..]);
            }
        }
        let mut v = a.to_vec();
        v.extend_from_slice(&r);
        return Ok(v);
    }
    let n = dados.len() / 8 - 1;
    let mut a: [u8; 8] = dados[..8].try_into().expect("8 bytes");
    let mut r = dados[8..].to_vec();
    for j in (0..6).rev() {
        for i in (0..n).rev() {
            let t = (n * j + i + 1) as u64;
            for (k, x) in t.to_be_bytes().into_iter().enumerate() {
                a[k] ^= x;
            }
            let b = aes.decifra(junta(a, &r[i * 8..i * 8 + 8]));
            a.copy_from_slice(&b[..8]);
            r[i * 8..i * 8 + 8].copy_from_slice(&b[8..]);
        }
    }
    if !iguais(&a, &KW_IV) {
        return Err(operacao());
    }
    Ok(r)
}

// --------------------------------------------------------------- DER

const OID_EC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
const OID_P256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
const OID_P384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.34");
const OID_P521: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.35");
const OID_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const OID_ED25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.112");
const OID_X25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.110");

fn spki_de(der: &[u8]) -> Result<SubjectPublicKeyInfoRef<'_>, Falha> {
    SubjectPublicKeyInfoRef::from_der(der).map_err(|_| dados())
}

fn pkcs8_de(der: &[u8]) -> Result<PrivateKeyInfoRef<'_>, Falha> {
    PrivateKeyInfoRef::from_der(der).map_err(|_| dados())
}

// -------------------------------------------------------------------- EC

#[derive(Clone, Copy, Debug, PartialEq)]
enum Curva {
    P256,
    P384,
}

impl Curva {
    fn de(nome: &str) -> Result<Curva, Falha> {
        match nome {
            "P-256" => Ok(Curva::P256),
            "P-384" => Ok(Curva::P384),
            _ => Err(nao_suportado()),
        }
    }

    fn oid(self) -> ObjectIdentifier {
        match self {
            Curva::P256 => OID_P256,
            Curva::P384 => OID_P384,
        }
    }
}

/// Confere o AlgorithmIdentifier de uma chave EC (id-ecPublicKey + curva pedida).
fn ec_algoritmo(alg: &p256::pkcs8::AlgorithmIdentifierRef<'_>, curva: Curva) -> Result<(), Falha> {
    if alg.oid != OID_EC {
        return Err(dados());
    }
    let oid = alg.parameters_oid().map_err(|_| dados())?;
    if oid == curva.oid() {
        Ok(())
    } else if [OID_P256, OID_P384, OID_P521].contains(&oid) {
        Err(falha("DataError", "The imported EC key specifies a different curve than requested"))
    } else {
        Err(dados())
    }
}

/// As operações de uma curva NIST (P-256 ou P-384); `d` é o escalar e
/// `publica` o ponto SEC1 descomprimido.
macro_rules! curva_nist {
    ($m:ident, $c:ident) => {
        mod $m {
            use super::{Falha, Hash, dados, falha, operacao, resumo, rng};
            use $c::ecdsa::signature::hazmat::{PrehashVerifier, RandomizedPrehashSigner};
            use $c::ecdsa::{Signature, SigningKey, VerifyingKey};
            use $c::elliptic_curve::Generate;
            use $c::elliptic_curve::sec1::ToSec1Point;
            use $c::pkcs8::EncodePrivateKey;
            use $c::pkcs8::spki::EncodePublicKey;
            use $c::{PublicKey, SecretKey};

            fn ponto(pk: &PublicKey) -> Vec<u8> {
                pk.to_sec1_point(false).as_bytes().to_vec()
            }

            fn secreta(d: &[u8]) -> Result<SecretKey, Falha> {
                SecretKey::from_slice(d).map_err(|_| operacao())
            }

            pub fn gerar() -> (Vec<u8>, Vec<u8>) {
                let sk = SecretKey::generate_from_rng(&mut rng());
                (sk.to_bytes().to_vec(), ponto(&sk.public_key()))
            }

            /// Ponto SEC1 (comprimido ou não) → descomprimido, validado.
            pub fn publica(bytes: &[u8]) -> Result<Vec<u8>, Falha> {
                PublicKey::from_sec1_bytes(bytes).map(|pk| ponto(&pk)).map_err(|_| dados())
            }

            /// JWK: o par (x, y) e, na privada, `d` que precisa bater com ele.
            pub fn jwk(xy: &[u8], d: Option<&[u8]>) -> Result<(Option<Vec<u8>>, Vec<u8>), Falha> {
                let mut sec1 = vec![4u8];
                sec1.extend_from_slice(xy);
                let pk = PublicKey::from_sec1_bytes(&sec1).map_err(|_| dados())?;
                let Some(d) = d else { return Ok((None, ponto(&pk))) };
                let sk = secreta(d)?;
                if sk.public_key() != pk {
                    return Err(falha("DataError", "The imported EC key is invalid"));
                }
                Ok((Some(sk.to_bytes().to_vec()), ponto(&pk)))
            }

            pub fn de_pkcs8(sec1_der: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Falha> {
                let sk = SecretKey::from_sec1_der(sec1_der).map_err(|_| dados())?;
                Ok((sk.to_bytes().to_vec(), ponto(&sk.public_key())))
            }

            pub fn para_pkcs8(d: &[u8]) -> Result<Vec<u8>, Falha> {
                let der = secreta(d)?.to_pkcs8_der().map_err(|_| operacao())?;
                Ok(der.as_bytes().to_vec())
            }

            pub fn para_spki(publica: &[u8]) -> Result<Vec<u8>, Falha> {
                let pk = PublicKey::from_sec1_bytes(publica).map_err(|_| operacao())?;
                Ok(pk.to_public_key_der().map_err(|_| operacao())?.as_bytes().to_vec())
            }

            /// O hash é o do algoritmo (não o "natural" da curva); a crate
            /// trunca ou completa o resumo como manda o ECDSA.
            pub fn assina(h: Hash, d: &[u8], dados: &[u8]) -> Result<Vec<u8>, Falha> {
                let sk = SigningKey::from_slice(d).map_err(|_| operacao())?;
                let sig: Signature = sk.sign_prehash_with_rng(&mut rng(), &resumo(h, dados)).map_err(|_| operacao())?;
                Ok(sig.to_bytes().to_vec())
            }

            pub fn verifica(h: Hash, publica: &[u8], dados: &[u8], assinatura: &[u8]) -> bool {
                let (Ok(vk), Ok(sig)) = (VerifyingKey::from_sec1_bytes(publica), Signature::from_slice(assinatura)) else {
                    return false;
                };
                vk.verify_prehash(&resumo(h, dados), &sig).is_ok()
            }

            pub fn ecdh(d: &[u8], publica: &[u8]) -> Result<Vec<u8>, Falha> {
                let pk = PublicKey::from_sec1_bytes(publica).map_err(|_| operacao())?;
                Ok(secreta(d)?.diffie_hellman(&pk).raw_secret_bytes().to_vec())
            }
        }
    };
}

curva_nist!(p256c, p256);
curva_nist!(p384c, p384);

/// Despacha para a curva certa.
macro_rules! na_curva {
    ($curva:expr, $m:ident => $e:expr) => {
        match $curva {
            Curva::P256 => {
                use p256c as $m;
                $e
            }
            Curva::P384 => {
                use p384c as $m;
                $e
            }
        }
    };
}

fn ec_spki(curva: Curva, der: &[u8]) -> Result<Vec<u8>, Falha> {
    let info = spki_de(der)?;
    ec_algoritmo(&info.algorithm, curva)?;
    let bytes = info.subject_public_key.as_bytes().ok_or_else(dados)?;
    na_curva!(curva, m => m::publica(bytes))
}

fn ec_pkcs8(curva: Curva, der: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Falha> {
    let info = pkcs8_de(der)?;
    ec_algoritmo(&info.algorithm, curva)?;
    na_curva!(curva, m => m::de_pkcs8(info.private_key.as_bytes()))
}

// ------------------------------------------------------------------- RSA

/// Componentes de uma chave RSA, em big-endian mínimo (como no JWK):
/// `[n, e]` na pública, `[n, e, d, p, q, dp, dq, qi]` na privada.
type Componentes = Vec<Vec<u8>>;

fn uint(bytes: &[u8], bits: u32) -> Result<BoxedUint, Falha> {
    BoxedUint::from_be_slice(bytes, bits.max(8)).map_err(|_| dados())
}

fn minimo(x: &BoxedUint) -> Vec<u8> {
    let b = x.to_be_bytes_trimmed_vartime().to_vec();
    if b.is_empty() { vec![0] } else { b }
}

fn sem_zeros(b: &[u8]) -> &[u8] {
    let i = b.iter().position(|&x| x != 0).unwrap_or(b.len().saturating_sub(1));
    &b[i..]
}

/// Chave pública com as checagens do BoringSSL (até 16384 bits, e ímpar de até 33 bits).
fn rsa_publica(n: &[u8], e: &[u8]) -> Result<rsa::PublicKey, Falha> {
    let (n, e) = (sem_zeros(n), sem_zeros(e));
    let bits = (n.len() * 8) as u32;
    rsa::PublicKey::new_with_size_limits(uint(n, bits)?, uint(e, (e.len() * 8) as u32)?, 1..=16384).map_err(|_| dados())
}

/// Chave privada a partir dos componentes; os de CRT dados (dp, dq, qi)
/// precisam bater com os calculados (o `RSA_check_key` do Chrome).
fn rsa_privada(c: &[Vec<u8>]) -> Result<rsa::PrivateKey, Falha> {
    if c.len() != 8 {
        return Err(dados());
    }
    let n = sem_zeros(&c[0]);
    let bits = (n.len() * 8) as u32;
    let e = sem_zeros(&c[1]);
    let chave = rsa::PrivateKey::from_components(
        uint(n, bits)?,
        uint(e, (e.len() * 8) as u32)?,
        uint(sem_zeros(&c[2]), bits)?,
        vec![uint(sem_zeros(&c[3]), bits)?, uint(sem_zeros(&c[4]), bits)?],
    )
    .map_err(|_| dados())?;
    let calculados = rsa_componentes(&chave)?;
    if (5..8).any(|i| sem_zeros(&c[i]) != calculados[i].as_slice()) {
        return Err(dados());
    }
    Ok(chave)
}

fn rsa_componentes(k: &rsa::PrivateKey) -> Result<Componentes, Falha> {
    let primos = k.primes();
    let (Some(dp), Some(dq), Some(qi), [p, q]) = (k.dp(), k.dq(), k.crt_coefficient(), primos) else {
        return Err(operacao());
    };
    Ok(vec![
        minimo(k.n().as_ref()),
        minimo(k.e()),
        minimo(k.d()),
        minimo(p.as_ref()),
        minimo(q.as_ref()),
        minimo(dp),
        minimo(dq),
        minimo(&qi),
    ])
}

/// Geração de chave; abaixo de 1024 bits (o Chrome aceita desde 256) os
/// primos saem do crypto-primes direto, pois a crate rsa recusa.
fn rsa_gerar(bits: usize, e: u64) -> Result<Componentes, Falha> {
    let exp = BoxedUint::from(e);
    let chave = if bits >= 1024 {
        rsa::PrivateKey::new_with_exp(&mut rng(), bits, exp).map_err(|_| operacao())?
    } else {
        let mut r = rng();
        loop {
            let p: BoxedUint = crypto_primes::random_prime(&mut r, crypto_primes::Flavor::Any, (bits / 2) as u32);
            let q: BoxedUint =
                crypto_primes::random_prime(&mut r, crypto_primes::Flavor::Any, (bits - bits / 2) as u32);
            if p == q {
                continue;
            }
            match rsa::PrivateKey::from_p_q(p, q, exp.clone()) {
                Ok(k) if k.n().as_ref().bit_width() as usize == bits => break k,
                _ => continue,
            }
        }
    };
    rsa_componentes(&chave)
}

fn rsa_algoritmo(alg: &p256::pkcs8::AlgorithmIdentifierRef<'_>) -> Result<(), Falha> {
    // rsaEncryption, com parâmetros NULL (ou ausentes, como o BoringSSL aceita)
    let nulo = alg.parameters.is_none_or(|p| p.tag() == p256::pkcs8::der::Tag::Null && p.value().is_empty());
    if alg.oid == OID_RSA && nulo { Ok(()) } else { Err(dados()) }
}

fn rsa_spki(der: &[u8]) -> Result<Componentes, Falha> {
    let info = spki_de(der)?;
    rsa_algoritmo(&info.algorithm)?;
    let bytes = info.subject_public_key.as_bytes().ok_or_else(dados)?;
    let k = rsa::pkcs1::RsaPublicKeyRef::from_der(bytes).map_err(|_| dados())?;
    let (n, e) = (k.modulus.as_bytes().to_vec(), k.public_exponent.as_bytes().to_vec());
    rsa_publica(&n, &e)?;
    Ok(vec![n, e])
}

fn rsa_pkcs8(der: &[u8]) -> Result<Componentes, Falha> {
    let info = pkcs8_de(der)?;
    rsa_algoritmo(&info.algorithm)?;
    let k = rsa::pkcs1::RsaPrivateKeyRef::from_der(info.private_key.as_bytes()).map_err(|_| dados())?;
    if k.other_prime_infos.is_some() {
        return Err(dados());
    }
    let c: Componentes =
        [k.modulus, k.public_exponent, k.private_exponent, k.prime1, k.prime2, k.exponent1, k.exponent2, k.coefficient]
            .iter()
            .map(|u| u.as_bytes().to_vec())
            .collect();
    rsa_privada(&c)?;
    Ok(c)
}

fn rsa_uint(b: &[u8]) -> Result<rsa::pkcs1::UintRef<'_>, Falha> {
    rsa::pkcs1::UintRef::new(b).map_err(|_| operacao())
}

fn rsa_para_spki(c: &[Vec<u8>]) -> Result<Vec<u8>, Falha> {
    let k = rsa::pkcs1::RsaPublicKey { modulus: rsa_uint(&c[0])?, public_exponent: rsa_uint(&c[1])? };
    let interna = k.to_der().map_err(|_| operacao())?;
    let info = SubjectPublicKeyInfoRef {
        algorithm: rsa::pkcs1::ALGORITHM_ID,
        subject_public_key: p256::pkcs8::der::asn1::BitStringRef::new(0, &interna).map_err(|_| operacao())?,
    };
    info.to_der().map_err(|_| operacao())
}

fn rsa_para_pkcs8(c: &[Vec<u8>]) -> Result<Vec<u8>, Falha> {
    if c.len() != 8 {
        return Err(operacao());
    }
    let k = rsa::pkcs1::RsaPrivateKey {
        modulus: rsa_uint(&c[0])?,
        public_exponent: rsa_uint(&c[1])?,
        private_exponent: rsa_uint(&c[2])?,
        prime1: rsa_uint(&c[3])?,
        prime2: rsa_uint(&c[4])?,
        exponent1: rsa_uint(&c[5])?,
        exponent2: rsa_uint(&c[6])?,
        coefficient: rsa_uint(&c[7])?,
        other_prime_infos: None,
    };
    let interna = k.to_der().map_err(|_| operacao())?;
    let octetos = p256::pkcs8::der::asn1::OctetStringRef::new(&interna).map_err(|_| operacao())?;
    let info = PrivateKeyInfoRef::new(rsa::pkcs1::ALGORITHM_ID, octetos);
    info.to_der().map_err(|_| operacao())
}

/// Confere componentes vindos do JWK (a pública só precisa de n e e).
fn rsa_jwk(c: &[Vec<u8>]) -> Result<(), Falha> {
    if c.len() == 2 {
        // o Chrome dá OperationError quando n/e do JWK não formam uma chave
        return rsa_publica(&c[0], &c[1]).map(|_| ()).map_err(|_| operacao());
    }
    rsa_privada(c).map(|_| ())
}

#[derive(Clone, Copy, PartialEq)]
enum Esquema {
    Pkcs1,
    Pss(usize),
}

fn rsa_assina(c: &[Vec<u8>], esquema: Esquema, h: Hash, dados: &[u8]) -> Result<Vec<u8>, Falha> {
    let k = rsa_privada(c)?;
    let resumo = resumo(h, dados);
    com_hash!(h, D => match esquema {
        Esquema::Pkcs1 => k.sign_with_rng(&mut rng(), rsa::Pkcs1v15Sign::new::<D>(), &resumo),
        Esquema::Pss(sal) => k.sign_with_rng(&mut rng(), rsa::Pss::<D>::new_with_salt(sal), &resumo),
    })
    .map_err(|_| operacao())
}

fn rsa_verifica(c: &[Vec<u8>], esquema: Esquema, h: Hash, dados: &[u8], assinatura: &[u8]) -> bool {
    let Ok(k) = rsa_publica(&c[0], &c[1]) else { return false };
    let resumo = resumo(h, dados);
    com_hash!(h, D => match esquema {
        Esquema::Pkcs1 => k.verify(rsa::Pkcs1v15Sign::new::<D>(), &resumo, assinatura),
        Esquema::Pss(sal) => k.verify(rsa::Pss::<D>::new_with_salt(sal), &resumo, assinatura),
    })
    .is_ok()
}

fn rsa_oaep(c: &[Vec<u8>], h: Hash, rotulo: &[u8], dados: &[u8], cifrar: bool) -> Result<Vec<u8>, Falha> {
    let rotulo = rotulo.to_vec().into_boxed_slice();
    if cifrar {
        let k = rsa_publica(&c[0], &c[1]).map_err(|_| operacao())?;
        com_hash!(h, D => k.encrypt(&mut rng(), rsa::Oaep::<D>::new_with_label(rotulo), dados)).map_err(|_| operacao())
    } else {
        let k = rsa_privada(c).map_err(|_| operacao())?;
        com_hash!(h, D => k.decrypt_blinded(&mut rng(), rsa::Oaep::<D>::new_with_label(rotulo), dados))
            .map_err(|_| operacao())
    }
}

// ----------------------------------------------------- Ed25519 e X25519

#[derive(Clone, Copy, PartialEq)]
enum Okp {
    Ed25519,
    X25519,
}

impl Okp {
    fn oid(self) -> ObjectIdentifier {
        match self {
            Okp::Ed25519 => OID_ED25519,
            Okp::X25519 => OID_X25519,
        }
    }

    fn byte_oid(self) -> u8 {
        match self {
            Okp::Ed25519 => 0x70,
            Okp::X25519 => 0x6e,
        }
    }
}

fn b32(b: &[u8]) -> Result<[u8; 32], Falha> {
    b.try_into().map_err(|_| dados())
}

fn okp_publica(alg: Okp, privada: &[u8]) -> Result<Vec<u8>, Falha> {
    let s = b32(privada)?;
    Ok(match alg {
        Okp::Ed25519 => ed25519_dalek::SigningKey::from_bytes(&s).verifying_key().to_bytes().to_vec(),
        Okp::X25519 => curve25519_dalek::MontgomeryPoint::mul_base_clamped(s).to_bytes().to_vec(),
    })
}

fn okp_spki(alg: Okp, der: &[u8]) -> Result<Vec<u8>, Falha> {
    let info = spki_de(der)?;
    if info.algorithm.oid != alg.oid() || info.algorithm.parameters.is_some() {
        return Err(dados());
    }
    let bytes = info.subject_public_key.as_bytes().ok_or_else(dados)?;
    Ok(b32(bytes)?.to_vec())
}

fn okp_pkcs8(alg: Okp, der: &[u8]) -> Result<Vec<u8>, Falha> {
    let info = pkcs8_de(der)?;
    if info.algorithm.oid != alg.oid() || info.algorithm.parameters.is_some() {
        return Err(dados());
    }
    // CurvePrivateKey ::= OCTET STRING (RFC 8410)
    let interna =
        <&p256::pkcs8::der::asn1::OctetStringRef>::from_der(info.private_key.as_bytes()).map_err(|_| dados())?;
    Ok(b32(interna.as_bytes())?.to_vec())
}

fn okp_para_spki(alg: Okp, publica: &[u8]) -> Vec<u8> {
    let mut v = vec![0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, alg.byte_oid(), 0x03, 0x21, 0x00];
    v.extend_from_slice(publica);
    v
}

fn okp_para_pkcs8(alg: Okp, privada: &[u8]) -> Vec<u8> {
    let mut v =
        vec![0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, alg.byte_oid(), 0x04, 0x22, 0x04, 0x20];
    v.extend_from_slice(privada);
    v
}

fn ed25519_assina(privada: &[u8], dados: &[u8]) -> Result<Vec<u8>, Falha> {
    use ed25519_dalek::Signer;
    let sk = ed25519_dalek::SigningKey::from_bytes(&b32(privada)?);
    Ok(sk.sign(dados).to_bytes().to_vec())
}

fn ed25519_verifica(publica: &[u8], dados: &[u8], assinatura: &[u8]) -> bool {
    use ed25519_dalek::Verifier;
    let (Ok(pk), Ok(sig)) = (
        b32(publica).map(|b| ed25519_dalek::VerifyingKey::from_bytes(&b)),
        ed25519_dalek::Signature::from_slice(assinatura),
    ) else {
        return false;
    };
    pk.is_ok_and(|pk| pk.verify(dados, &sig).is_ok())
}

fn x25519(privada: &[u8], publica: &[u8]) -> Result<Vec<u8>, Falha> {
    let s = curve25519_dalek::MontgomeryPoint(b32(publica)?).mul_clamped(b32(privada)?).to_bytes();
    // ponto de ordem pequena: o segredo seria zero (o BoringSSL recusa)
    if s == [0u8; 32] {
        return Err(operacao());
    }
    Ok(s.to_vec())
}

// ----------------------------------------------------- funções nativas

fn erro(vm: &mut Vm, f: Falha) -> Value {
    super::dom::dom_error(vm, f.0, &f.1)
}

fn arg_bytes(a: &CallArgs, i: usize) -> Vec<u8> {
    bytes_of(&a.arg(i)).unwrap_or_default()
}

fn arg_texto(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<String, Value> {
    vm.to_rust_string(&a.arg(i))
}

fn arg_num(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<f64, Value> {
    vm.to_number(&a.arg(i))
}

fn arg_hash(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<Hash, Value> {
    let nome = arg_texto(vm, a, i)?;
    Hash::de(&nome).map_err(|f| erro(vm, f))
}

/// Uma lista JS de Uint8Array (componentes RSA).
fn arg_lista(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<Componentes, Value> {
    let v = a.arg(i);
    let n = vm.get_str(&v, "length")?;
    let n = vm.to_number(&n)? as usize;
    let mut out = Vec::with_capacity(n);
    for k in 0..n.min(16) {
        let x = vm.get_index(&v, &Value::Number(k as f64))?;
        out.push(bytes_of(&x).unwrap_or_default());
    }
    Ok(out)
}

fn bytes(vm: &mut Vm, r: Result<Vec<u8>, Falha>) -> R {
    match r {
        Ok(b) => uint8_array(vm, &b),
        Err(f) => Err(erro(vm, f)),
    }
}

fn lista(vm: &mut Vm, itens: Vec<Option<Vec<u8>>>) -> R {
    let mut vals = Vec::with_capacity(itens.len());
    for b in itens {
        vals.push(match b {
            Some(b) => uint8_array(vm, &b)?,
            None => Value::Null,
        });
    }
    Ok(vm.array_value(vals))
}

/// `cryptoRandom(n)`: bytes do gerador do sistema.
fn random(vm: &mut Vm, a: &mut CallArgs) -> R {
    // o limite de 65536 do getRandomValues fica no prelúdio; chaves HMAC podem ser maiores
    let n = arg_num(vm, a, 0)?.max(0.0) as usize;
    uint8_array(vm, &aleatorio(n))
}

/// `cryptoDigest(hash, dados)`.
fn digest_nativo(vm: &mut Vm, a: &mut CallArgs) -> R {
    let h = arg_hash(vm, a, 0)?;
    uint8_array(vm, &resumo(h, &arg_bytes(a, 1)))
}

/// `cryptoHmac(hash, chave, dados[, assinatura])`: o MAC, ou se confere.
fn hmac_nativo(vm: &mut Vm, a: &mut CallArgs) -> R {
    let h = arg_hash(vm, a, 0)?;
    let mac = hmac(h, &arg_bytes(a, 1), &arg_bytes(a, 2));
    if a.arg(3).is_undefined() {
        return uint8_array(vm, &mac);
    }
    Ok(Value::Bool(iguais(&mac, &arg_bytes(a, 3))))
}

/// `cryptoPbkdf2(hash, senha, sal, iterações, bytes)`.
fn pbkdf2_nativo(vm: &mut Vm, a: &mut CallArgs) -> R {
    let h = arg_hash(vm, a, 0)?;
    let iteracoes = arg_num(vm, a, 3)? as u32;
    let n = arg_num(vm, a, 4)? as usize;
    uint8_array(vm, &pbkdf2(h, &arg_bytes(a, 1), &arg_bytes(a, 2), iteracoes, n))
}

/// `cryptoHkdf(hash, chave, sal, info, bytes)`.
fn hkdf_nativo(vm: &mut Vm, a: &mut CallArgs) -> R {
    let h = arg_hash(vm, a, 0)?;
    let n = arg_num(vm, a, 4)? as usize;
    let r = hkdf(h, &arg_bytes(a, 1), &arg_bytes(a, 2), &arg_bytes(a, 3), n);
    bytes(vm, r)
}

/// `cryptoAes(modo, chave, dados, a, b, c)`; modos: cbc-e/cbc-d (a = iv),
/// ctr (a = contador, b = bits), gcm-e/gcm-d (a = iv, b = aad, c = bits da
/// tag) e kw-e/kw-d.
fn aes_nativo(vm: &mut Vm, a: &mut CallArgs) -> R {
    let modo = arg_texto(vm, a, 0)?;
    let (chave, dados) = (arg_bytes(a, 1), arg_bytes(a, 2));
    let r = match modo.as_str() {
        "cbc-e" | "cbc-d" => aes_cbc(&chave, &arg_bytes(a, 3), &dados, modo == "cbc-e"),
        "ctr" => {
            let bits = arg_num(vm, a, 4)? as u32;
            aes_ctr(&chave, &arg_bytes(a, 3), bits, &dados)
        }
        "gcm-e" | "gcm-d" => {
            let tag = arg_num(vm, a, 5)? as u32;
            aes_gcm(&chave, &arg_bytes(a, 3), &arg_bytes(a, 4), tag, &dados, modo == "gcm-e")
        }
        "kw-e" | "kw-d" => aes_kw(&chave, &dados, modo == "kw-e"),
        _ => Err(nao_suportado()),
    };
    bytes(vm, r)
}

/// `cryptoEc(op, curva, a, b, c, d)`.
fn ec_nativo(vm: &mut Vm, a: &mut CallArgs) -> R {
    let op = arg_texto(vm, a, 0)?;
    let nome = arg_texto(vm, a, 1)?;
    let curva = Curva::de(&nome).map_err(|f| erro(vm, f))?;
    let par = |r: Result<(Vec<u8>, Vec<u8>), Falha>| r.map(|(d, p)| vec![Some(d), Some(p)]);
    let r = match op.as_str() {
        "gerar" => Ok(na_curva!(curva, m => { let (d, p) = m::gerar(); vec![Some(d), Some(p)] })),
        "pkcs8" => par(ec_pkcs8(curva, &arg_bytes(a, 2))),
        "jwk" => {
            let d = (!matches!(a.arg(3), Value::Null)).then(|| arg_bytes(a, 3));
            na_curva!(curva, m => m::jwk(&arg_bytes(a, 2), d.as_deref())).map(|(d, p)| vec![d, Some(p)])
        }
        "assina" | "verifica" => {
            let h = arg_hash(vm, a, 2)?;
            let (chave, dados) = (arg_bytes(a, 3), arg_bytes(a, 4));
            if op == "verifica" {
                let ok = na_curva!(curva, m => m::verifica(h, &chave, &dados, &arg_bytes(a, 5)));
                return Ok(Value::Bool(ok));
            }
            let r = na_curva!(curva, m => m::assina(h, &chave, &dados));
            return bytes(vm, r);
        }
        _ => {
            let x = arg_bytes(a, 2);
            let r = match op.as_str() {
                "raw" => na_curva!(curva, m => m::publica(&x)),
                "spki" => ec_spki(curva, &x),
                "para-spki" => na_curva!(curva, m => m::para_spki(&x)),
                "para-pkcs8" => na_curva!(curva, m => m::para_pkcs8(&x)),
                "ecdh" => na_curva!(curva, m => m::ecdh(&x, &arg_bytes(a, 3))),
                _ => Err(nao_suportado()),
            };
            return bytes(vm, r);
        }
    };
    match r {
        Ok(itens) => lista(vm, itens),
        Err(f) => Err(erro(vm, f)),
    }
}

/// `cryptoRsa(op, ...)`: gerar(bits, e), spki/pkcs8(der), jwk(componentes),
/// para-spki/para-pkcs8(componentes), assina(comp, esquema, hash, dados, sal),
/// verifica(comp, esquema, hash, dados, sal, assinatura), oaep-e/oaep-d(comp,
/// hash, rótulo, dados).
fn rsa_nativo(vm: &mut Vm, a: &mut CallArgs) -> R {
    let op = arg_texto(vm, a, 0)?;
    let componentes = |r: Result<Componentes, Falha>| r.map(|c| c.into_iter().map(Some).collect::<Vec<_>>());
    let r = match op.as_str() {
        "gerar" => {
            let bits = arg_num(vm, a, 1)? as usize;
            let e = arg_num(vm, a, 2)? as u64;
            componentes(rsa_gerar(bits, e))
        }
        "spki" => componentes(rsa_spki(&arg_bytes(a, 1))),
        "pkcs8" => componentes(rsa_pkcs8(&arg_bytes(a, 1))),
        "jwk" => {
            let c = arg_lista(vm, a, 1)?;
            return match rsa_jwk(&c) {
                Ok(()) => Ok(Value::Undefined),
                Err(f) => Err(erro(vm, f)),
            };
        }
        _ => {
            let c = arg_lista(vm, a, 1)?;
            let r = match op.as_str() {
                "para-spki" => rsa_para_spki(&c),
                "para-pkcs8" => rsa_para_pkcs8(&c),
                "assina" | "verifica" => {
                    let esquema = arg_texto(vm, a, 2)?;
                    let h = arg_hash(vm, a, 3)?;
                    let sal = arg_num(vm, a, 5)? as usize;
                    let esquema = if esquema == "pss" { Esquema::Pss(sal) } else { Esquema::Pkcs1 };
                    let dados = arg_bytes(a, 4);
                    if op == "verifica" {
                        return Ok(Value::Bool(rsa_verifica(&c, esquema, h, &dados, &arg_bytes(a, 6))));
                    }
                    rsa_assina(&c, esquema, h, &dados)
                }
                "oaep-e" | "oaep-d" => {
                    let h = arg_hash(vm, a, 2)?;
                    rsa_oaep(&c, h, &arg_bytes(a, 3), &arg_bytes(a, 4), op == "oaep-e")
                }
                _ => Err(nao_suportado()),
            };
            return bytes(vm, r);
        }
    };
    match r {
        Ok(itens) => lista(vm, itens),
        Err(f) => Err(erro(vm, f)),
    }
}

/// `cryptoOkp(op, alg, a, b, c)`: Ed25519 e X25519.
fn okp_nativo(vm: &mut Vm, a: &mut CallArgs) -> R {
    let op = arg_texto(vm, a, 0)?;
    let alg = if arg_texto(vm, a, 1)? == "X25519" { Okp::X25519 } else { Okp::Ed25519 };
    let x = arg_bytes(a, 2);
    if op == "verifica" {
        return Ok(Value::Bool(ed25519_verifica(&x, &arg_bytes(a, 3), &arg_bytes(a, 4))));
    }
    let r = match op.as_str() {
        "gerar" => Ok(aleatorio(32)),
        "publica" => okp_publica(alg, &x),
        "spki" => okp_spki(alg, &x),
        "pkcs8" => okp_pkcs8(alg, &x),
        "para-spki" => Ok(okp_para_spki(alg, &x)),
        "para-pkcs8" => Ok(okp_para_pkcs8(alg, &x)),
        "assina" => ed25519_assina(&x, &arg_bytes(a, 3)),
        "deriva" => x25519(&x, &arg_bytes(a, 3)),
        _ => Err(nao_suportado()),
    };
    bytes(vm, r)
}

pub(crate) fn install(vm: &Vm, n: &ObjRef) {
    let r = &vm.realm;
    for (name, len, f) in [
        ("cryptoRandom", 1, random as ferro_js::value::NativeFn),
        ("cryptoDigest", 2, digest_nativo),
        ("cryptoHmac", 4, hmac_nativo),
        ("cryptoPbkdf2", 5, pbkdf2_nativo),
        ("cryptoHkdf", 5, hkdf_nativo),
        ("cryptoAes", 6, aes_nativo),
        ("cryptoEc", 6, ec_nativo),
        ("cryptoRsa", 7, rsa_nativo),
        ("cryptoOkp", 5, okp_nativo),
    ] {
        r.method(n, name, len, f);
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn x(b: &[u8]) -> String {
        b.iter().map(|v| format!("{v:02x}")).collect()
    }

    #[test]
    fn sha1_e_sha2() {
        assert_eq!(x(&resumo(Hash::Sha1, b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(x(&resumo(Hash::Sha1, b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        let longo = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(x(&resumo(Hash::Sha1, longo)), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
        assert_eq!(x(&resumo(Hash::Sha1, &[b'a'; 1_000_000])), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
        assert_eq!(
            x(&resumo(Hash::Sha256, b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hmac_rfc4231_e_rfc2202() {
        let k = [0x0b; 20];
        assert_eq!(
            x(&hmac(Hash::Sha256, &k, b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            x(&hmac(Hash::Sha256, b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            x(&hmac(Hash::Sha512, &k, b"Hi There")),
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cde\
             daa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
        );
        assert_eq!(x(&hmac(Hash::Sha1, &k, b"Hi There")), "b617318655057264e28bc0b6fb378c8ef146be00");
        // chave maior que o bloco (RFC 4231, caso 6)
        assert_eq!(
            x(&hmac(Hash::Sha256, &[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn hkdf_rfc5869() {
        let sal = h("000102030405060708090a0b0c");
        let info = h("f0f1f2f3f4f5f6f7f8f9");
        assert_eq!(
            x(&hkdf(Hash::Sha256, &[0x0b; 22], &sal, &info, 42).unwrap()),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
        assert_eq!(
            x(&hkdf(Hash::Sha1, &[0x0b; 11], &sal, &info, 42).unwrap()),
            "085a01ea1b10f36933068b56efa5ad81a4f14b822f5b091568a9cdd4f155fda2c22e422478d305f3f896"
        );
        assert_eq!(hkdf(Hash::Sha256, b"k", b"", b"", 0).unwrap(), Vec::<u8>::new());
        assert_eq!(hkdf(Hash::Sha256, b"k", b"", b"", 255 * 32 + 1).unwrap_err().0, "OperationError");
    }

    #[test]
    fn pbkdf2_rfc6070() {
        let p = |s: &[u8], sal: &[u8], c, n| x(&pbkdf2(Hash::Sha1, s, sal, c, n));
        assert_eq!(p(b"password", b"salt", 1, 20), "0c60c80f961f0e71f3a9b524af6012062fe037a6");
        assert_eq!(p(b"password", b"salt", 2, 20), "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957");
        assert_eq!(p(b"password", b"salt", 4096, 20), "4b007901b765489abead49d926f721d065a429c1");
        assert_eq!(
            p(b"passwordPASSWORDpassword", b"saltSALTsaltSALTsaltSALTsaltSALTsalt", 4096, 25),
            "3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038"
        );
        assert_eq!(p(b"pass\0word", b"sa\0lt", 4096, 16), "56fa6aa75548099dcc37d7f03425e0c3");
    }

    #[test]
    fn aes_cbc_ctr_sp800_38a() {
        let k = h("2b7e151628aed2a6abf7158809cf4f3c");
        let iv = h("000102030405060708090a0b0c0d0e0f");
        let p = h("6bc1bee22e409f96e93d7e117393172a ae2d8a571e03ac9c9eb76fac45af8e51");
        let c = aes_cbc(&k, &iv, &p, true).unwrap();
        assert_eq!(x(&c[..32]), "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2");
        assert_eq!(c.len(), 48); // um bloco inteiro de preenchimento
        assert_eq!(aes_cbc(&k, &iv, &c, false).unwrap(), p);
        let mut adulterado = c.clone();
        adulterado[47] ^= 1;
        assert_eq!(aes_cbc(&k, &iv, &adulterado, false).unwrap_err(), operacao());
        assert_eq!(aes_cbc(&k, &iv[..8], &p, true).unwrap_err().0, "OperationError");
        let ctr = h("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        for bits in [64, 128] {
            assert_eq!(
                x(&aes_ctr(&k, &ctr, bits, &p).unwrap()),
                "874d6191b620e3261bef6864990db6ce9806f66b7970fdff8617187bb9fffdff"
            );
        }
        // contador de 8 bits dá a volta sem mexer no resto do bloco
        let a = aes_ctr(&k, &ctr, 8, &[0u8; 32]).unwrap();
        let mut c2 = ctr.clone();
        c2[15] = 0;
        assert_eq!(a[16..], aes_ctr(&k, &c2, 128, &[0u8; 16]).unwrap()[..]);
        assert_eq!(aes_ctr(&k, &ctr, 4, &[0u8; 17 * 16]).unwrap_err().0, "DataError");
        assert_eq!(aes_ctr(&k, &ctr, 0, &p).unwrap_err().0, "OperationError");
    }

    #[test]
    fn aes_gcm_nist() {
        let z = [0u8; 16];
        let c = aes_gcm(&z, &[0; 12], &[], 128, &z, true).unwrap();
        assert_eq!(x(&c), "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf");
        let k = h("feffe9928665731c6d6a8f9467308308");
        let p = h("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72
                   1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39");
        let aad = h("feedfacedeadbeeffeedfacedeadbeefabaddad2");
        // casos 4 (IV de 96 bits), 5 (64 bits) e 6 (480 bits) da especificação do GCM
        let casos = [
            (
                "cafebabefacedbaddecaf888",
                "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e\
                 21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091",
                "5bc94fbc3221a5db94fae95ae7121a47",
            ),
            (
                "cafebabefacedbad",
                "61353b4c2806934a777ff51fa22a4755699b2a714fcdc6f83766e5f97b6c7423\
                 73806900e49f24b22b097544d4896b424989b5e1ebac0f07c23f4598",
                "3612d2e79e3b0785561be14aaca2fccb",
            ),
            (
                "9313225df88406e555909c5aff5269aa6a7a9538534f7da1e4c303d2a318a728\
                 c3c0c95156809539fcf0e2429a6b525416aedbf5a0de6a57a637b39b",
                "8ce24998625615b603a033aca13fb894be9112a5c3a211a8ba262a3cca7e2ca7\
                 01e4a9a4fba43c90ccdcb281d48c7c6fd62875d2aca417034c34aee5",
                "619cc5aefffe0bfa462af43c1699d050",
            ),
        ];
        for (iv, cifrado, tag) in casos {
            let c = aes_gcm(&k, &h(iv), &aad, 128, &p, true).unwrap();
            assert_eq!(x(&c), format!("{cifrado}{tag}"));
            assert_eq!(aes_gcm(&k, &h(iv), &aad, 128, &c, false).unwrap(), p);
            // tag curta: os primeiros bytes da tag inteira
            let c96 = aes_gcm(&k, &h(iv), &aad, 96, &p, true).unwrap();
            assert_eq!(x(&c96), format!("{cifrado}{}", &tag[..24]));
            assert_eq!(aes_gcm(&k, &h(iv), &aad, 96, &c96, false).unwrap(), p);
        }
        let mut ruim = aes_gcm(&k, &h("cafebabefacedbad"), &aad, 128, &p, true).unwrap();
        ruim[0] ^= 1;
        assert_eq!(aes_gcm(&k, &h("cafebabefacedbad"), &aad, 128, &ruim, false).unwrap_err(), operacao());
        assert_eq!(aes_gcm(&k, &[], &aad, 128, &p, true).unwrap_err(), operacao());
        assert_eq!(aes_gcm(&k, &[1; 12], &[], 128, &[0; 5], false).unwrap_err().1, "The provided data is too small");
        assert_eq!(aes_gcm(&k, &[1; 12], &[], 48, &p, true).unwrap_err().0, "OperationError");
    }

    #[test]
    fn aes_kw_rfc3394() {
        let kek = h("000102030405060708090A0B0C0D0E0F");
        let chave = h("00112233445566778899AABBCCDDEEFF");
        let w = aes_kw(&kek, &chave, true).unwrap();
        assert_eq!(x(&w), "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5");
        assert_eq!(aes_kw(&kek, &w, false).unwrap(), chave);
        let kek256 = h("000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F");
        let chave256 = h("00112233445566778899AABBCCDDEEFF000102030405060708090A0B0C0D0E0F");
        assert_eq!(
            x(&aes_kw(&kek256, &chave256, true).unwrap()),
            "28c9f404c4b810f4cbccb35cfb87f8263f5786e2d80ed326cbc7f0e71a99f43bfb988b9b7a02dd21"
        );
        let mut ruim = w.clone();
        ruim[3] ^= 1;
        assert_eq!(aes_kw(&kek, &ruim, false).unwrap_err(), operacao());
        assert_eq!(aes_kw(&kek, &[0; 20], true).unwrap_err().0, "DataError");
        assert_eq!(aes_kw(&kek, &[0; 8], true).unwrap_err().0, "OperationError");
    }

    #[test]
    fn ec_ida_e_volta() {
        for curva in [Curva::P256, Curva::P384] {
            let (d, publica) = na_curva!(curva, m => m::gerar());
            for h in [Hash::Sha1, Hash::Sha256, Hash::Sha384, Hash::Sha512] {
                let sig = na_curva!(curva, m => m::assina(h, &d, b"mensagem")).unwrap();
                assert!(na_curva!(curva, m => m::verifica(h, &publica, b"mensagem", &sig)));
                assert!(!na_curva!(curva, m => m::verifica(h, &publica, b"mensagen", &sig)));
                assert!(!na_curva!(curva, m => m::verifica(h, &publica, b"mensagem", &sig[1..])));
            }
            // formatos: spki, pkcs8 e JWK voltam à mesma chave
            let spki = na_curva!(curva, m => m::para_spki(&publica)).unwrap();
            assert_eq!(ec_spki(curva, &spki).unwrap(), publica);
            let pkcs8 = na_curva!(curva, m => m::para_pkcs8(&d)).unwrap();
            assert_eq!(ec_pkcs8(curva, &pkcs8).unwrap(), (d.clone(), publica.clone()));
            let (d2, p2) = na_curva!(curva, m => m::jwk(&publica[1..], Some(&d))).unwrap();
            assert_eq!((d2.unwrap(), p2), (d.clone(), publica.clone()));
            // ECDH dos dois lados
            let (d3, p3) = na_curva!(curva, m => m::gerar());
            let s1 = na_curva!(curva, m => m::ecdh(&d, &p3)).unwrap();
            let s2 = na_curva!(curva, m => m::ecdh(&d3, &publica)).unwrap();
            assert_eq!(s1, s2);
        }
        // chave de uma curva importada como outra
        let (_, publica) = p384c::gerar();
        let spki = p384c::para_spki(&publica).unwrap();
        assert_eq!(
            ec_spki(Curva::P256, &spki).unwrap_err().1,
            "The imported EC key specifies a different curve than requested"
        );
    }

    #[test]
    fn rsa_ida_e_volta() {
        let c = rsa_gerar(512, 65537).unwrap();
        assert_eq!(c.len(), 8);
        assert_eq!(c[1], vec![1, 0, 1]);
        for esquema in [Esquema::Pkcs1, Esquema::Pss(20)] {
            let sig = rsa_assina(&c, esquema, Hash::Sha1, b"oi").unwrap();
            assert!(rsa_verifica(&c[..2], esquema, Hash::Sha1, b"oi", &sig));
            assert!(!rsa_verifica(&c[..2], esquema, Hash::Sha1, b"oo", &sig));
        }
        // OAEP com SHA-1 cabe em 512 bits (k - 2*20 - 2 = 22 bytes); com SHA-256, não
        let cifrado = rsa_oaep(&c[..2], Hash::Sha1, b"rotulo", b"segredo", true).unwrap();
        assert_eq!(rsa_oaep(&c, Hash::Sha1, b"rotulo", &cifrado, false).unwrap(), b"segredo");
        assert_eq!(rsa_oaep(&c, Hash::Sha1, b"outro", &cifrado, false).unwrap_err(), operacao());
        assert_eq!(rsa_oaep(&c[..2], Hash::Sha256, b"", b"x", true).unwrap_err(), operacao());
        assert_eq!(rsa_spki(&rsa_para_spki(&c[..2]).unwrap()).unwrap(), c[..2].to_vec());
        assert_eq!(rsa_pkcs8(&rsa_para_pkcs8(&c).unwrap()).unwrap(), c);
        // CRT inconsistente é recusado
        let mut ruim = c.clone();
        ruim[7][0] ^= 1;
        assert_eq!(rsa_jwk(&ruim).unwrap_err().0, "DataError");
        let grande = rsa_gerar(1024, 3).unwrap();
        assert_eq!(grande[1], vec![3]);
        let cifrado = rsa_oaep(&grande[..2], Hash::Sha256, b"", &[7; 62], true).unwrap();
        assert_eq!(rsa_oaep(&grande, Hash::Sha256, b"", &cifrado, false).unwrap(), [7; 62]);
        assert_eq!(rsa_oaep(&grande[..2], Hash::Sha256, b"", &[7; 63], true).unwrap_err(), operacao());
        assert_eq!(rsa_assina(&grande, Esquema::Pss(200), Hash::Sha512, b"x").unwrap_err(), operacao());
    }

    #[test]
    fn ed25519_e_x25519_rfc() {
        let sk = h("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
        let pk = okp_publica(Okp::Ed25519, &sk).unwrap();
        assert_eq!(x(&pk), "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let sig = ed25519_assina(&sk, b"").unwrap();
        assert_eq!(
            x(&sig),
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        );
        assert!(ed25519_verifica(&pk, b"", &sig));
        assert!(!ed25519_verifica(&pk, b"x", &sig));
        assert_eq!(okp_spki(Okp::Ed25519, &okp_para_spki(Okp::Ed25519, &pk)).unwrap(), pk);
        assert_eq!(okp_pkcs8(Okp::Ed25519, &okp_para_pkcs8(Okp::Ed25519, &sk)).unwrap(), sk);
        let a = h("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let b = h("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
        let pa = okp_publica(Okp::X25519, &a).unwrap();
        assert_eq!(x(&pa), "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        let pb = okp_publica(Okp::X25519, &b).unwrap();
        let s = "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742";
        assert_eq!(x(&x25519(&a, &pb).unwrap()), s);
        assert_eq!(x(&x25519(&b, &pa).unwrap()), s);
        assert_eq!(x25519(&a, &[0; 32]).unwrap_err(), operacao());
    }
}
