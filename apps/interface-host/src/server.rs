use std::{
    io,
    sync::{Arc, Mutex},
};

use futures::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    task::JoinSet,
};
use tokio_util::{
    bytes::BytesMut,
    codec::{FramedRead, FramedWrite, LinesCodec, LinesCodecError},
};

use crate::{
    dispatcher::HostDispatcher,
    protocol::{BridgeError, BridgeRequest, BridgeResponse, HostFrame, HostReady, PushFrame},
};

/// Eight frozen 16 MiB pasted-image inputs expand to just under 171 MiB when
/// base64 encoded. Keep one finite line bound on both sides of the private
/// Electron/Rust protocol while admitting every valid 0.1.169 paste.
pub const HOST_MAX_FRAME_BYTES: usize = 192 * 1024 * 1024;

/// Pushes already queued when one is being written leave in the same flush,
/// so a burst of provider deltas costs one write instead of one per delta.
/// The response barrier counts every push written, so a batch only brings a
/// held response closer; the bound keeps a flood from starving the loop.
const PUSH_BATCH_FRAMES: usize = 64;
/// A request line larger than this (a pasted image) releases the read
/// buffer it grew, instead of keeping that high-water mark for the life of
/// the process.
const READ_BUFFER_RELEASE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct HostServerConfig {
    pub max_frame_bytes: usize,
    pub max_concurrent_requests: usize,
    pub outbound_capacity: usize,
    /// The ready frame's `hostVersion`. The binary sets its release-stamped
    /// version; the library never reads the stamp, so a new release version
    /// does not recompile it (or iface, which links it).
    pub host_version: &'static str,
}

