//! The preview HTTP surface: `GET /preview/{token}/{*path}` over the
//! room's worktree. Every response, errors included, carries
//! `no-store`, `nosniff` and `Access-Control-Allow-Origin: *`.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::PreviewState;
use super::rewrite::rewrite_html;

const PICKER: &str = include_str!("picker.js");

/// Largest file the preview will serve.
pub const MAX_SERVED: u64 = 32 * 1024 * 1024;

pub fn router(state: Arc<PreviewState>) -> Router {
    Router::new()
        .route(
            "/preview/{token}/{*path}",
            get(serve_file).options(preflight),
        )
        .fallback(|| async { plain(StatusCode::NOT_FOUND) })
        .layer(axum::middleware::map_response(standard_headers))
        .with_state(state)
}

/// The headers every reply carries, including axum's own 405s.
async fn standard_headers(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

async fn preflight() -> Response {
    let mut res = StatusCode::NO_CONTENT.into_response();
    let h = res.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, HEAD, OPTIONS"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("*"),
    );
    res
}

/// Serve until the process ends.
pub async fn serve(listener: tokio::net::TcpListener, state: Arc<PreviewState>) {
    if let Err(e) = axum::serve(listener, router(state)).await {
        tracing::error!(error = %e, "design preview: server stopped");
    }
}

/// Why a relative path was refused. Always surfaced as a plain 404.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    Empty,
    Nul,
    Traversal,
    Absolute,
    Outside,
    NotAFile,
    Unreadable,
}

