//! The `skein` shell command (epic #255): what Settings → Command line
//! installs so `skein .` in a terminal opens that folder in Skein — the
//! `code .` pattern, including VS Code's "install the command from the
//! app" step, because a macOS .app's binary is never on `PATH`.
//!
//! The command is a small POSIX `sh` script in `~/.local/bin`, named
//! after the build's link scheme (`skein`, `skein-local`, `skein-dev`)
//! so profiles never shadow each other. It hands the path over one of
//! two ways:
//!
//! - **A bundled macOS app** gets a `<scheme>://open?path=…` link via
//!   `open`. `LaunchServices` then keeps one instance, starts Skein if it
//!   isn't running, and leaves it detached from the terminal — no
//!   SIGHUP when the tab closes, no log lines printed into it. Every
//!   byte of the path is percent-encoded, so no spelling of a path can
//!   break the link.
//! - **Everything else** (a dev binary, Linux) runs the binary itself,
//!   detached. When Skein is already running, the single-instance
//!   plugin forwards the path to it and the new process exits.
//!
//! Windows has no installer yet: it needs a `.cmd` shim plus a user
//! `PATH` edit and a settings-change broadcast, none of which can be
//! verified from here. The status reports that honestly instead.
//!
//! Skein only ever overwrites or removes a file carrying [`MARKER`] — a
//! `skein` someone else put in `~/.local/bin` is reported, never
//! replaced.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};

/// First comment line of every script Skein writes; how a file is
/// known to be ours.
const MARKER: &str = "# skein-cli-shim v1";

/// How the script reaches Skein.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Launch {
    /// A bundled macOS app: a link through `LaunchServices`.
    Link { scheme: String },
    /// Anything else: run this binary, detached.
    Exec { exe: PathBuf },
}

/// Pick the [`Launch`] for this process. `appimage` is `$APPIMAGE` —
/// an `AppImage`'s `current_exe` is inside a mount that vanishes when it
/// quits, so the script must run the image file itself.
fn launch_for(exe: &Path, appimage: Option<PathBuf>, scheme: &str, is_macos: bool) -> Launch {
    let in_bundle = exe.to_string_lossy().contains(".app/Contents/MacOS/");
    if is_macos && in_bundle {
        return Launch::Link {
            scheme: scheme.to_owned(),
        };
    }
    Launch::Exec {
        exe: appimage.unwrap_or_else(|| exe.to_path_buf()),
    }
}

/// Single-quote `s` for `sh`.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The script, in full.
fn shim_script(command: &str, launch: &Launch) -> String {
    let name = sh_quote(command);
    // `CDPATH=` stops `cd` echoing a directory into the capture.
    let resolve = format!(
        r#"target=$1
if [ -d "$target" ]; then
  abs=$(CDPATH= cd -- "$target" && pwd -P)
elif [ -e "$target" ]; then
  abs=$(CDPATH= cd -- "$(dirname -- "$target")" && pwd -P)/$(basename -- "$target")
else
  printf '%s: no such file or directory: %s\n' {name} "$target" >&2
  exit 1
fi
"#
    );
    let (bare, open) = match launch {
        Launch::Link { scheme } => {
            let base = sh_quote(&format!("{scheme}://open"));
            (
                format!("  exec open {base}\n"),
                format!(
                    "enc=$(printf '%s' \"$abs\" | od -An -v -tx1 | tr -d ' \\n' | sed 's/../%&/g')\n\
                     exec open {base}\"?path=$enc\"\n"
                ),
            )
        }
        Launch::Exec { exe } => {
            let exe = sh_quote(&exe.to_string_lossy());
            (
                format!("  nohup {exe} >/dev/null 2>&1 &\n  exit 0\n"),
                format!("nohup {exe} \"$abs\" >/dev/null 2>&1 &\n"),
            )
        }
    };
    format!(
        "#!/bin/sh\n\
         {MARKER}\n\
         # Installed by Skein (Settings → Command line), and removed from there.\n\
         # {command} [path] opens a folder in Skein; with no path it just brings Skein forward.\n\
         set -e\n\
         if [ \"$#\" -eq 0 ]; then\n\
         {bare}\
         fi\n\
         {resolve}\
         {open}"
    )
}

