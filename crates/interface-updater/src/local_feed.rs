use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use interface_storage::PrivateDirectory;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::{content_type, update_feed_file_name};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

const LOOPBACK: &str = "127.0.0.1";
const MAX_REQUEST_BYTES: usize = 8 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const ACCEPT_RETRY_BACKOFF: Duration = Duration::from_millis(50);
const MAX_CONSECUTIVE_ACCEPT_FAILURES: u32 = 10;

/// Sanitized failures for the owner-private loopback migration feed.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum LocalFeedError {
    #[error("the local update feed path is invalid or unsafe")]
    UnsafeRoot,
    #[error("the local update feed could not bind its loopback listener")]
    BindFailed,
    #[error("the local update feed stopped unexpectedly")]
    Stopped,
}

/// A deliberately tiny GET/HEAD-only server for the flat Electron migration
/// feed. It is never a general Application Support file server.
pub struct LocalUpdateFeed {
    address: SocketAddr,
    serving: Arc<AtomicBool>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl fmt::Debug for LocalUpdateFeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalUpdateFeed")
            .field("address", &self.address)
            .field("serving", &self.is_serving())
            .finish_non_exhaustive()
    }
}

impl LocalUpdateFeed {
    /// Opens or creates an exact mode-0700 feed root and binds an ephemeral
    /// IPv4 loopback port.
    ///
    /// # Errors
    ///
    /// Refuses non-absolute, linked, wrongly-owned, or wrongly-moded roots and
    /// reports listener startup failures without exposing private paths.
    pub async fn start(root: &Path) -> Result<Self, LocalFeedError> {
        if !root.is_absolute() {
            return Err(LocalFeedError::UnsafeRoot);
        }
        let directory = Arc::new(
            PrivateDirectory::open_or_create(root).map_err(|_| LocalFeedError::UnsafeRoot)?,
        );
        let listener = TcpListener::bind((LOOPBACK, 0))
            .await
            .map_err(|_| LocalFeedError::BindFailed)?;
        let address = listener
            .local_addr()
            .map_err(|_| LocalFeedError::BindFailed)?;
        if !address.ip().is_loopback() {
            return Err(LocalFeedError::BindFailed);
        }
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let serving = Arc::new(AtomicBool::new(true));
        let task = tokio::spawn(accept_loop(
            listener,
            directory,
            Arc::clone(&serving),
            shutdown_receiver,
        ));
        Ok(Self {
            address,
            serving,
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }

    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{LOOPBACK}:{}", self.address.port())
    }

    /// Reports whether the loopback listener is still accepting clients. A feed
    /// whose accept loop tripped off after persistent failures serves nothing,
    /// so callers must stop advertising it as an enabled update source.
    #[must_use]
    pub fn is_serving(&self) -> bool {
        self.serving.load(Ordering::SeqCst)
    }

    /// Trips the accept loop off the way a persistent accept failure does, so
    /// service policy can be exercised without an exhausted descriptor table.
    #[cfg(test)]
    pub(crate) fn stop_serving_for_test(&self) {
        self.serving.store(false, Ordering::SeqCst);
    }

    /// Stops accepting new clients and waits for the listener task to exit.
    ///
    /// # Errors
    ///
    /// Returns an error only when the listener task panicked or was cancelled.
    pub async fn close(mut self) -> Result<(), LocalFeedError> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.await.map_err(|_| LocalFeedError::Stopped)?;
        }
        Ok(())
    }
}

impl Drop for LocalUpdateFeed {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// The listener the feed accepts on. Exists so the failure policy of
/// [`accept_loop`] can be tested without exhausting the descriptor table.
trait FeedListener {
    fn accept(
        &self,
    ) -> impl std::future::Future<Output = io::Result<(TcpStream, SocketAddr)>> + Send;
}

impl FeedListener for TcpListener {
    fn accept(
        &self,
    ) -> impl std::future::Future<Output = io::Result<(TcpStream, SocketAddr)>> + Send {
        TcpListener::accept(self)
    }
}

async fn accept_loop<L: FeedListener + Send>(
    listener: L,
    root: Arc<PrivateDirectory>,
    serving: Arc<AtomicBool>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let mut failures: u32 = 0;
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else {
                    // A refused accept is usually recoverable (an aborted peer,
                    // momentary descriptor pressure), so back off and keep
                    // serving. Only a persistent failure trips the loop off, and
                    // then the feed must report unavailable rather than leave the
                    // update machine claiming an enabled source.
                    failures += 1;
                    if failures >= MAX_CONSECUTIVE_ACCEPT_FAILURES {
                        break;
                    }
                    tokio::time::sleep(ACCEPT_RETRY_BACKOFF).await;
                    continue;
                };
                failures = 0;
                if !peer.ip().is_loopback() {
                    continue;
                }
                let root = Arc::clone(&root);
                tokio::spawn(async move {
                    let _ = serve_connection(stream, &root).await;
                });
            }
        }
    }
    serving.store(false, Ordering::SeqCst);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestMethod {
    Get,
    Head,
}

