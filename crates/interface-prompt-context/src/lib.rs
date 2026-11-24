//! Prompt-context authorization, validation, and pasted-image storage.
//!
//! This crate is the transport-neutral Rust owner for the four frozen 0.1.169
//! prompt-context operations. It deliberately keeps renderer presentation out
//! of the trust boundary: paths are re-authorized for every send, pasted bytes
//! are decoded before they are written, and thumbnails can only read an open
//! workspace or an exact path granted by a drop/paste.

mod image;
mod path;
mod service;
mod validation;

pub use image::{
    PromptImageThumbnailer, RustImageThumbnailer, SystemPromptContextClock,
    SystemPromptImageNameMinter,
};
pub use service::{
    PROMPT_CONTEXT_OPERATIONS, PastedImageInput, PromptContextClock, PromptContextReviewSource,
    PromptContextRuntimeView, PromptContextService, PromptContextServiceConfig,
    PromptContextServiceError, PromptContextThreads, PromptImageNameMinter,
    SystemPromptContextReviewSource, ThreadReference, ThreadReferenceRefusal,
};
