//! The Tauri-free server side of Skein (epic #575). `app/src-tauri` embeds
//! it; this crate must never depend on `tauri`.

pub mod db;
pub mod design;
pub mod event_sink;
pub mod fs;
pub mod git;
pub use skein_proto::harness_kind;
pub mod ignore_filter;
pub mod paths;
pub mod perf_hist;
pub mod perf_spans;
pub mod resume;
pub mod review;
pub mod review_surface;
pub mod room_paths;
pub mod rooms;
pub mod spawn_env;
pub mod spawn_settings;
pub mod stream_sink;
pub mod tool_groups;
pub mod watcher;
pub mod worktree_key;
