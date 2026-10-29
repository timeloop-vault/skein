//! Byte framing for a Server-Sent Events stream, as opencode v2's
//! `GET /api/event` speaks it: `data: {json}` blocks separated by a blank
//! line, with `: heartbeat` comment blocks in between.

/// Removes every COMPLETE event block from `buf` and returns each block's
/// `data:` payload (multiple `data:` lines joined with `\n`, per the SSE spec).
///
/// A block ends at a blank line, so both `\n\n` and `\r\n\r\n` terminate one.
/// Comment-only blocks (`: heartbeat`) and blocks with no data are skipped;
/// `event:` / `id:` / `retry:` lines are ignored. A trailing partial block is
/// left in `buf` for the next chunk, so a block split at any byte (even inside
/// a UTF-8 sequence) is only decoded once it is whole.
pub fn drain_sse_data(buf: &mut Vec<u8>) -> Vec<String> {
    let mut out = Vec::new();
    let mut consumed = 0;
    let mut block_start = 0;
    let mut line_start = 0;
    while let Some(rel) = buf[line_start..].iter().position(|&b| b == b'\n') {
        let nl = line_start + rel;
        let mut line = &buf[line_start..nl];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        if line.is_empty() {
            if let Some(payload) = block_payload(&buf[block_start..line_start]) {
                out.push(payload);
            }
            consumed = nl + 1;
            block_start = consumed;
        }
        line_start = nl + 1;
    }
    buf.drain(..consumed);
    out
}

/// The joined `data:` payload of one block, or `None` when it has none.
fn block_payload(block: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(block);
    let mut parts: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        parts.push(rest.strip_prefix(' ').unwrap_or(rest));
    }
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join("\n");
    (!joined.is_empty()).then_some(joined)
}
