use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use floem::{
    Application, event::EventListener, ext_event::create_signal_from_channel, prelude::*,
    reactive::create_effect, views::scroll, window::WindowConfig,
};
use parking_lot::RwLock;
use skein_git::{Repo, StatusEntry, StatusKind};

use crate::pty::{PtyEvent, PtyManager};
use crate::term::RawTerm;
use crate::term_view::terminal_view;

mod pty;
mod term;
mod term_colors;
mod term_view;

const REPO_PATH: &str = ".";
const TERM_ID: &str = "term-1";
const INITIAL_ROWS: u16 = 24;
const INITIAL_COLS: u16 = 80;

fn refresh_status() -> Vec<StatusEntry> {
    Repo::open(Path::new(REPO_PATH))
        .and_then(|r| r.status())
        .unwrap_or_default()
}

fn status_kind_glyph(k: StatusKind) -> &'static str {
    match k {
        StatusKind::Added => "A",
        StatusKind::Modified => "M",
        StatusKind::Deleted => "D",
        StatusKind::Renamed => "R",
        StatusKind::Untracked => "?",
        StatusKind::Conflicted => "!",
        StatusKind::Typechange => "T",
    }
}

fn git_pane() -> impl IntoView {
    let entries = RwSignal::new(refresh_status());

    v_stack((
        h_stack((
            label(|| "git status").style(|s| s.font_size(16.0)),
            button("Refresh").action(move || entries.set(refresh_status())),
        ))
        .style(|s| s.padding(12).gap(12).items_center()),
        scroll(
            dyn_stack(
                move || entries.get(),
                |e: &StatusEntry| (e.path.clone(), e.staged, status_kind_glyph(e.kind)),
                |e: StatusEntry| {
                    let glyph = status_kind_glyph(e.kind);
                    let staged = if e.staged { "S" } else { " " };
                    let path = e.path;
                    label(move || format!("{glyph} {staged}  {path}"))
                        .style(|s| s.padding_horiz(12).padding_vert(2))
                },
            )
            .style(floem::style::Style::flex_col),
        )
        .style(|s| s.flex_grow(1.0)),
    ))
    .style(|s| s.flex_grow(1.0).height_full())
}

fn shell_command() -> Vec<String> {
    if cfg!(target_os = "windows") {
        vec!["cmd.exe".into()]
    } else {
        vec!["bash".into(), "-li".into()]
    }
}

fn term_pane() -> impl IntoView {
    let pty_manager = Arc::new(PtyManager::new());

    // Channel: alacritty_terminal -> PTY (for cursor reports etc.)
    let (alac_pty_tx, alac_pty_rx) = crossbeam_channel::unbounded::<Vec<u8>>();
    // Channel: PTY reader thread -> UI (output bytes + exit signal).
    let (pty_data_tx, pty_data_rx) = crossbeam_channel::unbounded::<PtyEvent>();

    let raw = Arc::new(RwLock::new(RawTerm::new(
        INITIAL_ROWS,
        INITIAL_COLS,
        alac_pty_tx,
    )));

    let cmd = shell_command();
    if let Err(err) = pty_manager.spawn(
        TERM_ID.into(),
        &cmd,
        Path::new(REPO_PATH),
        INITIAL_ROWS,
        INITIAL_COLS,
        move |ev| {
            let _ = pty_data_tx.send(ev);
        },
    ) {
        eprintln!("pty spawn failed: {err}");
    }

    // Drain alacritty's PTY-write requests on a worker thread —
    // cursor position reports, mouse reports, OSC responses.
    {
        let pty = Arc::clone(&pty_manager);
        std::thread::spawn(move || {
            while let Ok(bytes) = alac_pty_rx.recv() {
                let _ = pty.write(TERM_ID, &bytes);
            }
        });
    }

    // Build the terminal view. Resize and keyboard-input callbacks
    // route through PtyManager.
    let pty_for_resize = Arc::clone(&pty_manager);
    let pty_for_input = Arc::clone(&pty_manager);
    let view = terminal_view(
        Arc::clone(&raw),
        move |rows, cols| {
            let _ = pty_for_resize.resize(TERM_ID, rows, cols);
        },
        move |bytes| {
            let _ = pty_for_input.write(TERM_ID, bytes);
        },
    );
    let view_id = view.view_id();
    let focus_flag = view.focus_flag();
    let focus_flag_lost = Arc::clone(&focus_flag);

    // Bridge PTY data into the alacritty parser, then nudge the view
    // to repaint. update_state is the reactive-friendly trigger;
    // passing `()` is fine because the view ignores the payload.
    let pty_signal = create_signal_from_channel(pty_data_rx);
    let raw_for_effect = Arc::clone(&raw);
    create_effect(move |_| {
        if let Some(PtyEvent::Data { chunk }) = pty_signal.get() {
            raw_for_effect.write().advance(chunk.as_bytes());
            view_id.update_state(());
        }
    });

    v_stack((
        label(|| "terminal").style(|s| s.font_size(16.0).padding(12)),
        view.keyboard_navigable()
            .on_event_cont(EventListener::FocusGained, move |_| {
                focus_flag.store(true, Ordering::Relaxed);
                view_id.request_paint();
            })
            .on_event_cont(EventListener::FocusLost, move |_| {
                focus_flag_lost.store(false, Ordering::Relaxed);
                view_id.request_paint();
            })
            .style(|s| s.flex_grow(1.0).width_full()),
    ))
    .style(|s| s.flex_grow(1.0).height_full())
}

fn app_view() -> impl IntoView {
    h_stack((git_pane(), term_pane())).style(|s| s.width_full().height_full())
}

fn main() {
    Application::new()
        .window(
            |_| app_view(),
            Some(WindowConfig::default().title("Skein (proto)")),
        )
        .run();
}
