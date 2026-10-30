//! The signed channel envelope: one JSON object holding the exact manifest bytes and the owner's
//! Ed25519 signature over them, so a client never combines a manifest and a signature from
//! different publications.
//!
//! ```json
//! {"envelope": "pdf-overlay-update-envelope", "envelope_version": 1, "manifest_schema": 1,
//!  "key_id": "ov-a", "signature_alg": "ed25519",
//!  "manifest_b64": "<the manifest bytes>", "signature_b64": "<64-byte signature>"}
//! ```
//!
//! The signature covers [`signed_message`]: a domain-separation prefix, the envelope version, the
//! manifest schema and the key id, then the manifest bytes, so none of them can be swapped. It is
//! checked over the decoded bytes before they are parsed.

use base64::Engine as _;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde_json::{Map, Value};

use crate::{ENVELOPE_VERSION, Error, MANIFEST_SCHEMA};

/// The envelope's `envelope` field.
pub const KIND: &str = "pdf-overlay-update-envelope";
/// The only signature algorithm.
pub const ALGORITHM: &str = "ed25519";
/// The largest envelope read.
pub const MAX_ENVELOPE: usize = 128 * 1024;
/// The largest manifest inside it.
pub const MAX_MANIFEST: usize = 64 * 1024;
/// Prefix of every signed message.
const DOMAIN: &[u8] = b"PDF-OVERLAY-UPDATE-MANIFEST\0";

/// A public key this build trusts, by id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustedKey {
    pub id: &'static str,
    pub key: [u8; 32],
}

/// The bytes a signature covers.
pub fn signed_message(envelope_version: u32, manifest_schema: u32, key_id: &str, manifest: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(DOMAIN.len() + 8 + key_id.len() + 1 + manifest.len());
    m.extend_from_slice(DOMAIN);
    m.extend_from_slice(&envelope_version.to_be_bytes());
    m.extend_from_slice(&manifest_schema.to_be_bytes());
    m.extend_from_slice(key_id.as_bytes());
    m.push(0);
    m.extend_from_slice(manifest);
    m
}

