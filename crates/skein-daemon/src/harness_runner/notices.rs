//! The pane notices that follow a PTY (#809, slice T-5): what the daemon
//! appends to the engine, in frames mode, once the child's output has ended.
//!
//! The wording is a copy, byte for byte, of the webview's own, which the view
//! still writes in raw mode: `exitedBanner` and `ENTER_FOR_SHELL` in
//! `app/src/terminalBanners.ts`, and `sidecarStoppedLine` in
//! `app/src/opencodeSidecarExit.ts`. Slice T-6 deletes the TS side; until
//! then change both together. The tests pin the bytes.

/// `ENTER_FOR_SHELL` (`terminalBanners.ts`).
const ENTER_FOR_SHELL: &str = "\x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell.\x1b[0m\r\n";

/// `exitedBanner(programName, code)`: the exit line plus the shell hint. An
/// unknown code reads `?`.
pub(super) fn exit_banner(program: &str, code: Option<u32>) -> Vec<u8> {
    let code = code.map_or_else(|| "?".to_owned(), |c| c.to_string());
    format!("\r\n\x1b[2m[skein] {program} exited ({code})\x1b[0m\r\n{ENTER_FOR_SHELL}").into_bytes()
}

/// `sidecarStoppedLine(code)` (`opencodeSidecarExit.ts`). Its line ends are
/// bare LFs in the TS source, and are kept so.
pub(super) fn sidecar_stopped_line(code: Option<i32>) -> Vec<u8> {
    let code = code.map_or_else(|| "?".to_owned(), |c| c.to_string());
    format!(
        "\x1b[2m[opencode server stopped (exit {code}) \u{2014} use Restart harness to reconnect]\x1b[0m\n\n"
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exit_banner_is_the_views_bytes() {
        assert_eq!(
            String::from_utf8(exit_banner("claude", Some(1))).unwrap(),
            "\r\n\x1b[2m[skein] claude exited (1)\x1b[0m\r\n\
             \x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell.\x1b[0m\r\n"
        );
    }

    #[test]
    fn an_unknown_exit_code_reads_a_question_mark() {
        let banner = String::from_utf8(exit_banner("opencode", None)).unwrap();
        assert!(banner.contains("opencode exited (?)"), "{banner:?}");
    }

    #[test]
    fn the_sidecar_line_is_the_views_bytes() {
        assert_eq!(
            String::from_utf8(sidecar_stopped_line(Some(3))).unwrap(),
            "\x1b[2m[opencode server stopped (exit 3) \u{2014} use Restart harness to reconnect]\x1b[0m\n\n"
        );
        let unknown = String::from_utf8(sidecar_stopped_line(None)).unwrap();
        assert!(unknown.contains("(exit ?)"), "{unknown:?}");
    }
}
