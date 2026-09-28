//! Opening a folder in Skein from outside it (epic #255): `skein .` in
//! a terminal, a `<scheme>://open?path=…` link, or a second launch of
//! the app with a path. Every door funnels into one pending slot and
//! one resolver, so they cannot disagree about which room a folder
//! means.
//!
//! **Delivery** copies the notification-click shape in `os_notify.rs`
//! (#155/#294): the request lands in a latest-wins slot, the window
//! comes to the front, and the frontend gets a payload-less poke. The
//! frontend reads the slot through [`open_request_take`] — on the poke
//! once rooms have loaded, and once right after hydrate, which is how a
//! path handed to a cold launch survives the webview not existing yet.
//!
//! **Resolution** happens at take time, not arrival time, so it sees
//! the rooms as they are when the frontend acts. It runs over the
//! stored rooms with the same matcher `find_rooms_for_path` (#354) uses
//! (`room_paths`), after canonicalizing *both* sides: the shell hands
//! over whatever spelling of the path it has, symlinks and all, and a
//! room may have been created through a different one.
//!
//! **Single instance is the prerequisite**, and not only for this. A
//! second Skein on the same `skein.db` would erase the first one's rooms
//! on its next autosave (`Database::save_all` replaces every row) and
//! revoke every agent-API token at boot. `tauri-plugin-single-instance`
//! turns a second launch into a call to [`from_second_instance`] in the
//! running process and exits the new one before it opens anything.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Url};
use tauri_plugin_deep_link::DeepLinkExt;

use crate::db::{Database, Room};
use crate::room_paths::{normalize_path_for_match, path_match_kind, strip_verbatim};

/// The poke. No payload — see the module doc.
pub(crate) const OPEN_REQUEST_EVENT: &str = "skein://open-request";

/// The path that arrived most recently and has not been taken yet.
/// A static rather than managed state because the single-instance
/// listener starts inside the plugin's own setup, before `setup()` has
/// managed anything — a second launch landing in that window must not
/// be dropped.
static PENDING: parking_lot::Mutex<Option<PathBuf>> = parking_lot::const_mutex(None);

// ── arrival ─────────────────────────────────────────────────────────

/// The path a command line names, if any. `args` excludes argv[0].
///
/// Flags are skipped — a Finder launch on older macOS carries
/// `-psn_…`, and a cold toast activation on Windows (#155) is
/// `-Embedding` — and so is anything containing `://`, which is a link
/// the deep-link plugin routes to [`link_action`] instead. Of what is
/// left the last argument wins. A relative path resolves against
/// `cwd`, the directory the *caller* was in, never Skein's own.
///
/// On Windows, one trailing `"` is trimmed from the chosen argument
/// (#393): `skein "C:\dir\"` arrives on argv as `C:\dir"`, because a
/// trailing backslash escapes the closing quote rather than ending it. A
/// literal trailing `"` is legal in a Unix filename, so this is
/// Windows-only. If trimming leaves nothing, the argument is treated as
/// no path at all rather than the cwd.
pub(crate) fn path_from_args<S: AsRef<str>>(args: &[S], cwd: &Path) -> Option<PathBuf> {
    let arg = args
        .iter()
        .map(AsRef::as_ref)
        .filter(|a| !a.is_empty() && !a.starts_with('-') && !a.contains("://"))
        .last()?;
    #[cfg(windows)]
    let arg = arg.strip_suffix('"').unwrap_or(arg);
    if arg.is_empty() {
        return None;
    }
    let path = Path::new(arg);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    })
}

/// What a link of ours asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkAction {
    /// `<scheme>://open` with no path: bring Skein to the front.
    Focus,
    /// `<scheme>://open?path=<absolute path>`.
    Open(PathBuf),
}

/// Read a deep link. `None` for a scheme that isn't one of `schemes`,
/// for any verb but `open`, and for a relative `path` — a link carries
/// no working directory to resolve one against, so guessing would open
/// the wrong folder rather than none.
///
/// The value is form-decoded (`query_pairs`), so a literal `+` in a
/// hand-typed link reads as a space; the `skein` command percent-encodes
/// every byte and never produces one.
pub(crate) fn link_action(url: &Url, schemes: &[String]) -> Option<LinkAction> {
    if !schemes.iter().any(|s| s == url.scheme()) {
        return None;
    }
    // `skein://open?…` parses with host `open`; tolerate `skein:open?…`
    // and `skein:///open?…` as well, which some launchers produce.
    let verb = url
        .host_str()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| url.path().trim_matches('/'));
    if verb != "open" {
        return None;
    }
    let Some(raw) = url
        .query_pairs()
        .find_map(|(k, v)| (k == "path").then(|| v.into_owned()))
        .filter(|v| !v.is_empty())
    else {
        return Some(LinkAction::Focus);
    };
    let path = PathBuf::from(raw);
    path.is_absolute().then_some(LinkAction::Open(path))
}

