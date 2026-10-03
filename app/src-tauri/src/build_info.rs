//! Build-time version and commit, from one source.
//!
//! `CARGO_PKG_VERSION` is not honest: release builds patch Cargo.toml with a
//! pre-release-stripped semver, and dev/local builds carry the `0.1.0`
//! placeholder. `build.rs` instead emits `SKEIN_VERSION` / `SKEIN_COMMIT`:
//! from `SKEIN_BUILD_VERSION` / `SKEIN_BUILD_COMMIT` when CI sets them
//! (release.yml), else from git (`<nearest tag>-dev+<sha>`), else
//! `0.0.0-dev` with no commit.

/// The version this binary was built as, e.g. `0.4.2-dev+abc1234`.
pub const VERSION: &str = env!("SKEIN_VERSION");

const COMMIT: &str = env!("SKEIN_COMMIT");

/// The short commit this binary was built from, when known.
pub fn commit() -> Option<&'static str> {
    if COMMIT.is_empty() {
        None
    } else {
        Some(COMMIT)
    }
}

/// Which build profile a bundle identifier belongs to.
pub fn profile_for_identifier(id: &str) -> &'static str {
    match id {
        "com.timeloop-vault.skein" => "release",
        "com.timeloop-vault.skein.local" => "local",
        "com.timeloop-vault.skein.dev" => "dev",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_map() {
        assert_eq!(
            profile_for_identifier("com.timeloop-vault.skein"),
            "release"
        );
        assert_eq!(
            profile_for_identifier("com.timeloop-vault.skein.local"),
            "local"
        );
        assert_eq!(
            profile_for_identifier("com.timeloop-vault.skein.dev"),
            "dev"
        );
        assert_eq!(profile_for_identifier("com.example.other"), "unknown");
    }

    #[test]
    fn version_is_set() {
        println!("VERSION={VERSION} commit={:?}", commit());
        assert_ne!(VERSION, "");
    }
}