/// What Settings shows.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliShimStatus {
    /// What to type: `skein`, `skein-local` or `skein-dev`.
    command: String,
    /// Where the script is, or would be, installed.
    path: String,
    /// `installed`, `absent`, or `foreign` — a file Skein didn't write
    /// is in the way, and stays there.
    state: &'static str,
    /// The installed script differs from what this build would write —
    /// the app moved, or an older Skein wrote it. Install rewrites it.
    stale: bool,
    /// Whether the folder is on the login shell's `PATH`. `None` when
    /// that is unknown (the probe hasn't answered, or is off).
    on_path: Option<bool>,
    /// Why this platform can't install the command, when it can't.
    unsupported: Option<String>,
}

/// Everything status/install/uninstall need, resolved from the app.
struct Plan {
    command: String,
    dir: PathBuf,
    file: PathBuf,
    script: String,
}

fn plan(app: &AppHandle) -> Result<Plan, String> {
    let command = crate::open_request::configured_schemes(app.config())
        .into_iter()
        .next()
        .unwrap_or_else(|| "skein".to_owned());
    let home = app.path().home_dir().map_err(|e| e.to_string())?;
    let dir = home.join(".local").join("bin");
    let file = dir.join(&command);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    let launch = launch_for(&exe, appimage, &command, cfg!(target_os = "macos"));
    let script = shim_script(&command, &launch);
    Ok(Plan {
        command,
        dir,
        file,
        script,
    })
}

/// Whether `dir` is one of `path`'s entries (`:`-separated; this
/// installer only runs on unix).
fn path_contains(path: &str, dir: &Path) -> bool {
    path.split(':')
        .any(|entry| !entry.is_empty() && Path::new(entry) == dir)
}

fn status_of(plan: &Plan) -> CliShimStatus {
    let mut status = CliShimStatus {
        command: plan.command.clone(),
        path: plan.file.to_string_lossy().into_owned(),
        state: "absent",
        stale: false,
        on_path: crate::pty::login_shell_path().map(|p| path_contains(&p, &plan.dir)),
        unsupported: None,
    };
    if cfg!(windows) {
        status.unsupported = Some("The command isn't available on Windows yet.".to_owned());
        return status;
    }
    if let Ok(existing) = std::fs::read_to_string(&plan.file) {
        if existing.lines().nth(1) == Some(MARKER) {
            status.state = "installed";
            status.stale = existing != plan.script;
        } else {
            status.state = "foreign";
        }
    } else if plan.file.exists() {
        // Unreadable, or not text: certainly not ours.
        status.state = "foreign";
    }
    status
}

#[tauri::command]
pub async fn cli_shim_status(app: AppHandle) -> Result<CliShimStatus, String> {
    Ok(status_of(&plan(&app)?))
}

