//! Building and showing a `WinRT` toast for an unpackaged app (#155).

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastNotification, ToastNotificationManager,
};
use windows::core::{HSTRING, IInspectable, Interface, Ref};

use crate::error::Result;
use crate::launch::parse_launch;

/// Show a toast for `aumid` with `title`/`body`, and call
/// `on_banner_click(id)` if the user taps it while it's still visible
/// as a banner.
///
/// Built with the `XmlDocument` DOM API (`SetInnerText`, so `title`
/// and `body` are escaped for free) rather than string
/// concatenation. The root `<toast>` carries
/// `launch="skein-notify:<id>"` and `activationType="foreground"`, so
/// a tap delivers the same id string this crate's COM activator
/// would see for a later Action Center click — `parse_launch` is the
/// shared decoder for both.
///
/// # Errors
/// Returns an error if building the toast XML or showing it fails
/// (e.g. `aumid` was never registered via `register_app`); the caller
/// should log and continue — Skein is fully usable without OS toasts.
pub fn show_toast(
    aumid: &str,
    id: u32,
    title: &str,
    body: &str,
    on_banner_click: impl Fn(u32) + Send + 'static,
) -> Result<()> {
    let doc = XmlDocument::new()?;

    let toast_el = doc.CreateElement(&HSTRING::from("toast"))?;
    doc.AppendChild(&toast_el)?;
    toast_el.SetAttribute(
        &HSTRING::from("launch"),
        &HSTRING::from(format!("skein-notify:{id}")),
    )?;
    toast_el.SetAttribute(
        &HSTRING::from("activationType"),
        &HSTRING::from("foreground"),
    )?;

    let visual = doc.CreateElement(&HSTRING::from("visual"))?;
    let binding = doc.CreateElement(&HSTRING::from("binding"))?;
    binding.SetAttribute(&HSTRING::from("template"), &HSTRING::from("ToastGeneric"))?;

    let title_el = doc.CreateElement(&HSTRING::from("text"))?;
    title_el.SetInnerText(&HSTRING::from(title))?;
    binding.AppendChild(&title_el)?;

    let body_el = doc.CreateElement(&HSTRING::from("text"))?;
    body_el.SetInnerText(&HSTRING::from(body))?;
    binding.AppendChild(&body_el)?;

    visual.AppendChild(&binding)?;
    toast_el.AppendChild(&visual)?;

    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(aumid))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;

    toast.Activated(&TypedEventHandler::new(
        move |_sender: Ref<'_, ToastNotification>, args: Ref<'_, IInspectable>| {
            if let Some(inspectable) = &*args
                && let Ok(activated) = inspectable.cast::<ToastActivatedEventArgs>()
                && let Ok(arguments) = activated.Arguments()
                && let Some(clicked_id) = parse_launch(&arguments.to_string_lossy())
            {
                on_banner_click(clicked_id);
            }
            Ok(())
        },
    ))?;

    notifier.Show(&toast)?;
    Ok(())
}
