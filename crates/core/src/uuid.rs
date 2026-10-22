//! Player identity helpers.
//!
//! Offline mode must reproduce the vanilla "offline UUID" algorithm exactly,
//! otherwise world data and server whitelists keyed by UUID stop matching.

use md5::{Digest, Md5};

/// Version-3 (MD5) UUID derived from `OfflinePlayer:<name>`, undashed 32-char
/// hex (the form Minecraft expects in `${auth_uuid}`).
pub fn offline_uuid_undashed(name: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(format!("OfflinePlayer:{name}").as_bytes());
    let mut bytes: [u8; 16] = hasher.finalize().into();

    // RFC 4122: version 3, variant 10xx.
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Same UUID with canonical `8-4-4-4-12` hyphenation.
pub fn offline_uuid(name: &str) -> String {
    hyphenate(&offline_uuid_undashed(name))
}

/// Insert hyphens into a 32-char undashed UUID; leaves other shapes untouched.
pub fn hyphenate(uuid: &str) -> String {
    if uuid.len() == 32 && !uuid.contains('-') {
        format!(
            "{}-{}-{}-{}-{}",
            &uuid[0..8],
            &uuid[8..12],
            &uuid[12..16],
            &uuid[16..20],
            &uuid[20..32]
        )
    } else {
        uuid.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_is_stable_and_version_3() {
        let uuid = offline_uuid_undashed("Notch");
        assert_eq!(uuid.len(), 32);
        assert_eq!(&uuid[12..13], "3", "version nibble must be 3");
        assert!(["8", "9", "a", "b"].contains(&&uuid[16..17]));
        assert_eq!(uuid, offline_uuid_undashed("Notch"));
        assert_ne!(uuid, offline_uuid_undashed("notch"));
    }

    #[test]
    fn hyphenation() {
        let u = offline_uuid("Steve");
        assert_eq!(u.len(), 36);
        assert_eq!(hyphenate(&u), u);
        assert_eq!(hyphenate("0123456789abcdef0123456789abcdef").len(), 36);
    }
}