#[derive(Debug)]
struct FeedRequest {
    method: RequestMethod,
    file_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestError {
    BadRequest,
    MethodNotAllowed,
    NotFound,
}

async fn serve_connection(mut stream: TcpStream, root: &PrivateDirectory) -> io::Result<()> {
    let request = match timeout(REQUEST_TIMEOUT, read_request(&mut stream)).await {
        Ok(Ok(request)) => request,
        Ok(Err(RequestError::MethodNotAllowed)) => {
            return send_error(&mut stream, 405, "Method Not Allowed", true).await;
        }
        Ok(Err(RequestError::NotFound)) => {
            return send_error(&mut stream, 404, "Not Found", false).await;
        }
        Ok(Err(RequestError::BadRequest)) | Err(_) => {
            return send_error(&mut stream, 400, "Bad Request", false).await;
        }
    };

    let Ok((file, length)) = open_artifact(root, &request.file_name) else {
        return send_error(&mut stream, 404, "Not Found", false).await;
    };
    let headers = format!(
        "HTTP/1.1 200 OK\r\nCache-Control: no-store\r\nContent-Length: {length}\r\nContent-Type: {}\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        content_type(&request.file_name)
    );
    stream.write_all(headers.as_bytes()).await?;
    if request.method == RequestMethod::Get {
        let mut file = tokio::fs::File::from_std(file);
        tokio::io::copy(&mut file, &mut stream).await?;
    }
    stream.shutdown().await
}

async fn read_request(stream: &mut TcpStream) -> Result<FeedRequest, RequestError> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|_| RequestError::BadRequest)?;
        if count == 0 {
            return Err(RequestError::BadRequest);
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(RequestError::BadRequest);
        }
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let source = std::str::from_utf8(&bytes).map_err(|_| RequestError::BadRequest)?;
    let request_line = source
        .split("\r\n")
        .next()
        .ok_or(RequestError::BadRequest)?;
    let mut parts = request_line.split(' ');
    let method = match parts.next() {
        Some("GET") => RequestMethod::Get,
        Some("HEAD") => RequestMethod::Head,
        Some(_) => return Err(RequestError::MethodNotAllowed),
        None => return Err(RequestError::BadRequest),
    };
    let target = parts.next().ok_or(RequestError::BadRequest)?;
    if !matches!(parts.next(), Some("HTTP/1.0" | "HTTP/1.1")) || parts.next().is_some() {
        return Err(RequestError::BadRequest);
    }
    let file_name = update_feed_file_name(Some(target)).ok_or(RequestError::NotFound)?;
    Ok(FeedRequest { method, file_name })
}

