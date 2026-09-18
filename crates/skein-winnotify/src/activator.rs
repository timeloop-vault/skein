//! The COM `INotificationActivationCallback` that lets a click reach
//! Skein from Action Center — or a cold `-Embedding` launch — after
//! the toast banner itself is gone (#155). `toast::show_toast`'s
//! in-process `Activated` handler only fires while the banner is
//! still on screen; this is the other half.
//!
//! Registered once per process by [`start_activator`]. `Activate`
//! below runs on whichever COM RPC worker thread services the
//! activation, never the thread that called `start_activator`.

// `#[implement]` (used for `Activator`/`ActivatorFactory` below)
// expands to `#[inline(always)]` accessors and `&T as *const T`
// casts, emitted as sibling items whose spans land in this file.
// There's no source-level expression to rewrite, and an `#[allow]` on
// the annotated struct doesn't reach the generated impls, so both
// lints are scoped out for the whole module. Neither is written by
// hand anywhere here.
#![allow(clippy::inline_always, clippy::ref_as_ptr)]

use std::sync::Arc;

use windows::Win32::Foundation::CLASS_E_NOAGGREGATION;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoInitializeEx, CoRegisterClassObject,
    IClassFactory, IClassFactory_Impl, REGCLS_MULTIPLEUSE,
};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl,
    NOTIFICATION_USER_INPUT_DATA,
};
use windows::core::{BOOL, GUID, Interface, PCWSTR, Ref, implement};

use crate::error::{Error, Result};

/// Boxed so [`Activator`]/[`ActivatorFactory`] stay non-generic —
/// `#[implement]` generates a COM vtable per concrete type, and a
/// generic one adds nothing here since every registration is for the
/// process's one main window anyway.
type ActivateCallback = Arc<dyn Fn(String) + Send + Sync>;

#[implement(INotificationActivationCallback)]
struct Activator {
    on_activate: ActivateCallback,
}

impl INotificationActivationCallback_Impl for Activator_Impl {
    fn Activate(
        &self,
        _app_user_model_id: &PCWSTR,
        invoked_args: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        _count: u32,
    ) -> windows::core::Result<()> {
        // A null `invoked_args` is legal (e.g. a plain shortcut
        // launch with no `launch=`/`arguments=` string attached) —
        // `PCWSTR::to_string` calls `wcslen` with no null guard of
        // its own, so treat null as an empty string rather than
        // calling it: the app still comes forward, it just has no id
        // to jump with.
        let args = if invoked_args.is_null() {
            String::new()
        } else {
            // SAFETY: `invoked_args` was just checked non-null above
            // and is a valid, non-owning `PCWSTR` for the duration of
            // this COM call (the vtable thunk borrows it straight
            // from the caller); `to_string` copies it out before
            // returning, so nothing here outlives the call.
            unsafe { invoked_args.to_string() }.unwrap_or_default()
        };
        (self.on_activate)(args);
        Ok(())
    }
}

/// Out-of-proc COM activation requires a class factory —
/// `CoRegisterClassObject` takes an `IUnknown` that must implement
/// `IClassFactory`, not the activator instance directly.
#[implement(IClassFactory)]
struct ActivatorFactory {
    on_activate: ActivateCallback,
}

impl IClassFactory_Impl for ActivatorFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, windows::core::IUnknown>,
        riid: *const GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let activator = Activator {
            on_activate: self.on_activate.clone(),
        };
        let interface: INotificationActivationCallback = activator.into();
        // SAFETY: `riid`/`object` come straight from COM's own call
        // to `CreateInstance` and are valid for its duration; `query`
        // either writes the requested interface pointer into
        // `*object` or leaves it null, matching
        // `IUnknown::QueryInterface`'s contract.
        unsafe { interface.query(riid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Keeps the COM class registration alive. Deliberately does not
/// revoke it on drop: the dedicated thread `start_activator` spawns
/// parks forever regardless (COM class registrations are normally
/// held for the process lifetime, and `CoRevokeClassObject` has
/// thread-affinity rules of its own that aren't worth navigating for
/// a registration nothing ever needs to tear down early). Keep this
/// handle in long-lived state (e.g. Tauri managed state) so it isn't
/// dropped before the process exits.
pub struct ActivatorHandle {
    _cookie: u32,
}

/// Register `clsid` as an `INotificationActivationCallback` COM class
/// for the process lifetime, on a dedicated thread that parks forever
/// once registration succeeds — the registration and its `MTA` COM
/// apartment must outlive this function, and COM services `Activate`
/// calls on its own RPC worker threads regardless of which thread
/// owns the registration.
///
/// `on_activate` receives the toast's raw `Arguments` string (see
/// [`crate::parse_launch`]) and runs on that COM RPC worker thread —
/// never the thread that called `start_activator` — so it must be
/// `Send + Sync`.
///
/// Blocks until the dedicated thread reports whether registration
/// succeeded, so a caller can log the outcome immediately; the actual
/// `CoInitializeEx` + `CoRegisterClassObject` pair is fast.
///
/// # Errors
/// Returns an error if COM initialization or `CoRegisterClassObject`
/// fails, or if the dedicated thread ends without reporting either
/// way. Not fatal for the caller — Skein is fully usable without a
/// working notification-center click.
pub fn start_activator(
    clsid: u128,
    on_activate: impl Fn(String) + Send + Sync + 'static,
) -> Result<ActivatorHandle> {
    let guid = GUID::from_u128(clsid);
    let callback: ActivateCallback = Arc::new(on_activate);
    let (tx, rx) = std::sync::mpsc::channel::<Result<u32>>();

    std::thread::spawn(move || {
        // SAFETY: this thread is dedicated to owning the COM
        // registration below and is never reused for anything else.
        // `COINIT_MULTITHREADED` matches an out-of-proc
        // `LocalServer32` activator, whose `Activate` calls can land
        // on any RPC worker thread; per MSDN this returns `S_OK` on
        // the first call on a process and `S_FALSE` on later ones —
        // both treated as success by `HRESULT::ok()`.
        let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if let Err(e) = init.ok() {
            let _ = tx.send(Err(e.into()));
            return;
        }

        let factory = ActivatorFactory {
            on_activate: callback,
        };
        let factory: IClassFactory = factory.into();
        // SAFETY: `guid` and `factory` are both valid for the
        // duration of this call; `CLSCTX_LOCAL_SERVER` +
        // `REGCLS_MULTIPLEUSE` match an out-of-proc activator that
        // services repeated activations for as long as this thread
        // (and its COM apartment) stays alive.
        let registered = unsafe {
            CoRegisterClassObject(
                &raw const guid,
                &factory,
                CLSCTX_LOCAL_SERVER,
                REGCLS_MULTIPLEUSE,
            )
        };
        match registered {
            Ok(cookie) => {
                let _ = tx.send(Ok(cookie));
            }
            Err(e) => {
                let _ = tx.send(Err(e.into()));
                return;
            }
        }

        // Keep this thread — and the MTA apartment/registration it
        // owns — alive for the process lifetime.
        loop {
            std::thread::park();
        }
    });

    let cookie = rx.recv().map_err(|_| Error::ActivatorThreadDied)??;
    Ok(ActivatorHandle { _cookie: cookie })
}
