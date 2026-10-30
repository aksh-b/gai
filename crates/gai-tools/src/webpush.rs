//! Web push: browser notifications without a third-party service.
//!
//! Messages are encrypted for the browser (RFC 8291, `aes128gcm`) and
//! signed with the server's VAPID key (RFC 8292), then posted to the push
//! service the browser chose.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use ring::{aead, agreement, hkdf};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// What a browser's `PushManager.subscribe()` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PushSubscription {
    pub endpoint: String,
    pub keys: PushKeys,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PushKeys {
    /// The browser's P-256 public key, base64url.
    pub p256dh: String,
    /// 16 random bytes, base64url.
    pub auth: String,
}

impl PushSubscription {
    pub fn validate(&self) -> Result<(), String> {
        let u = reqwest::Url::parse(&self.endpoint).map_err(|e| format!("endpoint: {e}"))?;
        if u.scheme() != "https" {
            return Err("the push endpoint must be https".into());
        }
        let key = decode(&self.keys.p256dh).map_err(|_| "p256dh isn't base64url")?;
        if key.len() != 65 || key[0] != 4 {
            return Err("p256dh must be an uncompressed P-256 key".into());
        }
        if decode(&self.keys.auth)
            .map_err(|_| "auth isn't base64url")?
            .len()
            != 16
        {
            return Err("auth must be 16 bytes".into());
        }
        Ok(())
    }
}

fn decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    B64.decode(s.trim_end_matches('='))
}

/// The server's VAPID key pair.
pub struct Vapid {
    pair: EcdsaKeyPair,
    pkcs8: Vec<u8>,
    /// `mailto:` or `https:` contact the push services may use.
    pub subject: String,
}

impl Vapid {
    pub fn generate(subject: &str) -> Result<Self, String> {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| "could not make a VAPID key")?;
        Self::from_pkcs8(pkcs8.as_ref(), subject)
    }

    /// From [`to_base64`](Self::to_base64)'s output.
    pub fn from_base64(key: &str, subject: &str) -> Result<Self, String> {
        let der = decode(key).map_err(|e| format!("VAPID key: {e}"))?;
        Self::from_pkcs8(&der, subject)
    }

    fn from_pkcs8(der: &[u8], subject: &str) -> Result<Self, String> {
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der, &SystemRandom::new())
                .map_err(|e| format!("VAPID key: {e}"))?;
        Ok(Self {
            pair,
            pkcs8: der.to_vec(),
            subject: subject.to_owned(),
        })
    }

    /// The private key (PKCS#8), base64url, for storing.
    pub fn to_base64(&self) -> String {
        B64.encode(&self.pkcs8)
    }

    /// The public key browsers subscribe with (`applicationServerKey`).
    pub fn public_key(&self) -> String {
        B64.encode(self.pair.public_key().as_ref())
    }

    /// `Authorization: vapid t=<jwt>, k=<key>` for a push endpoint.
    fn authorization(&self, endpoint: &str) -> Result<String, String> {
        let u = reqwest::Url::parse(endpoint).map_err(|e| e.to_string())?;
        let aud = u.origin().ascii_serialization();
        let exp = chrono::Utc::now().timestamp() + 12 * 3600;
        let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = B64.encode(json!({"aud": aud, "exp": exp, "sub": self.subject}).to_string());
        let signing_input = format!("{header}.{claims}");
        let sig = self
            .pair
            .sign(&SystemRandom::new(), signing_input.as_bytes())
            .map_err(|_| "could not sign the VAPID token")?;
        Ok(format!(
            "vapid t={signing_input}.{}, k={}",
            B64.encode(sig.as_ref()),
            self.public_key()
        ))
    }
}

struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn expand(prk: &hkdf::Prk, info: &[&[u8]], len: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0; len];
    prk.expand(info, Len(len))
        .and_then(|okm| okm.fill(&mut out))
        .map_err(|_| "hkdf failed")?;
    Ok(out)
}

/// Encrypt `payload` for a subscription (one `aes128gcm` record).
pub fn encrypt(sub: &PushSubscription, payload: &[u8]) -> Result<Vec<u8>, String> {
    sub.validate()?;
    let ua_public = decode(&sub.keys.p256dh).map_err(|e| e.to_string())?;
    let auth = decode(&sub.keys.auth).map_err(|e| e.to_string())?;
    let rng = SystemRandom::new();
    let ours = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|_| "could not make a key")?;
    let as_public = ours
        .compute_public_key()
        .map_err(|_| "could not make a key")?
        .as_ref()
        .to_vec();
    let theirs = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, &ua_public);
    let ecdh = agreement::agree_ephemeral(ours, &theirs, |s| s.to_vec())
        .map_err(|_| "the browser's key is not usable")?;
    let mut salt = [0u8; 16];
    rng.fill(&mut salt).map_err(|_| "no randomness")?;
    seal(&ecdh, &auth, &ua_public, &as_public, &salt, payload)
}

