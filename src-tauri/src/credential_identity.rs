use sha2::{Digest, Sha256};
/// Domain-separated fingerprint; display hints never decide account ownership.
pub(crate) fn fingerprint(provider: &str, credential: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(provider.as_bytes());
    digest.update([0]);
    digest.update(credential.as_bytes());
    format!("credential-v1:{:x}", digest.finalize())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_suffix_and_different_providers_never_share_identity() {
        let a = fingerprint("zai", "fixture-account-a-1234");
        assert_ne!(a, fingerprint("zai", "fixture-account-b-1234"));
        assert_ne!(a, fingerprint("opencode", "fixture-account-a-1234"));
        assert!(!a.contains("fixture-account"));
    }
}
