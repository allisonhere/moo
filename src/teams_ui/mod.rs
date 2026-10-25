//! The Teams screen: a keyboard-first navigator, inspector and workspace over the local Teams API.
//!
//! State, layout and drawing are pure and independent of the terminal; `run` owns all I/O. The
//! terminal pane renderer, PTY parsing and detection are not touched, so standalone terminals and
//! detachable sessions behave exactly as before.

mod data;
mod layout;
mod render;
mod run;
mod state;
mod theme;

pub(crate) use run::run;

#[cfg(test)]
mod tests;
