const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Width and height from the IHDR chunk, which must come first.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn dimensions(png: &[u8]) -> Result<(u32, u32), String> {
    if png.len() < 24 || png[..8] != SIGNATURE || &png[12..16] != b"IHDR" {
        return Err("not a PNG".into());
    }
    let be = |i: usize| u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]);
    let (w, h) = (be(16), be(20));
    if w == 0 || h == 0 {
        return Err("PNG has zero size".into());
    }
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: u32, h: u32) -> Vec<u8> {
        let mut v = SIGNATURE.to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v
    }

    #[test]
    fn reads_ihdr() {
        assert_eq!(dimensions(&header(640, 480)), Ok((640, 480)));
    }

    #[test]
    fn rejects_garbage() {
        assert!(dimensions(b"nope").is_err());
        assert!(dimensions(&header(0, 5)).is_err());
        let mut bad = header(1, 1);
        bad[12] = b'X';
        assert!(dimensions(&bad).is_err());
    }
}
