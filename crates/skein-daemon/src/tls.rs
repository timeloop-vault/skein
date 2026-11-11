//! The process-wide rustls crypto provider.

/// Install rustls's default crypto provider so reqwest doesn't panic
/// with "No provider set" on the first `Client::builder().build()`.
/// Reqwest 0.13 + rustls 0.23 require an explicit `install_default`
/// call before any TLS context is constructed — and reqwest constructs
/// one eagerly even when we only ever use plain HTTP (the L2c-2
/// opencode adapter talks to 127.0.0.1). Idempotent: `install_default`
/// returns Err on the second call, which we ignore.
pub fn install_rustls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
