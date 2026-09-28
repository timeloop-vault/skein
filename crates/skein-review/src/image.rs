//! Image classification — issue #409. Pure and Tauri-free like the rest
//! of this crate: whether a path names an image, and what MIME type it
//! is, is decided by extension alone. Nothing here reads bytes; that is
//! the caller's job, capped by [`MAX_IMAGE_BYTES`].

/// Files above this are refused rather than read — binary IPC still
/// means holding the whole thing in memory twice (disk buffer + the
/// `ArrayBuffer` on the JS side), and a runaway generated image
/// shouldn't be able to stall the pane. 16 MiB is generous for a
/// screenshot or a rendered diagram, the two cases #409 targets.
pub const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;

/// The MIME type for `path`'s extension, or `None` if it is not one of
/// the image types the review pane and Files editor know how to render.
/// Case-insensitive; works on either `/` or `\` separators since it
/// only ever looks at the substring after the last `.`.
pub fn image_mime(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (stem, ext) = name.rsplit_once('.')?;
    // A dotfile like ".png" has no stem before its one dot — that's a
    // hidden file named "png", not a PNG with an empty name. Treat it
    // as extension-less rather than matching on the trailing letters.
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// Whether `path` names an image by the same rule as [`image_mime`].
pub fn is_image_path(path: &str) -> bool {
    image_mime(path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_known_extensions_case_insensitively() {
        let cases = [
            ("photo.png", Some("image/png")),
            ("photo.PNG", Some("image/png")),
            ("photo.jpg", Some("image/jpeg")),
            ("photo.JPEG", Some("image/jpeg")),
            ("anim.gif", Some("image/gif")),
            ("pic.webp", Some("image/webp")),
            ("icon.svg", Some("image/svg+xml")),
            ("notes.txt", None),
            ("README", None),
            ("archive.tar.gz", None),
            ("weird.pngx", None),
        ];
        for (path, want) in cases {
            assert_eq!(image_mime(path), want, "path {path}");
            assert_eq!(is_image_path(path), want.is_some(), "path {path}");
        }
    }

    #[test]
    fn works_on_both_path_separators() {
        assert_eq!(image_mime("src/assets/logo.png"), Some("image/png"));
        assert_eq!(image_mime("src\\assets\\logo.png"), Some("image/png"));
        assert_eq!(image_mime("C:\\Users\\me\\shot.webp"), Some("image/webp"));
    }

    #[test]
    fn a_dotfile_with_no_stem_is_not_an_image() {
        // ".png" has no filename before the dot — treat the whole thing
        // as an extension-less dotfile, not a PNG named "".
        assert_eq!(image_mime(".png"), None);
        assert_eq!(image_mime("dir/.png"), None);
    }

    #[test]
    fn a_trailing_dot_with_no_extension_is_not_an_image() {
        assert_eq!(image_mime("weird."), None);
    }
}
