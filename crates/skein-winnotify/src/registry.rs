//! HKCU registry writes that give an *unpackaged* Skein build its own
//! `AppUserModelId` and a `CustomActivator` CLSID (#155). Windows
//! silently drops a toast whose AUMID isn't registered at all, and an
//! unregistered app has no COM activator to catch a click made after
//! the banner itself is gone (Action Center, or a cold `-Embedding`
//! launch). Both writes are idempotent — safe to repeat on every
//! launch — so `register_app` never needs to know whether it ran
//! before.

use std::path::Path;

use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegSetValueExW,
};
use windows::core::{HRESULT, HSTRING, PCWSTR};

use crate::clsid::clsid_braced;
use crate::error::Result;

/// UTF-16LE bytes with a trailing NUL — the wire format
/// `RegSetValueExW` expects for `REG_SZ`. Built by hand, one `u16` at
/// a time, so nothing here touches a raw pointer: the crate's unsafe
/// stays confined to the FFI calls themselves.
fn reg_sz_bytes(value: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(value.len() * 2 + 2);
    for unit in value.encode_utf16().chain(std::iter::once(0u16)) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// `RegCreateKeyExW`/`RegSetValueExW`/`RegCloseKey` all return a raw
/// `WIN32_ERROR` rather than a `windows::core::Result` (they're plain
/// Win32 calls, not COM) — this is the same win32-to-HRESULT mapping
/// `windows::core::Error::from_win32` and `GetLastError` callers use
/// throughout the ecosystem.
fn win32_result(status: WIN32_ERROR) -> Result<()> {
    Ok(HRESULT::from_win32(status.0).ok()?)
}

/// RAII guard around an open `HKEY`: closes it on drop, on every exit
/// path — including a `set_string` failure between `create_key` and
/// what would otherwise be an explicit close further down the
/// function. Mirrors `foreground::ThreadInputLink`'s guard-over-raw-
/// resource shape.
struct OwnedKey(HKEY);

impl OwnedKey {
    fn handle(&self) -> HKEY {
        self.0
    }
}

impl Drop for OwnedKey {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from a successful `RegCreateKeyExW` in
        // `create_key` and is closed exactly once, here.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// Create (or open, if it already exists) `HKCU\<subkey>` for
/// writing.
fn create_key(subkey: &str) -> Result<OwnedKey> {
    let subkey = HSTRING::from(subkey);
    let mut hkey = HKEY::default();
    // SAFETY: `subkey` is a live `HSTRING` for the duration of this
    // call; `hkey` is a valid out-parameter the API initializes on
    // success. No class name or security attributes are passed, so
    // the key gets the default HKCU ACL.
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &subkey,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &raw mut hkey,
            None,
        )
    };
    win32_result(status)?;
    Ok(OwnedKey(hkey))
}

/// Write a named `REG_SZ` value, or the key's own default value when
/// `name` is `None` (Windows treats an empty `lpValueName` as "the
/// key's default value").
fn set_string(key: &OwnedKey, name: Option<&str>, value: &str) -> Result<()> {
    let wide_name = HSTRING::from(name.unwrap_or_default());
    let bytes = reg_sz_bytes(value);
    // SAFETY: `key.handle()` is an open, writable key owned by `key`
    // (still alive here — we only borrow it); `wide_name` and `bytes`
    // both outlive this call; `bytes` is a well-formed,
    // null-terminated UTF-16LE buffer matching `REG_SZ`.
    let status = unsafe { RegSetValueExW(key.handle(), &wide_name, None, REG_SZ, Some(&bytes)) };
    win32_result(status)
}

/// Idempotent HKCU registration Windows needs before a toast for
/// `aumid` shows anything at all for an *unpackaged* app, plus the
/// `CustomActivator` CLSID so a click reaches this process (or
/// relaunches it) even once the toast banner itself is gone:
///
/// - `HKCU\Software\Classes\AppUserModelId\<aumid>`: `DisplayName`,
///   `IconUri` (an absolute path — Windows accepts a plain filesystem
///   path here, unlike a toast's own `<image src>`), `CustomActivator`
///   (braced, uppercase CLSID).
/// - `HKCU\Software\Classes\CLSID\{clsid}\LocalServer32`: the quoted
///   exe path, so Action Center can *launch* Skein for a cold click.
///
/// # Errors
/// Returns an error if any registry write fails (e.g. no HKCU access
/// at all); the caller should log and continue — Skein remains fully
/// usable, just without a working notification click.
pub fn register_app(
    aumid: &str,
    display_name: &str,
    icon_path: &Path,
    clsid: u128,
    exe_path: &Path,
) -> Result<()> {
    let clsid_braced = clsid_braced(clsid);

    let app_key = create_key(&format!("Software\\Classes\\AppUserModelId\\{aumid}"))?;
    set_string(&app_key, Some("DisplayName"), display_name)?;
    set_string(&app_key, Some("IconUri"), &icon_path.display().to_string())?;
    set_string(&app_key, Some("CustomActivator"), &clsid_braced)?;

    let server_key = create_key(&format!(
        "Software\\Classes\\CLSID\\{clsid_braced}\\LocalServer32"
    ))?;
    set_string(&server_key, None, &format!("\"{}\"", exe_path.display()))?;

    // Both keys close here via `OwnedKey`'s `Drop` — on this success
    // path and on every early `?` return above.
    Ok(())
}