impl Default for HostServerConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: HOST_MAX_FRAME_BYTES,
            max_concurrent_requests: 32,
            outbound_capacity: 256,
            host_version: env!("CARGO_PKG_VERSION"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HostServerError {
    #[error("host input failed: {0}")]
    Input(#[from] LinesCodecError),
    #[error("host output failed: {0}")]
    Output(io::Error),
    #[error("host frame serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("host outbound channel closed")]
    OutboundClosed,
    #[error("host push receiver lock was poisoned")]
    PushReceiverPoisoned,
    #[error("host request task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

pub struct HostServer<D> {
    dispatcher: Arc<D>,
    config: HostServerConfig,
    pushes: Mutex<Option<mpsc::Receiver<PushFrame>>>,
}

impl<D> HostServer<D>
where
    D: HostDispatcher,
{
    #[cfg(test)]
    #[must_use]
    pub fn new(dispatcher: Arc<D>, config: HostServerConfig) -> Self {
        Self {
            dispatcher,
            config,
            pushes: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn with_pushes(
        dispatcher: Arc<D>,
        pushes: mpsc::Receiver<PushFrame>,
        config: HostServerConfig,
    ) -> Self {
        Self {
            dispatcher,
            config,
            pushes: Mutex::new(Some(pushes)),
        }
    }

    /// Runs the bridge until input reaches EOF and every admitted request has
    /// produced a response.
    ///
    /// # Errors
    ///
    /// Returns framing, serialization, output, or request-task failures. A
    /// malformed individual request is converted into a protocol response and
    /// does not terminate the host.
    pub async fn run<R, W>(&self, reader: R, writer: W) -> Result<(), HostServerError>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut inbound = FramedRead::new(
            reader,
            LinesCodec::new_with_max_length(self.config.max_frame_bytes),
        );
        let mut outbound = FramedWrite::new(
            writer,
            LinesCodec::new_with_max_length(self.config.max_frame_bytes),
        );
        outbound
            .send(serde_json::to_string(&HostFrame::Ready(
                HostReady::current(self.config.host_version),
            ))?)
            .await
            .map_err(codec_io)?;

        let mut pushes = self
            .pushes
            .lock()
            .map_err(|_| HostServerError::PushReceiverPoisoned)?
            .take();
        let mut requests = JoinSet::new();
        let mut input_open = true;
        let request_capacity = self.config.max_concurrent_requests.max(1);
        let mut pending_response: Option<(BridgeResponse, usize)> = None;

        loop {
            // A dispatcher may enqueue one or more mutation pushes immediately
            // before returning its response. Once a response completes, freeze
            // that already-queued prefix as its ordering barrier. Pushes that
            // arrive later cannot extend the barrier indefinitely.
            if pending_response.is_none() {
                if let Some(joined) = requests.try_join_next() {
                    let queued_pushes = pushes.as_ref().map_or(0, mpsc::Receiver::len);
                    pending_response = Some((joined?, queued_pushes));
                }
            }
            if let Some((response, _)) =
                pending_response.take_if(|(_, queued_pushes)| *queued_pushes == 0)
            {
                write_frame(
                    &mut outbound,
                    HostFrame::Response(response),
                    self.config.max_frame_bytes,
                )
                .await?;
                continue;
            }

            let push_pending = pushes.as_ref().is_some_and(|receiver| !receiver.is_empty());
            if !input_open && requests.is_empty() && pending_response.is_none() && !push_pending {
                break;
            }
            tokio::select! {
                // Admission comes before pushes while there is request capacity.
                // Otherwise a permanently readable push lane can prevent a new
                // request from ever reaching its handler. Completed responses
                // still honor the frozen push prefix above.
                biased;
                incoming = inbound.next(), if input_open && requests.len() < request_capacity => {
                    match incoming {
                        Some(Ok(line)) => {
                            if line.len() > READ_BUFFER_RELEASE_BYTES {
                                release_read_buffer(inbound.read_buffer_mut());
                            }
                            let dispatcher = Arc::clone(&self.dispatcher);
                            requests.spawn(async move {
                                handle_line(dispatcher.as_ref(), line).await
                            });
                        }
                        Some(Err(error)) => return Err(HostServerError::Input(error)),
                        None => input_open = false,
                    }
                }
                push_result = recv_push(&mut pushes), if pushes.is_some() => {
                    if let Some(frame) = push_result {
                        let written = write_push_batch(
                            &mut outbound,
                            frame,
                            pushes.as_mut(),
                            self.config.max_frame_bytes,
                        ).await?;
                        if let Some((_, queued_pushes)) = &mut pending_response {
                            *queued_pushes = queued_pushes.saturating_sub(written);
                        }
                    } else {
                        pushes = None;
                        if let Some((_, queued_pushes)) = &mut pending_response {
                            *queued_pushes = 0;
                        }
                    }
                }
                joined = requests.join_next(), if pending_response.is_none() && !requests.is_empty() => {
                    if let Some(joined) = joined {
                        let queued_pushes = pushes.as_ref().map_or(0, mpsc::Receiver::len);
                        pending_response = Some((joined?, queued_pushes));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Owns the line so it is dropped once parsed: a pasted-image prompt is up to
/// ~170 MiB of text that would otherwise stay alive beside its decoded copy
/// for the whole provider send.
async fn handle_line<D: HostDispatcher>(dispatcher: &D, line: String) -> BridgeResponse {
    let parsed = serde_json::from_str::<BridgeRequest>(&line);
    drop(line);
    let Ok(request) = parsed else {
        return BridgeResponse::failure(
            "invalid-request",
            BridgeError::new(
                "invalid-json",
                "The host request was not a valid protocol envelope.",
                false,
                true,
            ),
        );
    };
    if let Err(error) = request.validate() {
        return BridgeResponse::failure(request.id, error);
    }
    let result = dispatcher
        .dispatch(&request.operation, request.payload)
        .await;
    match result {
        Ok(result) => BridgeResponse::success(request.id, result),
        Err(error) => BridgeResponse::failure(request.id, error.into()),
    }
}

async fn recv_push(receiver: &mut Option<mpsc::Receiver<PushFrame>>) -> Option<PushFrame> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// Writes `first` and every push already queued behind it, up to
/// [`PUSH_BATCH_FRAMES`], in one flush. Returns how many pushes left.
async fn write_push_batch<W>(
    outbound: &mut FramedWrite<W, LinesCodec>,
    first: PushFrame,
    mut pushes: Option<&mut mpsc::Receiver<PushFrame>>,
    max_frame_bytes: usize,
) -> Result<usize, HostServerError>
where
    W: AsyncWrite + Unpin,
{
    let mut written = 1;
    feed_frame(outbound, HostFrame::Push(first), max_frame_bytes).await?;
    while written < PUSH_BATCH_FRAMES {
        let Some(frame) = pushes
            .as_mut()
            .and_then(|receiver| receiver.try_recv().ok())
        else {
            break;
        };
        feed_frame(outbound, HostFrame::Push(frame), max_frame_bytes).await?;
        written += 1;
    }
    SinkExt::<String>::flush(outbound).await.map_err(codec_io)?;
    Ok(written)
}

async fn write_frame<W>(
    outbound: &mut FramedWrite<W, LinesCodec>,
    frame: HostFrame,
    max_frame_bytes: usize,
) -> Result<(), HostServerError>
where
    W: AsyncWrite + Unpin,
{
    feed_frame(outbound, frame, max_frame_bytes).await?;
    SinkExt::<String>::flush(outbound).await.map_err(codec_io)
}

/// Encodes one frame into the writer's buffer without flushing it.
async fn feed_frame<W>(
    outbound: &mut FramedWrite<W, LinesCodec>,
    frame: HostFrame,
    max_frame_bytes: usize,
) -> Result<(), HostServerError>
where
    W: AsyncWrite + Unpin,
{
    let mut encoded = serde_json::to_string(&frame)?;
    if encoded.len() > max_frame_bytes {
        let HostFrame::Response(response) = frame else {
            return Err(HostServerError::Output(io::Error::new(
                io::ErrorKind::InvalidData,
                "serialized host frame exceeded the configured bound",
            )));
        };
        encoded = serde_json::to_string(&HostFrame::Response(BridgeResponse::failure(
            response.id,
            BridgeError::new(
                "response-too-large",
                "The Rust host result exceeded its bounded transport; the operation may already have taken effect.",
                false,
                false,
            ),
        )))?;
        if encoded.len() > max_frame_bytes {
            return Err(HostServerError::Output(io::Error::new(
                io::ErrorKind::InvalidData,
                "serialized host failure frame exceeded the configured bound",
            )));
        }
    }
    outbound.feed(encoded).await.map_err(codec_io)
}

fn codec_io(error: LinesCodecError) -> HostServerError {
    match error {
        LinesCodecError::Io(error) => HostServerError::Output(error),
        LinesCodecError::MaxLineLengthExceeded => HostServerError::Output(io::Error::new(
            io::ErrorKind::InvalidData,
            "serialized host frame exceeded the configured bound",
        )),
    }
}

/// Replaces an oversized read buffer with one holding only its unread bytes.
/// `FramedRead` never shrinks the buffer it grows for one huge line.
fn release_read_buffer(buffer: &mut BytesMut) {
    if buffer.capacity() > READ_BUFFER_RELEASE_BYTES {
        let pending = buffer.split();
        *buffer = BytesMut::from(&pending[..]);
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
