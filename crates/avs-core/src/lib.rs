//! # avs-core
//!
//! Núcleo puro (sem I/O) do AI Video Studio: tempo exato, modelo de dados,
//! validação, comandos de edição e histórico. Não depende de FFmpeg nem de UI,
//! o que o torna testável de forma determinística.

pub mod command;
pub mod history;
pub mod model;
pub mod subtitles;
pub mod time;
pub mod validate;

pub use command::{apply, ApplyOutcome, ClipPatch, CommandError, EditCommand, TrackPatch};
pub use history::{EditSession, HistoryInfo};
pub use model::*;
pub use subtitles::{Cue, SubtitleFormat};
pub use time::{Rational, Time, TimeRange, FLICKS_PER_SECOND};
pub use validate::{validate_project, validate_timeline, ValidationIssue};

#[cfg(test)]
mod tests;