/// The URL schemes this build answers to, from the deep-link plugin's
/// config. Per build profile (`skein`, `skein-local`, `skein-dev`) so a
/// dev build never steals a link meant for the daily driver — the same
/// trap as the AUMID on #155.
pub(crate) fn configured_schemes(config: &tauri::Config) -> Vec<String> {
    config
        .plugins
        .0
        .get("deep-link")
        .and_then(|v| v.get("desktop"))
        .and_then(|v| v.get("schemes"))
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn set_pending(path: PathBuf) {
    tracing::info!(path = %path.display(), "open request received");
    *PENDING.lock() = Some(path);
}

fn take_pending() -> Option<PathBuf> {
    PENDING.lock().take()
}

/// Bring the main window to the front — asked for whether or not a
/// path came with the request, since plain `skein` means "go to Skein".
fn raise_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        tracing::warn!("open request: main window missing");
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    // `set_focus` alone loses to the foreground lock: the running
    // instance did not receive the input that launched the request.
    // Same trick the toast activation uses (#155).
    #[cfg(windows)]
    match window.hwnd() {
        Ok(hwnd) => {
            skein_winnotify::bring_to_front(hwnd.0 as isize);
        }
        Err(e) => tracing::warn!(error = %e, "open request: hwnd lookup failed"),
    }
}

/// Raise the window and, with a path, queue it and poke the frontend.
fn deliver(app: &AppHandle, path: Option<PathBuf>) {
    raise_main_window(app);
    if let Some(path) = path {
        set_pending(path);
        if let Err(e) = app.emit(OPEN_REQUEST_EVENT, ()) {
            tracing::warn!(error = %e, "open request: poke failed");
        }
    }
}

fn deliver_link(app: &AppHandle, url: &Url, schemes: &[String]) {
    match link_action(url, schemes) {
        Some(LinkAction::Focus) => deliver(app, None),
        Some(LinkAction::Open(path)) => deliver(app, Some(path)),
        None => tracing::warn!(%url, "open request: ignoring a link Skein does not handle"),
    }
}

/// The single-instance callback: a second launch, already exited,
/// forwarded its argv (argv[0] included) and working directory here.
/// A link launch on Windows/Linux arrives this way too; the deep-link
/// plugin routes that one to [`init`]'s listener, so here it only
/// raises the window.
///
/// The forward itself cannot split a path containing its own delimiter
/// (#393): `tauri-plugin-single-instance` 2.4.5 joins argv with `|` only
/// on Windows, where `|` is illegal in a file name anyway; macOS forwards
/// argv joined with `\0`; Linux forwards it as a genuine string list over
/// `DBus`, never joined at all.
pub(crate) fn from_second_instance(app: &AppHandle, argv: &[String], cwd: &str) {
    let after_exe = argv.get(1..).unwrap_or_default();
    deliver(app, path_from_args(after_exe, Path::new(cwd)));
}

/// Pick up whatever this process itself was launched with, and listen
/// for links from now on. Called from `setup()`.
pub(crate) fn init(app: &tauri::App) {
    // A path on our own command line. No poke: nothing is listening
    // yet, and the post-hydrate drain is exactly what picks it up.
    let own_args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Some(path) = path_from_args(&own_args, &cwd) {
        set_pending(path);
    }

    let schemes = configured_schemes(app.config());
    let handle = app.handle().clone();
    let link_schemes = schemes.clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            deliver_link(&handle, &url, &link_schemes);
        }
    });
    // A cold launch *by* a link on Windows/Linux: the plugin read argv
    // during its own setup, before the listener above existed. (macOS
    // delivers a cold-launch link as a later `Opened` event instead,
    // which the listener catches.)
    if let Ok(Some(urls)) = app.deep_link().get_current() {
        for url in urls {
            deliver_link(app.handle(), &url, &schemes);
        }
    }
    // An installed build's scheme is registered by its installer. A
    // dev build never runs one, so register it here — Windows and
    // Linux only; macOS reads the scheme from the bundle's Info.plist.
    #[cfg(all(debug_assertions, any(windows, target_os = "linux")))]
    if let Err(e) = app.deep_link().register_all() {
        tracing::warn!(error = %e, "open request: registering the dev link scheme failed");
    }
}

