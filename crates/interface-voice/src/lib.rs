//! Pure parts of the Interface voice assistant: the helper wire protocol, the
//! sentence chunker that turns streamed assistant text into utterances, the
//! markdown-to-speech cleaner, and the ANSI-stripping terminal tail buffer.

pub mod chunker;
pub mod protocol;
pub mod speakable;
pub mod tail;

pub use chunker::SentenceChunker;
pub use protocol::{HelperToHost, HostToHelper, VoiceInfo};
pub use speakable::speakable;
pub use tail::TailBuffer;
