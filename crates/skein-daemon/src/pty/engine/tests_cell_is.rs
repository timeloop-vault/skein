//! `cell_is` (the allocation-free comparison the damage check uses) must say
//! exactly what `cell_dump(cell) == dump` says. Two functions that spell out
//! the same fields can drift apart silently; this pins them together.

use super::AlacrittyEngine;
use super::tests::feed;

const SAMPLE: &str = concat!(
    "plain text\r\n",
    "\x1b[1mbold\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munder\x1b[0m \x1b[2mdim\x1b[0m\r\n",
    "\x1b[31mred\x1b[0m \x1b[38;5;208mc256\x1b[0m \x1b[38;2;10;20;30mtrue\x1b[0m ",
    "\x1b[44;1mblue bold\x1b[0m \x1b[7minv\x1b[0m \x1b[9mstrike\x1b[0m\r\n",
    "\u{4e2d}\u{6587} e\u{301}x \u{1f600}\r\n",
    "\x1b]8;;https://example.com\x07link\x1b]8;;\x07 after\r\n",
);

#[test]
fn cell_is_agrees_with_cell_dump_on_every_cell_of_a_varied_screen() {
    let mut e = AlacrittyEngine::new(8, 40);
    feed(&mut e, SAMPLE);
    let matrix = e.cell_is_matrix();
    assert_eq!(matrix.len(), 8 * 40);
    let dumps: Vec<_> = matrix.iter().map(|(d, _)| d).collect();
    // The sample really is varied, or the checks below prove little.
    let distinct = dumps
        .iter()
        .enumerate()
        .filter(|(i, d)| !dumps[..*i].contains(d))
        .count();
    assert!(distinct >= 15, "only {distinct} distinct cells");
    assert!(dumps.iter().any(|d| d.link.is_some()));
    assert!(dumps.iter().any(|d| d.width == 2));
    assert!(dumps.iter().any(|d| d.width == 0));
    assert!(dumps.iter().any(|d| d.text.chars().count() > 1));
    for (i, (dump, row)) in matrix.iter().enumerate() {
        assert!(row[i], "cell {i} is not its own dump: {dump:?}");
        for (j, other) in dumps.iter().enumerate() {
            assert_eq!(
                row[j],
                dump == *other,
                "cell {i} vs the dump of cell {j}: {dump:?} / {other:?}"
            );
        }
    }
}
