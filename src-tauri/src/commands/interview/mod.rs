//! Copilot (interview / meeting) commands. Command names are unchanged so
//! the frontend `invoke` calls stay stable; preset-specific behaviour lives
//! in `crate::copilot`.

pub mod documents;
pub mod excerpts;
pub mod ingest;
pub mod stream;
pub mod suggest;
