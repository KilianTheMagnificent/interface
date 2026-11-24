//! Deterministic orchestration state, bounded projections, and storage.

mod error;
mod projection;
mod reducer;
mod store;
mod transcript;

pub use error::{OrchestrationError, OrchestrationErrorCode, Result};
pub use projection::*;
pub use reducer::{agent_state_is_terminal, reduce_orchestration};
pub use store::*;
pub use transcript::{
    clean_orchestration_text, orchestration_line, render_orchestration_transcript,
};