// ── resolution ──────────────────────────────────────────────────────

/// What the frontend should do with a taken request.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum OpenTarget {
    /// Rooms whose folder is, or contains, the requested one — all
    /// equally good, best first. Open rooms beat archived ones, so the
    /// list is either all open (the frontend keeps you in the room you
    /// are already in if it is one of them) or all archived (most
    /// recently archived first, to be reopened). `folder` is what to
    /// seed New room with if none of them is still around.
    Room {
        room_ids: Vec<String>,
        archived: bool,
        folder: String,
    },
    /// No room owns the folder: open New room seeded with it.
    NewRoom { folder: String },
    /// The path does not exist (or cannot be read).
    Missing { path: String },
}

/// A room as the matcher sees it: its folder, canonicalized where it
/// still exists, then normalized.
struct Candidate<'a> {
    room: &'a Room,
    cwd_norm: String,
}

/// Every room tied for best owner of `query_norm`. Deepest folder wins
/// (rooms can nest); at that folder open rooms beat archived ones; and
/// archived ones rank most recently archived first. A room whose folder
/// sits *under* the query is no owner at all — `skein ~/code` must not
/// jump into one of the dozens of rooms below it.
fn best_rooms<'a>(query_norm: &str, candidates: &[Candidate<'a>]) -> Vec<&'a Room> {
    let owners: Vec<&Candidate<'a>> = candidates
        .iter()
        // A retired room (#417) is history and owns nothing.
        .filter(|c| c.room.retired.is_none())
        .filter(|c| {
            matches!(
                path_match_kind(query_norm, &c.cwd_norm),
                Some("cwd" | "inside_room")
            )
        })
        .collect();
    let Some(deepest) = owners.iter().map(|c| c.cwd_norm.len()).max() else {
        return Vec::new();
    };
    let at_deepest = owners.into_iter().filter(|c| c.cwd_norm.len() == deepest);
    let (open, mut archived): (Vec<_>, Vec<_>) = at_deepest
        .map(|c| c.room)
        .partition(|r| r.archived.is_none());
    if !open.is_empty() {
        return open;
    }
    archived.sort_by_key(|r| Reverse(r.archived));
    archived
}

/// Canonicalize and render as a string a caller can use — the frontend
/// seeds New room with this, so it must never carry a Windows verbatim
/// (`\\?\`) prefix (#393): `dunce::canonicalize` avoids that prefix for an
/// ordinary path, but still emits it for a UNC path, one over 260
/// characters, or one hitting a reserved name.
fn canonical_string(path: &Path) -> Option<String> {
    dunce::canonicalize(path)
        .ok()
        .map(|p| strip_verbatim(&p.to_string_lossy()).into_owned())
}

/// Resolve `requested` against `rooms`. Touches the disk — canonicalize
/// per room, and a repo walk when nothing matches — so callers run it
/// off the main thread.
pub(crate) fn resolve(rooms: &[Room], requested: &Path) -> OpenTarget {
    let Ok(canon) = dunce::canonicalize(requested) else {
        return OpenTarget::Missing {
            path: requested.to_string_lossy().into_owned(),
        };
    };
    // A file opens the folder it sits in. Opening the file itself in
    // the room's `files` harness (#185) is a later slice of #255.
    let folder = if canon.is_dir() {
        canon
    } else {
        canon
            .parent()
            .map_or_else(|| canon.clone(), Path::to_path_buf)
    };
    // Stripped before it ever leaves this function: `folder_str` reaches
    // the frontend both directly (`Room.folder`) and via `canonical_string`
    // below, and New room must never be seeded with a verbatim path (#393).
    let folder_str = strip_verbatim(&folder.to_string_lossy()).into_owned();
    let query = normalize_path_for_match(&folder_str);

    let candidates: Vec<Candidate> = rooms
        .iter()
        .filter_map(|room| {
            let cwd = room.cwd.as_deref().filter(|c| !c.is_empty())?;
            // A folder that no longer exists keeps its stored spelling:
            // it cannot contain anything that does exist anyway.
            let cwd = canonical_string(Path::new(cwd)).unwrap_or_else(|| cwd.to_owned());
            Some(Candidate {
                room,
                cwd_norm: normalize_path_for_match(&cwd),
            })
        })
        .collect();

    let best = best_rooms(&query, &candidates);
    if let Some(first) = best.first() {
        return OpenTarget::Room {
            archived: first.archived.is_some(),
            room_ids: best.iter().map(|r| r.id.clone()).collect(),
            folder: folder_str,
        };
    }

    // Seed New room with the checkout the folder is in, not the folder
    // itself: a shell is usually somewhere deep inside a repo, and the
    // dialog only recognises a repo at its root. It then resolves a
    // linked worktree to its main checkout on its own, as it would for
    // a folder picked by hand.
    let seed = skein_git::Repo::enclosing_workdir(&folder)
        .and_then(|w| canonical_string(&w))
        .unwrap_or(folder_str);
    OpenTarget::NewRoom { folder: seed }
}