/// A key id: 1 to 32 characters of `a-z`, `0-9` and `-`.
pub fn valid_key_id(id: &str) -> bool {
    (1..=32).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// Check `bytes` as an envelope signed by one of `keys`; the verified manifest bytes.
pub fn open(bytes: &[u8], keys: &[TrustedKey]) -> Result<Vec<u8>, Error> {
    if bytes.len() > MAX_ENVELOPE {
        return Err(Error::Envelope(format!("larger than {} KiB", MAX_ENVELOPE / 1024)));
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::Envelope("not JSON".into()))?;
    let obj = value.as_object().ok_or_else(|| Error::Envelope("not a JSON object".into()))?;
    const FIELDS: [&str; 7] = ["envelope", "envelope_version", "manifest_schema", "key_id", "signature_alg", "manifest_b64", "signature_b64"];
    if let Some(extra) = obj.keys().find(|k| !FIELDS.contains(&k.as_str())) {
        return Err(Error::Envelope(format!("unexpected field {extra:?}")));
    }
    let text = |k: &str| obj.get(k).and_then(Value::as_str).ok_or_else(|| Error::Envelope(format!("missing {k}")));
    let number =
        |k: &str| obj.get(k).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| Error::Envelope(format!("missing {k}")));
    if text("envelope")? != KIND {
        return Err(Error::Envelope("not a PDF Overlay update envelope".into()));
    }
    let version = number("envelope_version")?;
    if version != ENVELOPE_VERSION {
        return Err(Error::Unsupported(format!("envelope version {version}")));
    }
    let schema = number("manifest_schema")?;
    if schema != MANIFEST_SCHEMA {
        return Err(Error::Unsupported(format!("manifest schema {schema}")));
    }
    let alg = text("signature_alg")?;
    if alg != ALGORITHM {
        return Err(Error::Unsupported(format!("signature algorithm {alg:?}")));
    }
    let key_id = text("key_id")?;
    if !valid_key_id(key_id) {
        return Err(Error::Envelope("malformed key id".into()));
    }
    if keys.is_empty() {
        return Err(Error::NoTrustedKey);
    }
    let key = keys.iter().find(|k| k.id == key_id).ok_or_else(|| Error::UnknownKey(key_id.into()))?;
    let manifest_b64 = text("manifest_b64")?;
    // Base64 is 4/3 of the bytes: refuse before decoding anything too large.
    if manifest_b64.len() > MAX_MANIFEST / 3 * 4 + 4 {
        return Err(Error::Envelope(format!("manifest larger than {} KiB", MAX_MANIFEST / 1024)));
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let manifest = b64.decode(manifest_b64).map_err(|_| Error::Envelope("manifest isn't base64".into()))?;
    if manifest.len() > MAX_MANIFEST {
        return Err(Error::Envelope(format!("manifest larger than {} KiB", MAX_MANIFEST / 1024)));
    }
    let signature = b64.decode(text("signature_b64")?).map_err(|_| Error::Envelope("signature isn't base64".into()))?;
    let signature: [u8; 64] = signature.try_into().map_err(|_| Error::Envelope("signature isn't 64 bytes".into()))?;
    let verifying = VerifyingKey::from_bytes(&key.key).map_err(|_| Error::UnknownKey(key_id.into()))?;
    verifying
        .verify_strict(&signed_message(version, schema, key_id, &manifest), &Signature::from_bytes(&signature))
        .map_err(|_| Error::BadSignature)?;
    Ok(manifest)
}

/// Sign `manifest` (the owner's publication tool and tests): the envelope's JSON text.
pub fn seal(manifest: &[u8], key_id: &str, key: &SigningKey) -> Result<String, Error> {
    if !valid_key_id(key_id) {
        return Err(Error::Envelope("malformed key id".into()));
    }
    if manifest.len() > MAX_MANIFEST {
        return Err(Error::Envelope(format!("manifest larger than {} KiB", MAX_MANIFEST / 1024)));
    }
    let signature = key.sign(&signed_message(ENVELOPE_VERSION, MANIFEST_SCHEMA, key_id, manifest));
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut obj = Map::new();
    obj.insert("envelope".into(), KIND.into());
    obj.insert("envelope_version".into(), ENVELOPE_VERSION.into());
    obj.insert("manifest_schema".into(), MANIFEST_SCHEMA.into());
    obj.insert("key_id".into(), key_id.into());
    obj.insert("signature_alg".into(), ALGORITHM.into());
    obj.insert("manifest_b64".into(), b64.encode(manifest).into());
    obj.insert("signature_b64".into(), b64.encode(signature.to_bytes()).into());
    serde_json::to_string_pretty(&Value::Object(obj)).map_err(|e| Error::Envelope(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> (SigningKey, TrustedKey) {
        let sk = SigningKey::from_bytes(&[n; 32]);
        (sk.clone(), TrustedKey { id: "test-a", key: sk.verifying_key().to_bytes() })
    }

    fn edit(envelope: &str, f: impl FnOnce(&mut Map<String, Value>)) -> String {
        let mut v: Value = serde_json::from_str(envelope).unwrap();
        f(v.as_object_mut().unwrap());
        v.to_string()
    }

    #[test]
    fn a_sealed_manifest_opens_with_its_key_only() {
        let (sk, trusted) = key(7);
        let env = seal(b"{\"x\":1}", "test-a", &sk).unwrap();
        assert_eq!(open(env.as_bytes(), &[trusted]).unwrap(), b"{\"x\":1}");
        let (_, other) = key(8);
        assert_eq!(open(env.as_bytes(), &[other]), Err(Error::BadSignature));
        assert_eq!(open(env.as_bytes(), &[]), Err(Error::NoTrustedKey));
        let renamed = TrustedKey { id: "test-b", ..trusted };
        assert_eq!(open(env.as_bytes(), &[renamed]), Err(Error::UnknownKey("test-a".into())));
    }

    #[test]
    fn every_signed_field_is_bound() {
        let (sk, trusted) = key(7);
        let env = seal(b"{\"seq\":1}", "test-a", &sk).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        // Another manifest under the same signature.
        let swapped = edit(&env, |o| {
            o.insert("manifest_b64".into(), b64.encode(b"{\"seq\":9}").into());
        });
        assert_eq!(open(swapped.as_bytes(), &[trusted]), Err(Error::BadSignature));
        // The same key bytes under another id: the id is part of the message.
        let other_id = edit(&env, |o| {
            o.insert("key_id".into(), "test-b".into());
        });
        let renamed = TrustedKey { id: "test-b", ..trusted };
        assert_eq!(open(other_id.as_bytes(), &[trusted, renamed]), Err(Error::BadSignature));
        // A message signed without the domain prefix doesn't verify.
        let raw = sk.sign(b"{\"seq\":1}");
        let bare = edit(&env, |o| {
            o.insert("signature_b64".into(), b64.encode(raw.to_bytes()).into());
        });
        assert_eq!(open(bare.as_bytes(), &[trusted]), Err(Error::BadSignature));
    }

    #[test]
    fn malformed_envelopes_are_refused_before_any_signature_check() {
        let (sk, trusted) = key(7);
        let env = seal(b"{}", "test-a", &sk).unwrap();
        let cases: Vec<(String, &str)> = vec![
            ("<html>Sign in</html>".into(), "not JSON"),
            ("[]".into(), "not a JSON object"),
            (
                edit(&env, |o| {
                    o.insert("extra".into(), 1.into());
                }),
                "unexpected field",
            ),
            (
                edit(&env, |o| {
                    o.insert("envelope".into(), "other".into());
                }),
                "not a PDF Overlay",
            ),
            (
                edit(&env, |o| {
                    o.remove("signature_b64");
                }),
                "missing signature_b64",
            ),
            (
                edit(&env, |o| {
                    o.insert("key_id".into(), "Bad Id".into());
                }),
                "malformed key id",
            ),
            (
                edit(&env, |o| {
                    o.insert("manifest_b64".into(), "***".into());
                }),
                "isn't base64",
            ),
            (
                edit(&env, |o| {
                    o.insert("signature_b64".into(), "AAAA".into());
                }),
                "isn't 64 bytes",
            ),
        ];
        for (bytes, expected) in cases {
            let e = open(bytes.as_bytes(), &[trusted]).unwrap_err().to_string();
            assert!(e.contains(expected), "{e} should mention {expected}");
        }
        for (field, value, expected) in [("envelope_version", 2, "envelope version 2"), ("manifest_schema", 2, "manifest schema 2")] {
            let e = edit(&env, |o| {
                o.insert(field.into(), value.into());
            });
            assert_eq!(open(e.as_bytes(), &[trusted]), Err(Error::Unsupported(expected.into())));
        }
        let alg = edit(&env, |o| {
            o.insert("signature_alg".into(), "rsa".into());
        });
        assert!(matches!(open(alg.as_bytes(), &[trusted]), Err(Error::Unsupported(_))));
    }

    #[test]
    fn sizes_are_capped() {
        let (sk, trusted) = key(7);
        assert!(seal(&vec![b' '; MAX_MANIFEST + 1], "test-a", &sk).is_err());
        let big = edit(&seal(b"{}", "test-a", &sk).unwrap(), |o| {
            o.insert("manifest_b64".into(), "A".repeat(MAX_MANIFEST / 3 * 4 + 8).into());
        });
        assert!(open(big.as_bytes(), &[trusted]).unwrap_err().to_string().contains("larger than"));
        assert!(open(&vec![b' '; MAX_ENVELOPE + 1], &[trusted]).unwrap_err().to_string().contains("larger than"));
    }
}
