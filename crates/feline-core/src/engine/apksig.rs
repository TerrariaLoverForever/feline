// SPDX-License-Identifier: GPL-3.0-or-later
//! Checking that an APK really comes from Roblox: Android's APK Signature
//! Scheme v2 (and v3, same layout plus SDK bounds), as `apksigner verify`
//! does. Every byte of the file outside the signing block is hashed in
//! 1 MiB chunks; the result must match the digest the signer signed, the
//! signature must verify with the signer's public key, and the signer's
//! certificate must be Roblox's (pinned by its SHA-256).
//!
//! Format: <https://source.android.com/docs/security/features/apksigning/v2>.

use ring::digest;
use ring::signature::{self, UnparsedPublicKey, VerificationAlgorithm};

const V2_BLOCK: u32 = 0x7109_871a;
const V3_BLOCK: u32 = 0xf053_68c0;
const MAGIC: &[u8; 16] = b"APK Sig Block 42";
const CHUNK: usize = 1 << 20;

/// What was verified: the signer certificate's SHA-256, as hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signer {
    pub certificate_sha256: String,
}

/// Length-prefixed (u32 LE) pieces, as the signing block nests them.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.0.get(..4).ok_or("truncated signature data")?;
        self.0 = &self.0[4..];
        Ok(u32::from_le_bytes(bytes.try_into().unwrap_or_default()))
    }
    fn prefixed(&mut self) -> Result<&'a [u8], String> {
        let len = self.u32()? as usize;
        let value = self.0.get(..len).ok_or("truncated signature data")?;
        self.0 = &self.0[len..];
        Ok(value)
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// (signature algorithm id) → (verification, content digest algorithm).
fn algorithm(id: u32) -> Option<(&'static dyn VerificationAlgorithm, &'static digest::Algorithm)> {
    Some(match id {
        0x0101 => (&signature::RSA_PSS_2048_8192_SHA256, &digest::SHA256),
        0x0102 => (&signature::RSA_PSS_2048_8192_SHA512, &digest::SHA512),
        0x0103 => (&signature::RSA_PKCS1_2048_8192_SHA256, &digest::SHA256),
        0x0104 => (&signature::RSA_PKCS1_2048_8192_SHA512, &digest::SHA512),
        0x0201 => (&signature::ECDSA_P256_SHA256_ASN1, &digest::SHA256),
        _ => return None,
    })
}

/// The key inside a SubjectPublicKeyInfo (DER): `SEQUENCE { algorithm,
/// BIT STRING key }`; ring wants the BIT STRING's contents.
fn spki_key(spki: &[u8]) -> Result<&[u8], String> {
    fn tlv(d: &[u8]) -> Option<(u8, &[u8], &[u8])> {
        let tag = *d.first()?;
        let first = *d.get(1)? as usize;
        let (len, head) = if first < 0x80 {
            (first, 2)
        } else {
            let n = first & 0x7f;
            if n == 0 || n > 4 {
                return None;
            }
            let mut len = 0usize;
            for i in 0..n {
                len = (len << 8) | *d.get(2 + i)? as usize;
            }
            (len, 2 + n)
        };
        let body = d.get(head..head + len)?;
        Some((tag, body, &d[head + len..]))
    }
    let bad = || "bad public key".to_owned();
    let (tag, seq, _) = tlv(spki).ok_or_else(bad)?;
    if tag != 0x30 {
        return Err(bad());
    }
    let (_, _algorithm, rest) = tlv(seq).ok_or_else(bad)?;
    let (tag, bits, _) = tlv(rest).ok_or_else(bad)?;
    // BIT STRING: first byte is the number of unused bits (0).
    if tag != 0x03 || bits.first() != Some(&0) {
        return Err(bad());
    }
    Ok(&bits[1..])
}

/// RSA PKCS#1 v1.5 with SHA-256 (0x0103) or SHA-512 (0x0104); `key` is an
/// RSAPublicKey (DER).
fn verify_rsa_pkcs1(id: u32, key: &[u8], data: &[u8], sig: &[u8]) -> bool {
    use rsa::pkcs1::DecodeRsaPublicKey;
    let Ok(public) = rsa::RsaPublicKey::from_pkcs1_der(key) else {
        return false;
    };
    if id == 0x0103 {
        let hashed = digest::digest(&digest::SHA256, data);
        public
            .verify(rsa::Pkcs1v15Sign::new::<rsa::sha2::Sha256>(), hashed.as_ref(), sig)
            .is_ok()
    } else {
        let hashed = digest::digest(&digest::SHA512, data);
        public
            .verify(rsa::Pkcs1v15Sign::new::<rsa::sha2::Sha512>(), hashed.as_ref(), sig)
            .is_ok()
    }
}

