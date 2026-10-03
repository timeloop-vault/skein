fn main() {
    // tauri-plugin-notifications (Choochmeque fork) compiles a Swift
    // package and statically links it when `notify-rust` is disabled.
    // The resulting binary depends on `libswift_Concurrency.dylib`
    // (and friends), which live in `/usr/lib/swift/` on macOS 11+ but
    // are NOT findable without an rpath hint. Without this the bundle
    // crashes at launch with `Library not loaded: @rpath/libswift_Concurrency.dylib`
    // / "no LC_RPATH's found".
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }

    // Windows: give *test* binaries the Common-Controls v6 manifest.
    //
    // `tauri-plugin-dialog` → `rfd` imports `TaskDialogIndirect`, which
    // only the v6 side-by-side comctl32 exports — `C:\Windows\System32\
    // comctl32.dll` is still v5.82 and does not have it. A binary binds
    // to v6 only by declaring the dependency in its manifest, and
    // `tauri_build::build()` embeds that manifest into the *app* binary
    // only. Cargo's test harness is a separate link target, so it got
    // v5 and died at load with STATUS_ENTRYPOINT_NOT_FOUND
    // (`0xc0000139`) before running a single test.
    //
    // `rustc-link-arg-tests` would be the tighter scope, but it only
    // applies to `[[test]]` targets and these are `#[cfg(test)]` units
    // inside the lib. The unscoped form also hits the app binary, which
    // is harmless — it already declares the same dependency through
    // tauri's embedded manifest, and a duplicate `/MANIFESTDEPENDENCY`
    // of identical content is merged rather than rejected (verified:
    // the app still links).
    #[cfg(target_os = "windows")]
    {
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' \
             name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
             processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }

    emit_build_info();

    tauri_build::build();
}

/// Run git from the manifest dir with every repo-redirecting variable
/// removed: the pre-commit hook exports `GIT_DIR` / `GIT_INDEX_FILE`, and a
/// bare git would then act on whatever repository the hook is running for.
fn git(args: &[&str]) -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        cmd.current_dir(dir);
    }
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_PREFIX",
    ] {
        cmd.env_remove(var);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    Some(text)
}

fn non_empty_env(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Emit `SKEIN_VERSION` / `SKEIN_COMMIT` for `src/build_info.rs`. Never
/// fails the build: any git problem degrades to `0.0.0-dev`, no commit.
fn emit_build_info() {
    let override_version = non_empty_env("SKEIN_BUILD_VERSION");
    let override_commit = non_empty_env("SKEIN_BUILD_COMMIT");

    let sha = git(&["rev-parse", "--short=7", "HEAD"]).filter(|s| !s.is_empty());
    let commit = override_commit.or_else(|| sha.clone()).unwrap_or_default();

    let version = override_version.unwrap_or_else(|| {
        if sha.is_none() {
            return "0.0.0-dev".to_string();
        }
        let tag = git(&["describe", "--tags", "--abbrev=0", "--match", "v[0-9]*"])
            .map(|t| t.trim_start_matches('v').to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "0.0.0".to_string());
        match &sha {
            Some(sha) => format!("{tag}-dev+{sha}"),
            None => format!("{tag}-dev"),
        }
    });

    println!("cargo:rustc-env=SKEIN_VERSION={version}");
    println!("cargo:rustc-env=SKEIN_COMMIT={commit}");

    // Rerun when HEAD moves: HEAD itself, the branch ref it points at,
    // packed refs and the tags dir. `--git-path` may print a path relative
    // to the cwd (the manifest dir), so absolutize.
    let base = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let mut watch = vec!["HEAD".to_string()];
    if let Some(sym) = git(&["symbolic-ref", "-q", "HEAD"]).filter(|s| !s.is_empty()) {
        watch.push(sym);
    }
    watch.push("packed-refs".to_string());
    watch.push("refs/tags".to_string());
    for name in watch {
        if let Some(p) = git(&["rev-parse", "--git-path", &name]).filter(|p| !p.is_empty()) {
            let path = std::path::Path::new(&base).join(p);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}
