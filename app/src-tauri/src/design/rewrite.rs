//! HTML rewriting for served pages: inject the picker script and make
//! in-browser Babel emit source locations. Hand-rolled scanning; the
//! tree has no regex crate and this needs only tag starts.
//!
//! A page carrying its own `<meta http-equiv="Content-Security-Policy">`
//! is deliberately NOT rewritten around: the picker may be blocked, and
//! the pane detects that by the missing `ready` beacon instead.

/// Inject `picker`, then each `(attribute, source)` of `extras` as its own
/// script after it (in order, they read what the picker leaves), and add
/// `data-plugins` to Babel scripts.
pub fn rewrite_html(src: &str, picker: &str, extras: &[(&str, &str)]) -> String {
    let src = tag_babel_scripts(src);
    let mut script = format!("<script data-skein-picker>{picker}</script>");
    for (attr, source) in extras {
        script.push_str("<script ");
        script.push_str(attr);
        script.push('>');
        script.push_str(source);
        script.push_str("</script>");
    }
    let lower = src.to_ascii_lowercase();
    let at = if let Some(i) = close_head(&lower) {
        i
    } else if let Some(end) = open_tag_end(&lower, "<head") {
        end
    } else {
        open_tag_end(&lower, "<html").unwrap_or_else(|| prologue_end(&lower))
    };
    let mut out = String::with_capacity(src.len() + script.len());
    out.push_str(&src[..at]);
    out.push_str(&script);
    out.push_str(&src[at..]);
    out
}

/// Offset just past any leading `<!doctype ...>` and comments (with
/// whitespace between them), so an insertion there keeps the page out of
/// quirks mode. 0 when there is none. `lower` must be ASCII-lowercased.
fn prologue_end(lower: &str) -> usize {
    let mut end = 0;
    let mut pos = 0;
    loop {
        pos += lower[pos..].len() - lower[pos..].trim_start().len();
        let rest = &lower[pos..];
        let len = if rest.starts_with("<!--") {
            rest.find("-->").map(|i| i + 3)
        } else if rest.starts_with("<!doctype") {
            rest.find('>').map(|i| i + 1)
        } else {
            None
        };
        let Some(len) = len else {
            return end;
        };
        pos += len;
        end = pos;
    }
}

/// Offset of the first `</head>` end tag, not `</header>`.
fn close_head(lower: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = lower[from..].find("</head") {
        let start = from + rel;
        let after = start + "</head".len();
        if matches!(
            lower.as_bytes().get(after),
            Some(b'>' | b' ' | b'\t' | b'\n' | b'\r')
        ) {
            return Some(start);
        }
        from = after;
    }
    None
}

/// Byte offset just past the `>` of the first `<name ...>` tag, where
/// `name` is followed by whitespace, `>` or `/` (so `<header>` is not
/// `<head`). `lower` must be ASCII-lowercased.
fn open_tag_end(lower: &str, open: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = lower[from..].find(open) {
        let start = from + rel;
        let after = start + open.len();
        match lower.as_bytes().get(after) {
            Some(b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r') => {
                return tag_close(lower, after);
            }
            _ => from = after,
        }
    }
    None
}

/// Offset just past the `>` closing a tag whose attributes start at
/// `from`, skipping over quoted values.
fn tag_close(s: &str, from: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (i, &b) in s.as_bytes().iter().enumerate().skip(from) {
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'>') => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// `(name, value)` pairs of a start tag's attribute text (everything
/// between the tag name and the closing `>`). Values may be double-quoted,
/// single-quoted or bare; a valueless attribute has an empty value. Pass
/// lowercased text for case-insensitive matching.
fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let skip_ws = |mut i: usize| {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        i
    };
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        i = skip_ws(i);
        while b.get(i) == Some(&b'/') {
            i = skip_ws(i + 1);
        }
        if i >= b.len() {
            return out;
        }
        let name_start = i;
        while b
            .get(i)
            .is_some_and(|c| !c.is_ascii_whitespace() && !matches!(c, b'=' | b'/'))
        {
            i += 1;
        }
        if i == name_start {
            // A stray `=`; step over it.
            i += 1;
            continue;
        }
        let name = s[name_start..i].to_owned();
        i = skip_ws(i);
        let mut value = "";
        if b.get(i) == Some(&b'=') {
            i = skip_ws(i + 1);
            let (vs, stop) = match b.get(i) {
                Some(&q @ (b'"' | b'\'')) => (i + 1, Some(q)),
                _ => (i, None),
            };
            i = vs;
            while b
                .get(i)
                .is_some_and(|&c| stop.map_or(!c.is_ascii_whitespace(), |q| c != q))
            {
                i += 1;
            }
            value = &s[vs..i];
            if stop.is_some() {
                i += 1;
            }
        }
        out.push((name, value.to_owned()));
    }
}

