//! API key material. Keys are 160 random bits, shown once; only a SHA-256
//! hash is stored. A fast hash is right here: the keys are random, so there
//! is nothing for password stretching to protect against.

use sha2::{Digest, Sha256};

/// A fresh key.
pub struct NewKey {
    /// The full key, e.g. `gai_3f9a…`. Show it once, never store it.
    pub secret: String,
    /// The first characters, safe to display.
    pub prefix: String,
    pub hash: String,
}

pub fn generate() -> NewKey {
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).expect("the OS random source is available");
    let secret = format!("gai_{}", hex(&bytes));
    NewKey {
        prefix: secret[..12].to_owned(),
        hash: hash(&secret),
        secret,
    }
}

pub fn hash(secret: &str) -> String {
    hex(&Sha256::digest(secret.trim().as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_hash_stably() {
        let a = generate();
        let b = generate();
        assert_ne!(a.secret, b.secret);
        assert!(a.secret.starts_with("gai_") && a.secret.len() == 44);
        assert_eq!(hash(&a.secret), a.hash);
        assert_eq!(hash(&format!(" {} ", a.secret)), a.hash);
        assert!(a.secret.starts_with(&a.prefix));
    }
}
