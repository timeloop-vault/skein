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
    tauri_build::build();
}