/// Put a path back into the pending slot, but only if it is still empty
/// (#393). A caller reaches for this after a failed load, to give the
/// request back to whoever takes next — but a newer request may have
/// arrived while the load was in flight, and latest-wins means that one
/// keeps its spot rather than being clobbered by the older, failed one.
fn restore_pending(path: PathBuf) {
    let mut pending = PENDING.lock();
    if pending.is_none() {
        *pending = Some(path);
    }
}

/// Take the pending request, if any, and resolve it via `load_rooms`.
/// `None` when nothing was pending. On a load failure the path is *not*
/// lost (#393): it is logged and handed back to [`restore_pending`], and
/// the error is returned so the caller can retry — a wholesale rooms-load
/// failure must not silently drop the folder the user asked to open.
fn take_and_resolve(
    load_rooms: impl FnOnce() -> Result<Vec<Room>, String>,
) -> Result<Option<OpenTarget>, String> {
    let Some(path) = take_pending() else {
        return Ok(None);
    };
    match load_rooms() {
        Ok(rooms) => {
            let target = resolve(&rooms, &path);
            tracing::info!(path = %path.display(), ?target, "open request resolved");
            Ok(Some(target))
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "open request: loading rooms failed, restoring the pending path"
            );
            restore_pending(path);
            Err(e)
        }
    }
}

