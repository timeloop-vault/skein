//! The bytes a prompt becomes on the PTY: what xterm.js `Terminal.paste`
//! writes, so the daemon and the webview paste identically.

/// The submit keystroke, written after the paste.
pub const SUBMIT: &[u8] = b"\r";

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// The PTY bytes for pasting `body`. Every ESC is stripped first (bodies
/// are user-editable, and an embedded `ESC[201~` would end a bracketed
/// paste early), then `\r\n` and `\n` become `\r` as in xterm.js, and a
/// `bracketed` paste is wrapped in `ESC[200~` ... `ESC[201~`.
#[must_use]
pub fn paste_bytes(body: &str, bracketed: bool) -> Vec<u8> {
    let clean: String = body.chars().filter(|&c| c != '\u{1b}').collect();
    let normalized = clean.replace("\r\n", "\r").replace('\n', "\r");
    let mut out = Vec::with_capacity(normalized.len() + PASTE_START.len() + PASTE_END.len());
    if bracketed {
        out.extend_from_slice(PASTE_START);
    }
    out.extend_from_slice(normalized.as_bytes());
    if bracketed {
        out.extend_from_slice(PASTE_END);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_single_line() {
        assert_eq!(paste_bytes("hello", false), b"hello");
    }

    #[test]
    fn bracketed_wraps() {
        assert_eq!(paste_bytes("hi", true), b"\x1b[200~hi\x1b[201~");
    }

    #[test]
    fn newlines_become_carriage_returns() {
        assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(paste_bytes("a\nb", true), b"\x1b[200~a\rb\x1b[201~");
    }

    #[test]
    fn lone_carriage_return_is_kept() {
        assert_eq!(paste_bytes("a\rb", false), b"a\rb");
    }

    #[test]
    fn escapes_are_stripped_so_the_paste_cannot_end_early() {
        assert_eq!(
            paste_bytes("x\x1b[201~y", true),
            b"\x1b[200~x[201~y\x1b[201~"
        );
        assert_eq!(paste_bytes("\x1b\x1b", false), b"");
    }

    #[test]
    fn empty_body() {
        assert_eq!(paste_bytes("", true), b"\x1b[200~\x1b[201~");
        assert_eq!(paste_bytes("", false), b"");
    }

    #[test]
    fn non_ascii_is_utf8() {
        assert_eq!(paste_bytes("å", false), "å".as_bytes());
    }

    #[test]
    fn submit_is_a_carriage_return() {
        assert_eq!(SUBMIT, b"\r");
    }
}
