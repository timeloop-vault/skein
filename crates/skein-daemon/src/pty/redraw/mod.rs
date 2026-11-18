//! The redraw encoder (#807, slice T-2 of `docs/skeind-terminal-design.md`,
//! "The frame").
//!
//! A pure function from the engine's grid and one viewer's last-sent state
//! to a byte string in a fixed VT subset: a full redraw for a fresh viewer
//! (the attach snapshot), otherwise a diff. No I/O, no clock, no threads;
//! nothing calls it yet but [`TerminalEngine::frame`](super::engine::TerminalEngine::frame).
//!
//! - `source`: [`GridSource`], what the encoder reads
//! - `state`: [`ViewerState`], what a viewer has been sent
//! - `encode`: [`encode_frame`], frame assembly and the row loop
//! - `history`: the history push and the history of a full redraw
//! - `rows`: writing one row (wide characters, wrapped rows)
//! - `sgr`: style and link encoding
//! - `modes`: the mode mirrors and the cursor's look
//!
//! Resizes: any change of rows or columns is a full redraw (see `encode`).

mod encode;
mod history;
mod modes;
mod rows;
mod sgr;
mod source;
mod state;

#[cfg(test)]
mod test_grid;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_alt;
#[cfg(test)]
mod tests_engine;
#[cfg(test)]
mod tests_history;
#[cfg(test)]
mod tests_modes;
#[cfg(test)]
mod tests_replay;
#[cfg(test)]
mod tests_resize;
#[cfg(test)]
mod tests_rows;

pub use self::encode::encode_frame;
pub use self::source::GridSource;
pub use self::state::ViewerState;