/// Resolve `rel` under `root`, refusing anything that could leave it.
/// Symlinks are followed by the final canonicalize, so one pointing out
/// of the root is caught by the `starts_with` check.
pub fn resolve_under(root: &Path, rel: &str) -> Result<PathBuf, Refusal> {
    if rel.is_empty() {
        return Err(Refusal::Empty);
    }
    if rel.contains('\0') {
        return Err(Refusal::Nul);
    }
    // `\` is a separator on Windows; treat it as one everywhere so a
    // smuggled `..\` is rejected on every platform.
    let rel = rel.replace('\\', "/");
    let mut joined = PathBuf::new();
    for comp in Path::new(&rel).components() {
        match comp {
            Component::Normal(part) => {
                // NTFS alternate data streams (`file:stream`) and drive
                // letters hide behind a colon.
                if cfg!(windows) && part.to_string_lossy().contains(':') {
                    return Err(Refusal::Absolute);
                }
                joined.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir => return Err(Refusal::Traversal),
            Component::RootDir | Component::Prefix(_) => return Err(Refusal::Absolute),
        }
    }
    if joined.as_os_str().is_empty() {
        return Err(Refusal::Empty);
    }
    let root = root.canonicalize().map_err(|_| Refusal::Unreadable)?;
    let full = root
        .join(joined)
        .canonicalize()
        .map_err(|_| Refusal::Unreadable)?;
    if !full.starts_with(&root) {
        return Err(Refusal::Outside);
    }
    if !full.is_file() {
        return Err(Refusal::NotAFile);
    }
    Ok(full)
}

/// `full` (already canonical, from [`resolve_under`]) as a
/// `/`-separated path relative to the room folder.
fn relative_to(root: &str, full: &Path) -> Option<String> {
    let root = Path::new(root).canonicalize().ok()?;
    let rel = full.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs" | "jsx" | "ts" | "tsx") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// A response with a content type; `standard_headers` adds the rest.
fn respond(status: StatusCode, content_type: &'static str, body: Vec<u8>) -> Response {
    let mut res = (status, body).into_response();
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    res
}

fn plain(status: StatusCode) -> Response {
    respond(status, "text/plain; charset=utf-8", Vec::new())
}

async fn serve_file(
    State(state): State<Arc<PreviewState>>,
    UrlPath((token, path)): UrlPath<(String, String)>,
) -> Response {
    let Some(root) = state.root_for(&token) else {
        return plain(StatusCode::NOT_FOUND);
    };
    let full = match resolve_under(Path::new(&root), &path) {
        Ok(full) => full,
        Err(refusal) => {
            tracing::debug!(?refusal, "design preview: refused");
            return plain(StatusCode::NOT_FOUND);
        }
    };
    let Ok(meta) = tokio::fs::metadata(&full).await else {
        return plain(StatusCode::NOT_FOUND);
    };
    if meta.len() > MAX_SERVED {
        return plain(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let Ok(bytes) = tokio::fs::read(&full).await else {
        return plain(StatusCode::NOT_FOUND);
    };
    // Recorded from the raw bytes, before any rewrite: the freshness
    // check recomputes the same digest from the file on disk.
    if let Some(rel) = state.root_for(&token).and_then(|r| relative_to(&r, &full)) {
        state.record_served(
            &token,
            &rel,
            crate::review_surface::element::digest_bytes(&bytes),
        );
    }
    let ctype = content_type(&full);
    let body = if ctype.starts_with("text/html") {
        match String::from_utf8(bytes) {
            Ok(src) => rewrite_html(&src, PICKER).into_bytes(),
            Err(e) => e.into_bytes(),
        }
    } else {
        bytes
    };
    respond(StatusCode::OK, ctype, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, Room};
    use tempfile::TempDir;

    fn dir_with(files: &[(&str, &str)]) -> TempDir {
        let d = TempDir::new().unwrap();
        for (name, body) in files {
            let p = d.path().join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        d
    }

    /// Create a file symlink, or None where the OS won't allow it.
    fn try_symlink(target: &Path, link: &Path) -> Option<()> {
        #[cfg(unix)]
        let r = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let r = std::os::windows::fs::symlink_file(target, link);
        r.ok()
    }

    #[test]
    fn resolves_a_plain_file() {
        let d = dir_with(&[("a/b.txt", "x")]);
        let got = resolve_under(d.path(), "a/b.txt").unwrap();
        assert!(got.ends_with("b.txt"));
    }

    #[test]
    fn refuses_traversal_in_every_spelling() {
        let d = dir_with(&[("a/b.txt", "x")]);
        for rel in ["../x", "a/../../x", "a/../b.txt", "..\\x", "a\\..\\..\\x"] {
            assert_eq!(
                resolve_under(d.path(), rel),
                Err(Refusal::Traversal),
                "{rel}"
            );
        }
    }

    #[test]
    fn refuses_absolute_empty_nul_and_directories() {
        let d = dir_with(&[("a/b.txt", "x")]);
        assert!(matches!(
            resolve_under(d.path(), "/etc/passwd"),
            Err(Refusal::Absolute)
        ));
        assert!(matches!(
            resolve_under(d.path(), "\\etc\\passwd"),
            Err(Refusal::Absolute)
        ));
        assert_eq!(resolve_under(d.path(), ""), Err(Refusal::Empty));
        assert_eq!(resolve_under(d.path(), "a\0b"), Err(Refusal::Nul));
        assert_eq!(resolve_under(d.path(), "a"), Err(Refusal::NotAFile));
        assert_eq!(resolve_under(d.path(), "nope"), Err(Refusal::Unreadable));
        #[cfg(windows)]
        assert!(resolve_under(d.path(), "C:/Windows/win.ini").is_err());
    }

    #[test]
    fn refuses_a_symlink_that_escapes_the_root() {
        let root = dir_with(&[("ok.txt", "x")]);
        let other = dir_with(&[("secret.txt", "s")]);
        let link = root.path().join("link.txt");
        if try_symlink(&other.path().join("secret.txt"), &link).is_none() {
            eprintln!("skipped: cannot create symlinks here");
            return;
        }
        assert_eq!(
            resolve_under(root.path(), "link.txt"),
            Err(Refusal::Outside)
        );
    }

    // ── over real HTTP ────────────────────────────────────────────

    fn room(id: &str, cwd: &Path) -> Room {
        Room {
            id: id.to_owned(),
            name: id.to_owned(),
            task: String::new(),
            status: "running".to_owned(),
            badge: 0,
            active_harness_id: String::new(),
            harnesses: Vec::new(),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            branch: None,
            repo: None,
            archived: None,
            repo_root: None,
            attention: None,
            created_by: None,
            closed_by: None,
            retired: None,
            repo_identity: None,
        }
    }

    async fn get(url: &str) -> reqwest::Response {
        reqwest::Client::new().get(url).send().await.unwrap()
    }

    #[tokio::test]
    async fn a_preview_url_for_room_a_cannot_read_a_file_of_room_b() {
        crate::install_rustls_provider();
        let a = dir_with(&[
            ("a.txt", "AAA"),
            (
                "page.html",
                "<html><head></head><body><script type=\"text/babel\">1</script></body></html>",
            ),
        ]);
        let b = dir_with(&[("secret.txt", "BBB")]);
        let b_name = b.path().file_name().unwrap().to_string_lossy().into_owned();

        let db_dir = TempDir::new().unwrap();
        let db = Arc::new(Database::open(&db_dir.path().join("skein.db")).unwrap());
        db.load_all().unwrap();
        db.save_all(&[room("ra", a.path()), room("rb", b.path())])
            .unwrap();

        let state = Arc::new(PreviewState::new(Arc::clone(&db)));
        let ta = state.mint("ra");
        let tb = state.mint("rb");
        assert_eq!(ta, state.mint("ra"), "mint is idempotent per room");
        assert_ne!(ta, tb);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        tokio::spawn(serve(listener, Arc::clone(&state)));

        // A's own file.
        let res = get(&format!("{base}/preview/{ta}/a.txt")).await;
        assert_eq!(res.status(), 200);
        let h = res.headers();
        assert_eq!(h["content-type"], "text/plain; charset=utf-8");
        assert_eq!(h["cache-control"], "no-store");
        assert_eq!(h["access-control-allow-origin"], "*");
        assert_eq!(h["x-content-type-options"], "nosniff");
        assert_eq!(res.text().await.unwrap(), "AAA");
        // The raw bytes' digest is remembered under the room, by
        // relative path (#434), and only for that room.
        assert_eq!(
            state.served_digests("ra")["a.txt"],
            crate::review_surface::element::digest_bytes(b"AAA")
        );
        assert!(state.served_digests("rb").is_empty());

        // B's file under B's own token works (the fixture is sound).
        assert_eq!(
            get(&format!("{base}/preview/{tb}/secret.txt"))
                .await
                .text()
                .await
                .unwrap(),
            "BBB"
        );

        // Escape attempts with A's token.
        for rel in [
            format!("../{b_name}/secret.txt"),
            format!("%2e%2e%2f{b_name}%2fsecret.txt"),
            format!("..%5c{b_name}%5csecret.txt"),
            format!("%2e%2e%5c{b_name}%5csecret.txt"),
            b.path()
                .join("secret.txt")
                .to_string_lossy()
                .replace('\\', "%5c"),
            format!(
                "%2f{}",
                b.path()
                    .join("secret.txt")
                    .to_string_lossy()
                    .trim_start_matches('/')
            ),
        ] {
            let res = get(&format!("{base}/preview/{ta}/{rel}")).await;
            assert_eq!(res.status(), 404, "{rel}");
            assert_eq!(res.headers()["access-control-allow-origin"], "*");
            assert!(!res.text().await.unwrap().contains("BBB"), "{rel}");
        }

        // A symlink inside A pointing at B's file.
        let link = a.path().join("link.txt");
        if try_symlink(&b.path().join("secret.txt"), &link).is_some() {
            assert_eq!(
                get(&format!("{base}/preview/{ta}/link.txt")).await.status(),
                404
            );
        } else {
            eprintln!("skipped: cannot create symlinks here");
        }

        // Unknown token, directory, missing file.
        assert_eq!(
            get(&format!("{base}/preview/nope/a.txt")).await.status(),
            404
        );
        assert_eq!(get(&format!("{base}/preview/{ta}/")).await.status(), 404);
        assert_eq!(
            get(&format!("{base}/preview/{ta}/missing.txt"))
                .await
                .status(),
            404
        );

        // OPTIONS is a preflight; other methods are 405. Both carry the headers.
        let client = reqwest::Client::new();
        let url = format!("{base}/preview/{ta}/a.txt");
        let res = client
            .request(reqwest::Method::OPTIONS, &url)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 204);
        let h = res.headers();
        assert_eq!(h["access-control-allow-methods"], "GET, HEAD, OPTIONS");
        assert_eq!(h["access-control-allow-headers"], "*");
        assert_eq!(h["access-control-allow-origin"], "*");
        assert_eq!(h["cache-control"], "no-store");
        assert_eq!(h["x-content-type-options"], "nosniff");
        let res = client.post(&url).send().await.unwrap();
        assert_eq!(res.status(), 405);
        let h = res.headers();
        assert_eq!(h["access-control-allow-origin"], "*");
        assert_eq!(h["cache-control"], "no-store");
        assert_eq!(h["x-content-type-options"], "nosniff");

        // HTML is rewritten.
        let html = get(&format!("{base}/preview/{ta}/page.html")).await;
        assert_eq!(html.headers()["content-type"], "text/html; charset=utf-8");
        let body = html.text().await.unwrap();
        assert!(body.contains("data-skein-picker"));
        assert!(body.contains("data-plugins=\"transform-react-jsx-source\""));
    }
}
