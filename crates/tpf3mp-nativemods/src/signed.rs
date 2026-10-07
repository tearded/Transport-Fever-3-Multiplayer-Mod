//! Signatures and hashes, as the launcher's signed releases use them (D7):
//! Ed25519 over the exact bytes of a JSON file, public keys as a list of
//! base64 strings built into the launcher, SHA-256 in lowercase hex. The
//! launcher's updater calls these same functions.

use base64::Engine;
use ring::{
    digest::{SHA256, digest},
    signature::{ED25519, UnparsedPublicKey},
};

/// The public keys the native-mods index is signed with, set when the
/// launcher is built (`TPF3MP_NATIVE_MODS_PUBLIC_KEY`, one or more, as
/// `TPF3MP_UPDATE_PUBLIC_KEY` for releases). A launcher built without one
/// installs no native mod.
const PUBLIC_KEYS: Option<&str> = option_env!("TPF3MP_NATIVE_MODS_PUBLIC_KEY");

/// The keys this build trusts for the native-mods index.
pub fn trusted_keys() -> Vec<Vec<u8>> {
    parse_keys(PUBLIC_KEYS.unwrap_or_default())
}

/// The Ed25519 public keys in `text`: base64, separated by commas or
/// spaces. An entry that is not a 32-byte key is left out.
pub fn parse_keys(text: &str) -> Vec<Vec<u8>> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let key = base64::engine::general_purpose::STANDARD
                .decode(entry)
                .ok()?;
            (key.len() == 32).then_some(key)
        })
        .collect()
}

/// Whether `signature` is one of `keys`' Ed25519 signatures of `message`.
/// No keys, no verification.
pub fn verify(message: &[u8], signature: &[u8], keys: &[Vec<u8>]) -> bool {
    keys.iter().any(|key| {
        UnparsedPublicKey::new(&ED25519, key)
            .verify(message, signature)
            .is_ok()
    })
}

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The SHA-256 of `bytes`, in lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(digest(&SHA256, bytes).as_ref())
}

/// Whether `text` is a SHA-256 in lowercase hex.
pub fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use ring::{
        rand::SystemRandom,
        signature::{Ed25519KeyPair, KeyPair},
    };

    use super::*;

    fn pair() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    #[test]
    fn only_a_trusted_key_verifies() {
        let ours = pair();
        let theirs = pair();
        let message = b"{\"format\":1}";
        let signature = ours.sign(message);
        let keys = vec![ours.public_key().as_ref().to_vec()];
        assert!(verify(message, signature.as_ref(), &keys));
        assert!(!verify(b"{\"format\":2}", signature.as_ref(), &keys));
        assert!(!verify(
            message,
            signature.as_ref(),
            &[theirs.public_key().as_ref().to_vec()]
        ));
        assert!(!verify(message, signature.as_ref(), &[]));
    }

    #[test]
    fn keys_are_a_list_and_junk_is_left_out() {
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let short = base64::engine::general_purpose::STANDARD.encode([7u8; 31]);
        assert_eq!(parse_keys(&format!("{key}, {key}\n{short} nope")).len(), 2);
        assert!(parse_keys("").is_empty());
    }

    #[test]
    fn hashes_are_lowercase_hex() {
        let hash = sha256_hex(b"abc");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(is_sha256(&hash));
        assert!(!is_sha256(&hash.to_ascii_uppercase()));
        assert!(!is_sha256("abc"));
    }
}