/// Add `data-plugins="transform-react-jsx-source"` to every
/// `<script type="text/babel">` that has no `data-plugins` yet.
fn tag_babel_scripts(src: &str) -> String {
    let lower = src.to_ascii_lowercase();
    let mut out = String::with_capacity(src.len());
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find("<script") {
        let start = pos + rel;
        let name_end = start + "<script".len();
        let boundary = matches!(
            lower.as_bytes().get(name_end),
            Some(b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r')
        );
        let Some(end) = boundary.then(|| tag_close(&lower, name_end)).flatten() else {
            out.push_str(&src[pos..name_end]);
            pos = name_end;
            continue;
        };
        let attrs = parse_attrs(&lower[name_end..end - 1]);
        let is_babel = attrs.iter().any(|(n, v)| n == "type" && v == "text/babel");
        let has_plugins = attrs.iter().any(|(n, _)| n == "data-plugins");
        out.push_str(&src[pos..name_end]);
        if is_babel && !has_plugins {
            out.push_str(" data-plugins=\"transform-react-jsx-source\"");
        }
        out.push_str(&src[name_end..end]);
        pos = end;
    }
    out.push_str(&src[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "PICK";
    const S: &str = "<script data-skein-picker>PICK</script>";

    #[test]
    fn injects_before_closing_head() {
        let out = rewrite_html(
            "<html><head><title>x</title></head><body></body></html>",
            P,
            &[],
        );
        assert_eq!(
            out,
            format!("<html><head><title>x</title>{S}</head><body></body></html>")
        );
    }

    #[test]
    fn injects_before_uppercase_head() {
        let out = rewrite_html("<HTML><HEAD></HEAD><BODY></BODY></HTML>", P, &[]);
        assert_eq!(out, format!("<HTML><HEAD>{S}</HEAD><BODY></BODY></HTML>"));
    }

    #[test]
    fn falls_back_to_after_open_head_then_html_then_prepend() {
        assert_eq!(
            rewrite_html("<head lang=\"a>b\"><p>", P, &[]),
            format!("<head lang=\"a>b\">{S}<p>")
        );
        assert_eq!(
            rewrite_html("<html lang=en><body>", P, &[]),
            format!("<html lang=en>{S}<body>")
        );
        assert_eq!(rewrite_html("<p>hi</p>", P, &[]), format!("{S}<p>hi</p>"));
    }

    #[test]
    fn extra_scripts_follow_the_picker_in_order() {
        assert_eq!(
            rewrite_html("<head></head>", P, &[("data-a", "A"), ("data-b", "B")]),
            format!("<head>{S}<script data-a>A</script><script data-b>B</script></head>")
        );
    }

    #[test]
    fn header_element_is_not_head() {
        assert_eq!(
            rewrite_html("<header>x</header>", P, &[]),
            format!("{S}<header>x</header>")
        );
    }

    #[test]
    fn babel_script_gets_data_plugins() {
        let out = rewrite_html(
            "<script type=\"text/babel\" src=\"a.jsx\"></script><SCRIPT TYPE='text/babel'>x</SCRIPT>",
            P,
            &[],
        );
        assert_eq!(
            out.matches("data-plugins=\"transform-react-jsx-source\"")
                .count(),
            2
        );
    }

    #[test]
    fn headless_page_keeps_its_doctype_first() {
        assert_eq!(
            rewrite_html("<!DOCTYPE html>\n<p>hi</p>", P, &[]),
            format!("<!DOCTYPE html>{S}\n<p>hi</p>")
        );
        assert_eq!(
            rewrite_html("  <!-- a -->\n<!doctype html><p>", P, &[]),
            format!("  <!-- a -->\n<!doctype html>{S}<p>")
        );
        assert_eq!(rewrite_html("<!-- open", P, &[]), format!("{S}<!-- open"));
    }

    fn plugins(src: &str) -> usize {
        rewrite_html(src, P, &[])
            .matches("data-plugins=\"transform-react-jsx-source\"")
            .count()
    }

    #[test]
    fn babel_type_attribute_is_parsed_not_substring_matched() {
        assert_eq!(plugins("<script type = \"text/babel\"></script>"), 1);
        assert_eq!(plugins("<script type=text/babel></script>"), 1);
        assert_eq!(plugins("<script TYPE='TEXT/Babel' src=a.jsx></script>"), 1);
        assert_eq!(plugins("<script data-type=\"text/babel\"></script>"), 0);
        assert_eq!(
            plugins("<script type=\"text/babel\" data-plugins=\"x\"></script>"),
            0
        );
    }

    #[test]
    fn existing_data_plugins_and_other_scripts_untouched() {
        let src = "<script type=\"text/babel\" data-plugins=\"mine\"></script><script src=\"a.js\"></script>";
        let out = rewrite_html(src, P, &[]);
        assert!(!out.contains("transform-react-jsx-source"));
        assert!(out.ends_with(src));
    }
}
