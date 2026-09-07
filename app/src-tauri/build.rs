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

    tauri_build::build();
}