/// Take (clearing) the pending request and resolve it against the
/// stored rooms. `None` when nothing is pending — a second poke for a
/// request the first one already took.
#[tauri::command]
pub async fn open_request_take(
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Option<OpenTarget>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || take_and_resolve(|| db.all_rooms()))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: &str, cwd: &str, archived: Option<i64>) -> Room {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": id,
            "task": "",
            "status": "idle",
            "badge": 0,
            "harnesses": [],
            "activeHarnessId": "",
            "cwd": cwd,
            "archived": archived,
        }))
        .unwrap()
    }

    fn ids(rooms: &[Room], query: &str) -> Vec<String> {
        let candidates: Vec<Candidate> = rooms
            .iter()
            .map(|r| Candidate {
                room: r,
                cwd_norm: normalize_path_for_match(r.cwd.as_deref().unwrap_or_default()),
            })
            .collect();
        best_rooms(&normalize_path_for_match(query), &candidates)
            .into_iter()
            .map(|r| r.id.clone())
            .collect()
    }

    #[test]
    fn args_skip_flags_and_links_and_resolve_against_the_callers_cwd() {
        let cwd = Path::new("/home/me/code");
        assert_eq!(
            path_from_args(&["."], cwd),
            Some(PathBuf::from("/home/me/code/."))
        );
        assert_eq!(
            path_from_args(&["-psn_0_123", "/abs/repo"], cwd),
            Some(PathBuf::from("/abs/repo"))
        );
        assert_eq!(path_from_args(&["-Embedding"], cwd), None);
        assert_eq!(path_from_args(&["skein://open?path=/x"], cwd), None);
        assert_eq!(path_from_args::<&str>(&[], cwd), None);
        // The last path wins.
        assert_eq!(
            path_from_args(&["a", "b"], cwd),
            Some(PathBuf::from("/home/me/code/b"))
        );
    }

    // `skein "C:\dir\"` arrives on argv with the closing quote glued onto
    // the path, since a trailing backslash escapes it rather than ending
    // the argument (#393).
    #[cfg(windows)]
    #[test]
    fn args_trim_one_trailing_quote_from_an_escaped_backslash() {
        let cwd = Path::new(r"C:\home\me\code");
        assert_eq!(
            path_from_args(&[r#"C:\dir""#], cwd),
            Some(PathBuf::from(r"C:\dir"))
        );
        // Only one quote is trimmed, never two.
        assert_eq!(
            path_from_args(&[r#"C:\dir"""#], cwd),
            Some(PathBuf::from(r#"C:\dir""#))
        );
        // Trimming down to nothing is no path at all.
        assert_eq!(path_from_args(&[r#"""#], cwd), None);
    }

    #[test]
    fn links_open_absolute_paths_and_refuse_everything_else() {
        let schemes = vec!["skein-dev".to_owned()];
        let parse = |s: &str| link_action(&Url::parse(s).unwrap(), &schemes);

        #[cfg(unix)]
        assert_eq!(
            parse("skein-dev://open?path=%2Fhome%2Fme%2Fmy%20repo"),
            Some(LinkAction::Open(PathBuf::from("/home/me/my repo")))
        );
        #[cfg(windows)]
        assert_eq!(
            parse("skein-dev://open?path=C%3A%5Cme%5Cmy%20repo"),
            Some(LinkAction::Open(PathBuf::from(r"C:\me\my repo")))
        );
        assert_eq!(parse("skein-dev://open"), Some(LinkAction::Focus));
        assert_eq!(parse("skein-dev://open?path="), Some(LinkAction::Focus));
        // Another profile's scheme is not ours.
        assert_eq!(parse("skein://open?path=%2Fx"), None);
        assert_eq!(parse("skein-dev://close?path=%2Fx"), None);
        // No working directory to resolve a relative path against.
        assert_eq!(parse("skein-dev://open?path=relative%2Fdir"), None);
    }

    #[test]
    fn the_deepest_owning_room_wins_and_a_contained_room_never_does() {
        let rooms = [
            room("outer", "/code/mono", None),
            room("inner", "/code/mono/packages/app", None),
            room("below", "/code/mono/packages/app/deep", None),
        ];
        assert_eq!(ids(&rooms, "/code/mono/packages/app/src"), ["inner"]);
        assert_eq!(ids(&rooms, "/code/mono"), ["outer"]);
        // A parent folder of every room owns none of them.
        assert!(ids(&rooms, "/code").is_empty());
    }

    #[test]
    fn open_rooms_beat_archived_ones_at_the_same_folder() {
        let rooms = [
            room("old", "/code/repo", Some(100)),
            room("live", "/code/repo", None),
        ];
        assert_eq!(ids(&rooms, "/code/repo"), ["live"]);
    }

    #[test]
    fn archived_rooms_rank_most_recently_archived_first() {
        let rooms = [
            room("older", "/code/repo", Some(100)),
            room("newer", "/code/repo", Some(300)),
            room("middle", "/code/repo", Some(200)),
        ];
        assert_eq!(ids(&rooms, "/code/repo"), ["newer", "middle", "older"]);
    }

    #[test]
    fn every_open_room_at_the_folder_is_returned_for_the_frontend_to_pick() {
        let rooms = [room("a", "/code/repo", None), room("b", "/code/repo", None)];
        assert_eq!(ids(&rooms, "/code/repo"), ["a", "b"]);
    }

    #[test]
    fn resolve_reports_a_missing_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let gone = tmp.path().join("nope");
        assert_eq!(
            resolve(&[], &gone),
            OpenTarget::Missing {
                path: gone.to_string_lossy().into_owned()
            }
        );
    }

    fn retired(mut r: Room, at: i64) -> Room {
        r.retired = Some(at);
        r
    }

    #[test]
    fn a_retired_room_owns_nothing_so_the_folder_resolves_to_new_room() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = canonical_string(tmp.path()).unwrap();
        let rooms = [retired(room("gone", &cwd, Some(100)), 200)];
        assert_eq!(
            resolve(&rooms, tmp.path()),
            OpenTarget::NewRoom { folder: cwd }
        );
    }

    #[test]
    fn a_retired_deeper_owner_yields_to_a_plain_archived_shallower_one() {
        let rooms = [
            room("shallow", "/code/mono", Some(100)),
            retired(room("deep", "/code/mono/app", Some(300)), 400),
        ];
        assert_eq!(ids(&rooms, "/code/mono/app/src"), ["shallow"]);
    }

    #[test]
    fn an_open_room_is_unaffected_by_retired_neighbours() {
        let rooms = [
            retired(room("gone", "/code/repo", Some(100)), 200),
            room("live", "/code/repo", None),
        ];
        assert_eq!(ids(&rooms, "/code/repo"), ["live"]);
    }

    #[test]
    fn resolve_opens_a_files_folder_and_seeds_new_room_with_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("notes.txt");
        std::fs::write(&file, "x").unwrap();
        let folder = canonical_string(tmp.path()).unwrap();
        assert_eq!(resolve(&[], &file), OpenTarget::NewRoom { folder });
    }

    #[test]
    fn resolve_seeds_new_room_with_the_enclosing_checkout() {
        let tmp = tempfile::TempDir::new().unwrap();
        git2::Repository::init(tmp.path()).unwrap();
        let nested = tmp.path().join("src");
        std::fs::create_dir(&nested).unwrap();
        let root = canonical_string(tmp.path()).unwrap();
        assert_eq!(resolve(&[], &nested), OpenTarget::NewRoom { folder: root });
    }

    // The shell's spelling and the room's spelling of one folder can
    // differ by a symlink — macOS's own `/tmp` → `/private/tmp` is the
    // everyday case. Both sides are canonicalized before matching.
    #[cfg(unix)]
    #[test]
    fn resolve_matches_through_a_symlink_on_either_side() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let via_real = [room("r", &real.to_string_lossy(), None)];
        let via_link = [room("r", &link.to_string_lossy(), None)];
        let want = OpenTarget::Room {
            room_ids: vec!["r".to_owned()],
            archived: false,
            folder: canonical_string(&real).unwrap(),
        };
        assert_eq!(resolve(&via_real, &link), want);
        assert_eq!(resolve(&via_link, &real), want);
    }

    /// A path long enough that `dunce::canonicalize` falls back to a
    /// Windows verbatim (`\\?\`) spelling (#393): neither `resolve`'s
    /// `OpenTarget::NewRoom.folder` nor a room match through it may leak
    /// that prefix to the frontend.
    #[cfg(windows)]
    #[test]
    fn resolve_strips_a_verbatim_prefix_from_a_long_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut long = tmp.path().to_path_buf();
        while long.to_string_lossy().len() < 260 {
            long = long.join("a".repeat(50));
        }
        std::fs::create_dir_all(&long).unwrap();

        let target = resolve(&[], &long);
        let OpenTarget::NewRoom { folder } = &target else {
            panic!("expected NewRoom, got {target:?}");
        };
        assert!(
            !folder.starts_with(r"\\?\"),
            "folder leaked a verbatim prefix: {folder}"
        );

        // A room stored with the plain (non-prefixed) long spelling still
        // resolves as its owner.
        let plain_cwd = long.to_string_lossy().into_owned();
        let rooms = [room("r", &plain_cwd, None)];
        assert_eq!(
            resolve(&rooms, &long),
            OpenTarget::Room {
                room_ids: vec!["r".to_owned()],
                archived: false,
                folder: folder.clone(),
            }
        );
    }

    // `PENDING` is a process-global static; every assertion touching it
    // lives in this one test so parallel tests can't race on it (#393).
    #[test]
    fn take_and_resolve_survives_a_failed_load() {
        let tmp = tempfile::TempDir::new().unwrap();
        let folder = canonical_string(tmp.path()).unwrap();

        // A failing loader leaves the path pending rather than losing it.
        *PENDING.lock() = Some(tmp.path().to_path_buf());
        let err = take_and_resolve(|| Err("db locked".to_owned()));
        assert_eq!(err, Err("db locked".to_owned()));
        assert_eq!(*PENDING.lock(), Some(tmp.path().to_path_buf()));

        // A subsequent successful call takes the restored path.
        let ok = take_and_resolve(|| Ok(Vec::new()));
        assert_eq!(ok, Ok(Some(OpenTarget::NewRoom { folder })));
        assert_eq!(*PENDING.lock(), None);

        // `restore_pending` never overwrites a newer pending path — a
        // second request may have arrived while the failed load ran.
        *PENDING.lock() = Some(PathBuf::from("/newer"));
        restore_pending(PathBuf::from("/older"));
        assert_eq!(*PENDING.lock(), Some(PathBuf::from("/newer")));

        // Do not leak this static's state into any other test.
        *PENDING.lock() = None;
    }
}