/// The RFC 8291 key schedule and one record, given the shared secret.
fn seal(
    ecdh: &[u8],
    auth: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8; 16],
    payload: &[u8],
) -> Result<Vec<u8>, String> {
    let prk_key = hkdf::Salt::new(hkdf::HKDF_SHA256, auth).extract(ecdh);
    let ikm = expand(&prk_key, &[b"WebPush: info\0", ua_public, as_public], 32)?;
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(&ikm);
    let cek = expand(&prk, &[b"Content-Encoding: aes128gcm\0"], 16)?;
    let nonce = expand(&prk, &[b"Content-Encoding: nonce\0"], 12)?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(|_| "bad key")?,
    );
    let mut body = payload.to_vec();
    body.push(2); // the last (only) record
    key.seal_in_place_append_tag(
        aead::Nonce::try_assume_unique_for_key(&nonce).map_err(|_| "bad nonce")?,
        aead::Aad::empty(),
        &mut body,
    )
    .map_err(|_| "could not encrypt")?;
    let mut out = Vec::with_capacity(86 + body.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(as_public);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Why a push failed.
#[derive(Debug, PartialEq)]
pub enum PushError {
    /// The subscription is gone (404/410): forget it.
    Gone,
    Failed(String),
}

/// Send one message. Payloads above about 3 KB are refused by push
/// services, so keep them small.
pub async fn send(
    http: &reqwest::Client,
    vapid: &Vapid,
    sub: &PushSubscription,
    payload: &[u8],
) -> Result<(), PushError> {
    let body = encrypt(sub, payload).map_err(PushError::Failed)?;
    let auth = vapid
        .authorization(&sub.endpoint)
        .map_err(PushError::Failed)?;
    let res = http
        .post(&sub.endpoint)
        .header("Authorization", auth)
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("TTL", "86400")
        .header("Urgency", "normal")
        .timeout(Duration::from_secs(20))
        .body(body)
        .send()
        .await
        .map_err(|e| PushError::Failed(e.to_string()))?;
    match res.status().as_u16() {
        200..=299 => Ok(()),
        404 | 410 => Err(PushError::Gone),
        s => Err(PushError::Failed(format!(
            "push service answered {s}: {}",
            res.text().await.unwrap_or_default()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decrypt as a browser would, with the browser's side of the agreement.
    fn open(
        ua: agreement::EphemeralPrivateKey,
        ua_public: &[u8],
        auth: &[u8],
        msg: &[u8],
    ) -> Vec<u8> {
        let salt: [u8; 16] = msg[..16].try_into().unwrap();
        assert_eq!(u32::from_be_bytes(msg[16..20].try_into().unwrap()), 4096);
        let idlen = msg[20] as usize;
        let as_public = &msg[21..21 + idlen];
        let theirs = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, as_public);
        let ecdh = agreement::agree_ephemeral(ua, &theirs, |s| s.to_vec()).unwrap();
        let prk_key = hkdf::Salt::new(hkdf::HKDF_SHA256, auth).extract(&ecdh);
        let ikm = expand(&prk_key, &[b"WebPush: info\0", ua_public, as_public], 32).unwrap();
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &salt).extract(&ikm);
        let cek = expand(&prk, &[b"Content-Encoding: aes128gcm\0"], 16).unwrap();
        let nonce = expand(&prk, &[b"Content-Encoding: nonce\0"], 12).unwrap();
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
        let mut body = msg[21 + idlen..].to_vec();
        let plain = key
            .open_in_place(
                aead::Nonce::try_assume_unique_for_key(&nonce).unwrap(),
                aead::Aad::empty(),
                &mut body,
            )
            .unwrap();
        assert_eq!(plain.last(), Some(&2));
        plain[..plain.len() - 1].to_vec()
    }

    #[test]
    fn a_browser_can_read_what_is_sent() {
        let rng = SystemRandom::new();
        let ua = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let ua_public = ua.compute_public_key().unwrap().as_ref().to_vec();
        let mut auth = [0u8; 16];
        rng.fill(&mut auth).unwrap();
        let sub = PushSubscription {
            endpoint: "https://push.example.com/send/abc".into(),
            keys: PushKeys {
                p256dh: B64.encode(&ua_public),
                auth: B64.encode(auth),
            },
        };
        let msg = encrypt(&sub, b"{\"title\":\"hi\"}").unwrap();
        assert_eq!(open(ua, &ua_public, &auth, &msg), b"{\"title\":\"hi\"}");
    }

    #[test]
    fn vapid_tokens_verify_with_the_public_key() {
        let v = Vapid::generate("mailto:ops@example.com").unwrap();
        let again = Vapid::from_base64(&v.to_base64(), "mailto:ops@example.com").unwrap();
        assert_eq!(v.public_key(), again.public_key());
        let header = again
            .authorization("https://push.example.com/send/abc")
            .unwrap();
        let t = header
            .strip_prefix("vapid t=")
            .unwrap()
            .split(", k=")
            .next()
            .unwrap();
        let (input, sig) = t.rsplit_once('.').unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&decode(input.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://push.example.com");
        let key = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_FIXED,
            decode(&v.public_key()).unwrap(),
        );
        key.verify(input.as_bytes(), &decode(sig).unwrap()).unwrap();
    }

    #[test]
    fn bad_subscriptions_are_refused() {
        let sub = PushSubscription {
            endpoint: "http://push.example.com/x".into(),
            keys: PushKeys {
                p256dh: "AAAA".into(),
                auth: "AAAA".into(),
            },
        };
        assert!(sub.validate().is_err());
    }
}
