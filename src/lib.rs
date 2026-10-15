//! Whole-disk file search for Windows: a fuzzy name index over every entry
//! on every fixed drive, kept live from change notifications, plus a
//! trigram content index for text files. `Engine` runs it all in-process;
//! the `qasd` binary wraps one in a daemon with a JSON-lines named pipe.
//!
//! A Windows port of fsearch (github.com/noahdunnagan/fsearch, MIT).

pub mod alloc;
pub mod content;
mod engine;
mod fsevents;
pub mod index;
pub mod live;
pub mod mft;
pub mod pipe;
pub mod query;
pub mod sys;
pub mod usn;
pub mod walk;

pub use content::{Case, FileMatches, Grep, GrepResult, Hit};
pub use engine::{Engine, Found, Options, Status, default_dir, gated, has_full_disk_access, no_materialize};
pub use query::{GrepMode, Query};
