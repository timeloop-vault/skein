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
//! second Skein on the same `skein.db` would hold its own room store and
//! overwrite the first one's rows without seeing its changes, and
//! revoke every agent-API token at boot. `tauri-plugin-single-instance`
//! turns a second launch into a call to [`from_second_instance`] in the
//! running process and exits the new one before it opens anything.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Url};
use tauri_plugin_deep_link::DeepLinkExt;

use crate::db::{Database, Room};
use crate::git::{IdentityCheck, check_identity};
use crate::room_paths::{normalize_path_for_match, path_match_kind, strip_verbatim};

/// The poke. No payload — see the module doc.
pub(crate) const OPEN_REQUEST_EVENT: &str = "skein://open-request";

/// The path that arrived most recently and has not been taken yet.
/// A static rather than managed state because the single-instance
/// listener starts inside the plugin's own setup, before `setup()` has
/// managed anything — a second launch landing in that window must not
/// be dropped.
static PENDING: parking_lot::Mutex<Option<Pending>> = parking_lot::const_mutex(None);

/// What is waiting in [`PENDING`]. One slot for both kinds, so the latest
/// request wins whichever door it came through.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    /// A folder, resolved against the stored rooms at take time.
    Path(PathBuf),
    /// A room (and maybe a harness) named by id; the frontend resolves it.
    Room {
        room_id: String,
        harness_id: Option<String>,
    },
}

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
    /// `<scheme>://room/<roomId>[?harness=<harnessId>]`: jump to a room.
    Room {
        room_id: String,
        harness_id: Option<String>,
    },
}

/// An id a room link may carry: 1-64 of `[A-Za-z0-9_-]`. Skein's own ids
/// are uuids or short slugs; anything wider is refused rather than passed
/// on to the frontend.
fn valid_link_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Read a deep link. `None` for a scheme that isn't one of `schemes`,
/// for any verb but `open` and `room`, for a malformed room or harness
/// id (the whole link, not just the harness part), and for a relative `path` — a link carries
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
    // and `skein:///open?…` as well, which some launchers produce. Then
    // the verb is the first path segment and `rest` what follows it.
    let (verb, rest) = if let Some(host) = url.host_str().filter(|h| !h.is_empty()) {
        (host, url.path())
    } else {
        let path = url.path().trim_start_matches('/');
        path.split_once('/').unwrap_or((path, ""))
    };
    match verb {
        "open" if url.host_str().is_some_and(|h| !h.is_empty()) || rest.is_empty() => {
            open_action(url)
        }
        "room" => room_action(url, rest),
        _ => None,
    }
}

/// `<scheme>://room/<roomId>[?harness=<harnessId>]`, `rest` being what
/// follows the verb. One path segment, a trailing slash tolerated.
fn room_action(url: &Url, rest: &str) -> Option<LinkAction> {
    let room_id = rest.trim_matches('/');
    if !valid_link_id(room_id) {
        return None;
    }
    let harness_id = match url.query_pairs().find(|(k, _)| k == "harness") {
        Some((_, v)) if !v.is_empty() => {
            if !valid_link_id(&v) {
                return None;
            }
            Some(v.into_owned())
        }
        _ => None,
    };
    Some(LinkAction::Room {
        room_id: room_id.to_owned(),
        harness_id,
    })
}

/// `<scheme>://open[?path=<absolute path>]`.
fn open_action(url: &Url) -> Option<LinkAction> {
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
    *PENDING.lock() = Some(Pending::Path(path));
}

fn set_pending_room(room_id: String, harness_id: Option<String>) {
    tracing::info!(room_id = %room_id, harness_id = ?harness_id, "open request received");
    *PENDING.lock() = Some(Pending::Room {
        room_id,
        harness_id,
    });
}

fn take_pending() -> Option<Pending> {
    PENDING.lock().take()
}

/// Raise the window and, with a request, queue it and poke the frontend.
fn deliver(app: &AppHandle, pending: Option<Pending>) {
    crate::setup::raise_main_window(app);
    if let Some(pending) = pending {
        match pending {
            Pending::Path(path) => set_pending(path),
            Pending::Room {
                room_id,
                harness_id,
            } => set_pending_room(room_id, harness_id),
        }
        if let Err(e) = app.emit(OPEN_REQUEST_EVENT, ()) {
            tracing::warn!(error = %e, "open request: poke failed");
        }
    }
}

fn deliver_link(app: &AppHandle, url: &Url, schemes: &[String]) {
    match link_action(url, schemes) {
        Some(LinkAction::Focus) => deliver(app, None),
        Some(LinkAction::Open(path)) => deliver(app, Some(Pending::Path(path))),
        Some(LinkAction::Room {
            room_id,
            harness_id,
        }) => deliver(
            app,
            Some(Pending::Room {
                room_id,
                harness_id,
            }),
        ),
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
    deliver(
        app,
        path_from_args(after_exe, Path::new(cwd)).map(Pending::Path),
    );
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
    /// A room link (`<scheme>://room/<roomId>`): the ids exactly as the
    /// link carried them. Not checked against the stored rooms here - the
    /// frontend resolves them, and handles ids that no longer exist.
    Jump {
        room_id: String,
        harness_id: Option<String>,
    },
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
        // A folder that now holds a different repo than the room was made
        // in (#418) is not that room's folder any more. Checked after the
        // path filter: it opens repos, so only pay for path matches.
        .filter(|c| {
            let cwd = c.room.cwd.as_deref().unwrap_or_default();
            check_identity(c.room.repo_identity.as_ref(), Path::new(cwd)) != IdentityCheck::Mismatch
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

/// Put a request back into the pending slot, but only if it is still
/// empty (#393). A caller reaches for this after a failed load, to give
/// the request back to whoever takes next — but a newer request may have
/// arrived while the load was in flight, and latest-wins means that one
/// keeps its spot rather than being clobbered by the older, failed one.
fn restore_pending(request: Pending) {
    let mut pending = PENDING.lock();
    if pending.is_none() {
        *pending = Some(request);
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
    let path = match take_pending() {
        None => return Ok(None),
        // A room link names ids, not a folder: nothing to load or match.
        Some(Pending::Room {
            room_id,
            harness_id,
        }) => {
            tracing::info!(room_id = %room_id, harness_id = ?harness_id, "open request: room jump");
            return Ok(Some(OpenTarget::Jump {
                room_id,
                harness_id,
            }));
        }
        Some(Pending::Path(path)) => path,
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
            restore_pending(Pending::Path(path));
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

/// The URL schemes this build answers to (#813), so the terminal can open
/// its own links itself instead of deferring to a CLI that never will.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn open_request_link_schemes(app: AppHandle) -> Vec<String> {
    configured_schemes(app.config())
}

#[cfg(test)]
mod tests;
