//! Conversion engine for `retype`.
//!
//! Domain logic only: the intent grammar, the plan compiler, conversion
//! backends, the journal and undo store. No process, no socket, no watcher.
//! The long-running daemon and the command line client live beside this crate
//! and use it.