/// Hashes the file outside the signing block, the v2/v3 way.
fn content_digest(apk: &[u8], block_start: usize, cd_start: usize, eocd: usize, alg: &'static digest::Algorithm) -> Vec<u8> {
    // The end record, with its central directory offset pointing where the
    // signing block starts (as it did before signing).
    let mut end = apk[eocd..].to_vec();
    if end.len() >= 20 {
        end[16..20].copy_from_slice(&(block_start as u32).to_le_bytes());
    }
    let sections: [&[u8]; 3] = [&apk[..block_start], &apk[cd_start..eocd], &end];
    let mut chunk_digests = Vec::new();
    let mut count = 0u32;
    for section in sections {
        for chunk in section.chunks(CHUNK) {
            let mut ctx = digest::Context::new(alg);
            ctx.update(&[0xa5]);
            ctx.update(&(chunk.len() as u32).to_le_bytes());
            ctx.update(chunk);
            chunk_digests.extend_from_slice(ctx.finish().as_ref());
            count += 1;
        }
    }
    let mut top = digest::Context::new(alg);
    top.update(&[0x5a]);
    top.update(&count.to_le_bytes());
    top.update(&chunk_digests);
    top.finish().as_ref().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Verifies `apk`'s v2 (or v3) signature. The signer, or why it failed.
pub fn verify(apk: &[u8]) -> Result<Signer, String> {
    let (eocd, cd_size, cd_start) = super::zip::end_record(apk).ok_or("not a zip file")?;
    if cd_start + cd_size != eocd || cd_start < 32 {
        return Err("unexpected zip layout".into());
    }
    // The signing block ends right before the central directory with
    // its size and magic.
    if &apk[cd_start - 16..cd_start] != MAGIC {
        return Err("the APK isn't signed with scheme v2/v3".into());
    }
    let size = u64::from_le_bytes(apk[cd_start - 24..cd_start - 16].try_into().unwrap_or_default()) as usize;
    let block_start = cd_start
        .checked_sub(size + 8)
        .ok_or("bad signing block size")?;
    let mut pairs = &apk[block_start + 8..cd_start - 24];
    let (mut v2, mut v3) = (None, None);
    while pairs.len() >= 12 {
        let len = u64::from_le_bytes(pairs[..8].try_into().unwrap_or_default()) as usize;
        let pair = pairs.get(8..8 + len).ok_or("bad signing block entry")?;
        let id = u32::from_le_bytes(pair.get(..4).ok_or("bad entry")?.try_into().unwrap_or_default());
        match id {
            V2_BLOCK => v2 = Some(&pair[4..]),
            V3_BLOCK => v3 = Some(&pair[4..]),
            _ => {}
        }
        pairs = &pairs[8 + len..];
    }
    let (block, is_v3) = match (v2, v3) {
        (Some(b), _) => (b, false),
        (None, Some(b)) => (b, true),
        _ => return Err("no v2/v3 signature".into()),
    };

    let mut signers = Reader(Reader(block).prefixed()?);
    let mut signer = Reader(signers.prefixed()?);
    let signed_data = signer.prefixed()?;
    if is_v3 {
        let _min_sdk = signer.u32()?;
        let _max_sdk = signer.u32()?;
    }
    let mut signatures = Reader(signer.prefixed()?);
    let public_key = signer.prefixed()?;
    let key = spki_key(public_key)?;

    // The strongest signature this check supports.
    let mut best: Option<(u32, &[u8])> = None;
    while !signatures.is_empty() {
        let mut s = Reader(signatures.prefixed()?);
        let id = s.u32()?;
        let sig = s.prefixed()?;
        if algorithm(id).is_some() && best.is_none_or(|(b, _)| id > b) {
            best = Some((id, sig));
        }
    }
    let (id, sig) = best.ok_or("no supported signature algorithm")?;
    let (verification, digest_alg) = algorithm(id).ok_or("unsupported algorithm")?;
    let verified = match id {
        // RSA PKCS#1 v1.5 through `rsa`: Roblox's key is 1024-bit, which
        // ring refuses (2048-bit minimum).
        0x0103 | 0x0104 => verify_rsa_pkcs1(id, key, signed_data, sig),
        _ => UnparsedPublicKey::new(verification, key)
            .verify(signed_data, sig)
            .is_ok(),
    };
    if !verified {
        return Err("the signature doesn't verify".into());
    }

    // The signed data: digests, certificates, (v3: SDK bounds), attributes.
    let mut data = Reader(signed_data);
    let mut digests = Reader(data.prefixed()?);
    let mut certificates = Reader(data.prefixed()?);
    let mut expected = None;
    while !digests.is_empty() {
        let mut d = Reader(digests.prefixed()?);
        let dig_id = d.u32()?;
        let value = d.prefixed()?;
        if dig_id == id {
            expected = Some(value);
        }
    }
    let expected = expected.ok_or("no digest for the signature's algorithm")?;
    let certificate = certificates.prefixed()?;
    // The certificate must hold the key that signed.
    if !certificate.windows(public_key.len()).any(|w| w == public_key) {
        return Err("the certificate doesn't match the signing key".into());
    }
    if content_digest(apk, block_start, cd_start, eocd, digest_alg) != expected {
        return Err("the APK's contents were changed after signing".into());
    }
    Ok(Signer {
        certificate_sha256: hex(digest::digest(&digest::SHA256, certificate).as_ref()),
    })
}
