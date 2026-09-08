//! The agent-facing review API (#213, epic #52 D8).
//!
//! #211 gave the diff a lifetime and #212 gave the user somewhere to
//! write comments; the loop still closed by copy-paste. This module is
//! the half that closes it properly: a localhost HTTP server inside the
//! running Skein process, exposed as an MCP endpoint, so an agent in a
//! room can read the comments on its own work and answer them.
//!
//! # Why HTTP, inside the app
//!
//! Skein is a long-running desktop app. A stdio MCP server would be a
//! fresh process per harness with no route back to the instance holding
//! the database, so D8 chose localhost HTTP — and both harnesses speak
//! MCP over HTTP with custom headers (Claude Code `type: "http"`,
//! opencode `type: "remote"`), which is why there is no separate bridge
//! binary here. One endpoint, both clients, nothing extra to ship.
//!
//! # The token is the scope
//!
//! No request carries a room id. Authentication resolves a bearer token
//! to exactly one room, and every verb is scoped to that room — so a
//! harness cannot read, answer or address another room's review even by
//! guessing an id. `X-Skein-Harness` adds *attribution* on top of that,
//! and is never trusted for authorisation.
//!
//! # Resolve is not here, and that is the point
//!
//! The agent gets read, [`verbs::reply`] and [`verbs::mark_addressed`].
//! It cannot resolve a thread: an agent that can close its own comments
//! removes the gate the review loop exists to provide, and the user
//! would find "resolved" threads they never read. `tools/list` does not
//! offer it and `tools/call` refuses it by name.
//!
//! # Where things are
//!
//! | module | question it answers |
//! | :-- | :-- |
//! | [`state`] | what does a handler have access to |
//! | [`auth`] | who is calling, and may they |
//! | [`verbs`] | what can they actually do |
//! | [`mcp`] | how does that look as MCP |
//! | [`http`] | the routes, and nothing else |
//! | [`commands`] | the Tauri boundary, for the settings readout |

pub mod auth;
pub mod commands;
pub mod http;
pub mod mcp;
pub mod state;
pub mod verbs;

#[cfg(test)]
mod tests;

pub use state::AgentApiState;