async fn send_error(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    allow_get: bool,
) -> io::Result<()> {
    let body = if status == 404 {
        "Not found"
    } else {
        "Update feed unavailable"
    };
    let allow = if allow_get {
        "Allow: GET, HEAD\r\n"
    } else {
        ""
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n{allow}Cache-Control: no-store\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

fn open_artifact(root: &PrivateDirectory, name: &str) -> Result<(File, u64), ()> {
    root.assert_unchanged().map_err(|_| ())?;
    let root_metadata = fs::symlink_metadata(root.path()).map_err(|_| ())?;
    let path = root.path().join(name);
    let before = fs::symlink_metadata(&path).map_err(|_| ())?;
    validate_artifact_metadata(&before, &root_metadata)?;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    let file = options.open(&path).map_err(|_| ())?;
    let opened = file.metadata().map_err(|_| ())?;
    validate_artifact_metadata(&opened, &root_metadata)?;
    if !same_file_state(&before, &opened) {
        return Err(());
    }
    let named_after = fs::symlink_metadata(&path).map_err(|_| ())?;
    if !same_file_state(&opened, &named_after) {
        return Err(());
    }
    root.assert_unchanged().map_err(|_| ())?;
    Ok((file, opened.len()))
}

fn validate_artifact_metadata(
    metadata: &fs::Metadata,
    root_metadata: &fs::Metadata,
) -> Result<(), ()> {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(());
    }
    #[cfg(unix)]
    if metadata.nlink() != 1
        || metadata.uid() != root_metadata.uid()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(());
    }
    if metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(());
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn same_file_state(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mode() == right.mode()
        && left.uid() == right.uid()
        && left.nlink() == right.nlink()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicU32;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    /// A listener that replays a script of accepted streams and refusals. An
    /// exhausted script keeps refusing, the way a broken listener would.
    #[derive(Clone)]
    struct ScriptedListener {
        script: Arc<StdMutex<VecDeque<Option<TcpStream>>>>,
        failures: Arc<AtomicU32>,
    }

    impl ScriptedListener {
        fn new(script: VecDeque<Option<TcpStream>>) -> Self {
            Self {
                script: Arc::new(StdMutex::new(script)),
                failures: Arc::new(AtomicU32::new(0)),
            }
        }

        fn failures(&self) -> u32 {
            self.failures.load(Ordering::SeqCst)
        }
    }

    impl FeedListener for ScriptedListener {
        fn accept(
            &self,
        ) -> impl std::future::Future<Output = io::Result<(TcpStream, SocketAddr)>> + Send {
            let accepted = self
                .script
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .flatten();
            if accepted.is_none() {
                self.failures.fetch_add(1, Ordering::SeqCst);
            }
            std::future::ready(match accepted {
                Some(stream) => Ok((stream, SocketAddr::from(([127, 0, 0, 1], 1)))),
                None => Err(io::Error::other("accept refused")),
            })
        }
    }

    async fn connected_stream() -> TcpStream {
        let listener = TcpListener::bind((LOOPBACK, 0)).await.expect("bind");
        let address = listener.local_addr().expect("address");
        let client = TcpStream::connect(address).await.expect("connect");
        let (server, _) = listener.accept().await.expect("accept");
        drop(client);
        server
    }

    async fn run_accept_loop(listener: ScriptedListener) -> Arc<AtomicBool> {
        let parent = tempfile::tempdir().expect("parent");
        let root = Arc::new(
            PrivateDirectory::open_or_create(parent.path().join("feed")).expect("feed root"),
        );
        let serving = Arc::new(AtomicBool::new(true));
        let (_shutdown, shutdown_receiver) = oneshot::channel();
        accept_loop(listener, root, Arc::clone(&serving), shutdown_receiver).await;
        serving
    }

    async fn request(feed: &LocalUpdateFeed, request: &str) -> String {
        let mut stream = TcpStream::connect(feed.address).await.expect("connect");
        stream.write_all(request.as_bytes()).await.expect("write");
        stream.shutdown().await.expect("finish request");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.expect("response");
        String::from_utf8(response).expect("utf8 response")
    }

    #[tokio::test]
    async fn serves_only_flat_get_and_head_artifacts() {
        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("feed");
        let feed = LocalUpdateFeed::start(&root).await.expect("feed");
        fs::write(root.join("latest-mac.yml"), b"version: 0.2.0\n").expect("manifest");

        let get = request(
            &feed,
            "GET /latest-mac.yml HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(get.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(get.ends_with("version: 0.2.0\n"));
        assert!(get.contains("Cache-Control: no-store\r\n"));

        let head = request(
            &feed,
            "HEAD /latest-mac.yml HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(head.ends_with("\r\n\r\n"));

        let traversal = request(
            &feed,
            "GET /%2e%2e%2fsecret.zip HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(traversal.starts_with("HTTP/1.1 404 Not Found\r\n"));

        let post = request(
            &feed,
            "POST /latest-mac.yml HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(post.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        assert!(post.contains("Allow: GET, HEAD\r\n"));
        feed.close().await.expect("close");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_symlinked_and_hard_linked_artifacts() {
        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("feed");
        let feed = LocalUpdateFeed::start(&root).await.expect("feed");
        let source = root.join("source.zip");
        fs::write(&source, b"artifact").expect("source");
        fs::hard_link(&source, root.join("Interface.zip")).expect("hard link");
        symlink(&source, root.join("Alias.zip")).expect("symlink");

        for name in ["Interface.zip", "Alias.zip"] {
            let response = request(
                &feed,
                &format!("GET /{name} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
            )
            .await;
            assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
        }
        feed.close().await.expect("close");
    }

    #[tokio::test]
    async fn a_persistent_accept_failure_trips_the_feed_off() {
        let listener = ScriptedListener::new(VecDeque::new());
        let serving = run_accept_loop(listener.clone()).await;
        assert_eq!(listener.failures(), MAX_CONSECUTIVE_ACCEPT_FAILURES);
        assert!(!serving.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn an_accepted_client_resets_the_consecutive_failure_count() {
        let mut script: VecDeque<Option<TcpStream>> =
            (1..MAX_CONSECUTIVE_ACCEPT_FAILURES).map(|_| None).collect();
        script.push_back(Some(connected_stream().await));
        let listener = ScriptedListener::new(script);
        let serving = run_accept_loop(listener.clone()).await;
        assert_eq!(listener.failures(), MAX_CONSECUTIVE_ACCEPT_FAILURES * 2 - 1);
        assert!(!serving.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_healthy_feed_reports_that_it_is_serving() {
        let parent = tempfile::tempdir().expect("parent");
        let feed = LocalUpdateFeed::start(&parent.path().join("feed"))
            .await
            .expect("feed");
        assert!(feed.is_serving());
        feed.close().await.expect("close");
    }
}
