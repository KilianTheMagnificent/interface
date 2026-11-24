//! Opt-in remote access, pairing, input authority and bounded terminal replay.
mod access;
mod approval;
pub mod connection;
pub mod connection_crypto;
mod http;
pub mod relay;
pub mod relay_admission;
pub mod replay;
pub mod threads;
pub mod workspaces;

pub use access::{RemoteError, RemoteHub, ThreadEffect, ThreadReceipt, TrustPersistence};
pub use http::{
    ConnectionPersistence, HistoryFuture, InboxFuture, RemoteBackend, RemoteServer,
    ThreadAnswerFuture, ThreadDetailFuture, ThreadsFuture,
};