/// Write (or rewrite) the script. Refuses on Windows and over a file
/// Skein didn't write.
#[tauri::command]
pub async fn cli_shim_install(app: AppHandle) -> Result<CliShimStatus, String> {
    let plan = plan(&app)?;
    let before = status_of(&plan);
    if let Some(why) = before.unsupported {
        return Err(why);
    }
    if before.state == "foreign" {
        return Err(format!(
            "{} already exists and wasn't installed by Skein, so it was left alone.",
            plan.file.display()
        ));
    }
    std::fs::create_dir_all(&plan.dir).map_err(|e| format!("{}: {e}", plan.dir.display()))?;
    std::fs::write(&plan.file, &plan.script)
        .map_err(|e| format!("{}: {e}", plan.file.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&plan.file, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", plan.file.display()))?;
    }
    tracing::info!(path = %plan.file.display(), "cli shim installed");
    Ok(status_of(&plan))
}

/// Remove the script — only ever one Skein wrote.
#[tauri::command]
pub async fn cli_shim_uninstall(app: AppHandle) -> Result<CliShimStatus, String> {
    let plan = plan(&app)?;
    if status_of(&plan).state == "installed" {
        std::fs::remove_file(&plan.file).map_err(|e| format!("{}: {e}", plan.file.display()))?;
        tracing::info!(path = %plan.file.display(), "cli shim removed");
    }
    Ok(status_of(&plan))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use tauri::Url;

    use super::*;

    #[test]
    fn a_bundled_mac_app_links_and_everything_else_execs() {
        let bundled = Path::new("/Applications/Skein.app/Contents/MacOS/skein-app");
        assert_eq!(
            launch_for(bundled, None, "skein", true),
            Launch::Link {
                scheme: "skein".to_owned()
            }
        );
        // A dev build isn't registered with LaunchServices, so a link
        // would go nowhere.
        let dev = Path::new("/src/skein/target/debug/skein-app");
        assert_eq!(
            launch_for(dev, None, "skein-dev", true),
            Launch::Exec {
                exe: dev.to_path_buf()
            }
        );
        let appimage = PathBuf::from("/home/me/Skein.AppImage");
        assert_eq!(
            launch_for(
                Path::new("/tmp/.mount_x/usr/bin/skein-app"),
                Some(appimage.clone()),
                "skein",
                false
            ),
            Launch::Exec { exe: appimage }
        );
    }

    #[test]
    fn the_marker_is_the_second_line_status_checks() {
        let script = shim_script(
            "skein",
            &Launch::Link {
                scheme: "skein".to_owned(),
            },
        );
        assert_eq!(script.lines().nth(1), Some(MARKER));
    }

    #[test]
    fn exec_paths_are_single_quoted_for_sh() {
        assert_eq!(sh_quote("/a b/it's"), r"'/a b/it'\''s'");
        let script = shim_script(
            "skein-dev",
            &Launch::Exec {
                exe: PathBuf::from("/a b/skein-app"),
            },
        );
        assert!(script.contains("nohup '/a b/skein-app' \"$abs\""));
    }

    #[test]
    fn path_membership_is_by_whole_entry() {
        let dir = Path::new("/home/me/.local/bin");
        assert!(path_contains("/usr/bin:/home/me/.local/bin", dir));
        assert!(!path_contains("/usr/bin:/home/me/.local/bin2", dir));
        assert!(!path_contains("", dir));
    }

    // Runs the generated script for real, with `open` swapped for a
    // stub that records its argument — proof that the link decodes back
    // to exactly the path, whatever bytes it contains.
    #[cfg(unix)]
    #[test]
    fn the_link_script_round_trips_an_awkward_path() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("my repo & co #1 + ünïcode");
        std::fs::create_dir(&target).unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let out = tmp.path().join("opened");
        let stub = bin.join("open");
        std::fs::write(
            &stub,
            format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", out.display()),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let shim = tmp.path().join("skein");
        std::fs::write(
            &shim,
            shim_script(
                "skein",
                &Launch::Link {
                    scheme: "skein".to_owned(),
                },
            ),
        )
        .unwrap();

        let path_var = format!("{}:/usr/bin:/bin", bin.display());
        let status = std::process::Command::new("/bin/sh")
            .arg(&shim)
            .arg(&target)
            .env("PATH", path_var)
            .status()
            .unwrap();
        assert!(status.success());

        let url = Url::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
        let want = dunce::canonicalize(&target).unwrap();
        assert_eq!(
            crate::open_request::link_action(&url, &["skein".to_owned()]),
            Some(crate::open_request::LinkAction::Open(want))
        );
    }

    // The exec variant, run for real against a stub binary: a relative
    // path reaches Skein absolute and physical, and a bare call passes
    // no path at all (plain `skein` only brings Skein forward).
    #[cfg(unix)]
    #[test]
    fn the_exec_script_hands_the_binary_an_absolute_path() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("a repo");
        std::fs::create_dir_all(target.join("src")).unwrap();
        let out = tmp.path().join("argv");
        let exe = tmp.path().join("fake skein-app");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\nprintf '%s|' \"$@\" > '{}.tmp'\nmv '{0}.tmp' '{0}'\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let shim = tmp.path().join("skein-dev");
        std::fs::write(&shim, shim_script("skein-dev", &Launch::Exec { exe })).unwrap();

        // The script backgrounds the binary, so wait for its record.
        let run = |args: &[&str], cwd: &Path| {
            let _ = std::fs::remove_file(&out);
            let status = std::process::Command::new("/bin/sh")
                .arg(&shim)
                .args(args)
                .current_dir(cwd)
                .status()
                .unwrap();
            assert!(status.success());
            for _ in 0..100 {
                if let Ok(recorded) = std::fs::read_to_string(&out) {
                    return recorded;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("the stub binary never ran");
        };

        let want = dunce::canonicalize(target.join("src")).unwrap();
        assert_eq!(run(&["src"], &target), format!("{}|", want.display()));
        // `printf '%s|'` with no arguments still prints one empty field.
        assert_eq!(run(&[], &target), "|");
    }
}
