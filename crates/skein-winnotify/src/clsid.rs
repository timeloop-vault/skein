//! Deterministic per-build CLSID derivation (#155).
//!
//! Each of Skein's three profiles (dev/local/release, issue #21) has
//! its own `identifier` and needs its own COM toast activator so they
//! can register and run side by side without fighting over one
//! `AppUserModelId`/CLSID pair. `uuid`'s v5 (name-based UUIDs) would
//! be the obvious tool, but it isn't in the lockfile yet — enabling it
//! pulls a brand-new dependency (`sha1_smol`) for a value nothing
//! ever needs to validate against a real UUID authority. A pure,
//! documented 128-bit hash gets the same two properties (deterministic,
//! distinct per identifier) without the extra download.

/// Non-cryptographic, deterministic 128-bit hash of `input`. Two
/// independent 64-bit FNV-1a passes (distinct seeds, one of them
/// additionally bit-rotated) simply because a single FNV-1a pass only
/// produces 64 bits — collision resistance is not a requirement here,
/// only that the same bytes always produce the same hash and that
/// different bytes are vanishingly unlikely to collide.
const fn stable_hash128(input: &[u8]) -> u128 {
    const FNV_OFFSET_A: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_OFFSET_B: u64 = 0x8422_2325_cbf2_9ce4;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

    let mut hash_a = FNV_OFFSET_A;
    let mut hash_b = FNV_OFFSET_B;
    let mut i = 0;
    while i < input.len() {
        let byte = input[i] as u64;
        hash_a = (hash_a ^ byte).wrapping_mul(FNV_PRIME);
        hash_b = (hash_b ^ byte).wrapping_mul(FNV_PRIME).rotate_left(7);
        i += 1;
    }
    ((hash_a as u128) << 64) | (hash_b as u128)
}

/// Deterministic CLSID for a Skein build `identifier`
/// (`com.timeloop-vault.skein[.dev|.local]`, from `tauri.conf.json`).
/// The same identifier always derives the same value, so re-running
/// `register_app` on every launch is idempotent; different
/// identifiers derive different values, so
/// dev/local/release can register distinct COM activators and run
/// side by side. Pure and cross-platform on purpose — see the module
/// docs — even though only Windows ever calls it for real.
pub fn clsid_for(identifier: &str) -> u128 {
    stable_hash128(identifier.as_bytes())
}

/// Format a CLSID the way the registry expects it: braced, uppercase,
/// grouped `8-4-4-4-12` hex digits. The grouping matches
/// `windows::core::GUID::from_u128`'s own big-endian byte layout, so
/// a value written here and one built from the same `u128` via that
/// conversion always print identically.
pub fn clsid_braced(clsid: u128) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}",
        (clsid >> 96) as u32,
        ((clsid >> 80) & 0xFFFF) as u16,
        ((clsid >> 64) & 0xFFFF) as u16,
        ((clsid >> 48) & 0xFFFF) as u16,
        clsid & 0xFFFF_FFFF_FFFF,
    )
}

#[cfg(test)]
mod tests {
    use super::{clsid_braced, clsid_for};

    #[test]
    fn deterministic_for_the_same_identifier() {
        let a = clsid_for("com.timeloop-vault.skein.dev");
        let b = clsid_for("com.timeloop-vault.skein.dev");
        assert_eq!(a, b);
    }

    #[test]
    fn distinct_across_the_three_profiles() {
        let dev = clsid_for("com.timeloop-vault.skein.dev");
        let local = clsid_for("com.timeloop-vault.skein.local");
        let release = clsid_for("com.timeloop-vault.skein");
        assert_ne!(dev, local);
        assert_ne!(dev, release);
        assert_ne!(local, release);
    }

    #[test]
    fn braced_uppercase_format() {
        let clsid = clsid_for("com.timeloop-vault.skein");
        let s = clsid_braced(clsid);
        assert!(s.starts_with('{'));
        assert!(s.ends_with('}'));
        assert_eq!(s.len(), 38, "{{8-4-4-4-12 hex}} plus braces: {s}");
        assert_eq!(s, s.to_uppercase());
        let inner = &s[1..s.len() - 1];
        let groups: Vec<&str> = inner.split('-').collect();
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(inner.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
    }

    #[test]
    fn empty_identifier_still_deterministic() {
        assert_eq!(clsid_for(""), clsid_for(""));
    }
}
