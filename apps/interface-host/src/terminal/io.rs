//! The binary data plane: the input pump and per-terminal lanes, output
//! pumps and their readable tails, and transport failure.

use super::shutdown::close_active_terminals;
use super::{
    Arc, DispatchError, Duration, Mutex, Ordering, TailBuffer, TerminalDataDirection,
    TerminalDataFrame, TerminalDataKind, TerminalDataReadError, TerminalDataSender, TerminalError,
    TerminalGeneration, TerminalGenerationRequest, TerminalHostDispatcher, TerminalId,
    TerminalOutputPump, TerminalService, TerminalTransportFailure, mpsc, oneshot,
    terminal_closed_error, terminal_dispatch_error, terminal_state_error, watch,
};

/// A backstop only: every credit, drain, close and transport failure wakes a
/// parked pump directly.
const OUTPUT_BACKPRESSURE_RETRY: Duration = Duration::from_millis(250);
// A PTY write blocks until its child reads, so input cannot be applied on the
// one shared pump: a single pane running a build would freeze typing in every
// other pane. Each terminal gets its own bounded lane instead. Frames are
// capped at 64 KiB, so a lane holds at most 4 MiB of pending input.
const TERMINAL_INPUT_LANE_FRAMES: usize = 64;
// ponytail: 64 KiB of raw output per pane generation is all `read_pane` needs.
const OUTPUT_TAIL_BYTES: usize = 64 * 1024;
/// Tails kept before finished generations without a delivered removal are dropped.
const MAX_RETAINED_TAILS: usize = 256;

impl TerminalHostDispatcher {
    /// Starts the unique Electron-to-host byte pump.
    ///
    /// # Errors
    ///
    /// Refuses a second pump or a dispatcher already closing.
    pub fn start_input_pump(
        self: &Arc<Self>,
        inbound: mpsc::Receiver<Result<TerminalDataFrame, TerminalDataReadError>>,
    ) -> Result<(), DispatchError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(terminal_closed_error());
        }
        let mut slot = self.input_pump.lock().map_err(|_| terminal_state_error())?;
        if slot.is_some() {
            return Err(DispatchError::new(
                "terminal-data-owner-conflict",
                "The integrated terminal input plane already has an owner.",
                false,
                true,
            ));
        }
        let owner = Arc::downgrade(self);
        *slot = Some(tokio::spawn(run_input_pump(owner, inbound)));
        Ok(())
    }

    /// Watches the shared binary plane. A terminal byte-pipe failure fences
    /// new process creation and tears down existing invisible shells.
    ///
    /// # Errors
    ///
    /// Refuses a second failure monitor or a dispatcher already closing.
    pub fn start_failure_monitor(
        self: &Arc<Self>,
        failures: watch::Receiver<Option<TerminalTransportFailure>>,
    ) -> Result<(), DispatchError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(terminal_closed_error());
        }
        let mut slot = self
            .failure_monitor
            .lock()
            .map_err(|_| terminal_state_error())?;
        if slot.is_some() {
            return Err(DispatchError::new(
                "terminal-data-owner-conflict",
                "The integrated terminal failure monitor already has an owner.",
                false,
                true,
            ));
        }
        let owner = Arc::downgrade(self);
        *slot = Some(tokio::spawn(run_failure_monitor(owner, failures)));
        Ok(())
    }

    pub(super) fn start_output_pump(
        &self,
        request: TerminalGenerationRequest,
        sender: TerminalDataSender,
    ) -> Result<(), DispatchError> {
        let key = (request.id, request.generation);
        let mut pumps = self
            .output_pumps
            .lock()
            .map_err(|_| terminal_state_error())?;
        pumps.retain(|_, pump| !pump.task.is_finished());
        if pumps.contains_key(&key) {
            return Err(DispatchError::new(
                "terminal-output-owner-conflict",
                "That terminal generation already has an output owner.",
                false,
                false,
            ));
        }
        let reader = self
            .service
            .take_output(request)
            .map_err(|error| terminal_dispatch_error(&error))?;
        let (admit, admission) = oneshot::channel();
        let tail = Arc::new(Mutex::new(TailBuffer::new(OUTPUT_TAIL_BYTES)));
        let pump_tail = Arc::clone(&tail);
        let remote = self.remote_hub.clone();
        let stream = (request.id.as_uuid(), request.generation.0);
        // Output counted while phone access was off is filled in from this
        // tail on the first phone read.
        let seed_tail = Arc::downgrade(&tail);
        remote.replay.register_seeded(
            stream,
            Arc::new(move || {
                let tail = seed_tail.upgrade()?;
                let tail = tail.lock().ok()?;
                Some((tail.total(), tail.bytes()))
            }),
        );
        // Claim the unique reader before committing restart authority, but do
        // not expose bytes for a lifecycle row the renderer cannot yet see.
        // Dropping `admit` on rollback makes this thread release the reader
        // without forwarding or leaving an orphan Electron buffer. The pump
        // lives as long as the generation, so it runs on its own named thread
        // rather than holding a Tokio blocking-pool thread.
        let pump = std::thread::Builder::new()
            .name(format!("iface-pump-{}", request.id))
            .spawn(move || {
                // Released however the pump ends, even by unwinding.
                let _replay = FinishReplayOnDrop(Arc::clone(&remote), stream);
                if admission.blocking_recv().is_ok() {
                    forward_output_observed(&reader, &sender, Some(&pump_tail), Some(&remote));
                }
            })
            .map_err(|_| {
                self.remote_hub.replay.finish(stream);
                DispatchError::new(
                    "terminal-output-unavailable",
                    "The terminal output owner could not start.",
                    false,
                    false,
                )
            })?;
        pumps.insert(
            key,
            TerminalOutputPump {
                task: pump,
                admission: Some(admit),
            },
        );
        if let Ok(mut tails) = self.lifecycle_sink.output_tails.lock() {
            tails.insert(key, tail);
        }
        Ok(())
    }

    /// The last `lines` of plain text a pane generation produced, or `None`
    /// once that generation is no longer shown.
    pub(crate) fn read_tail(
        &self,
        id: TerminalId,
        generation: TerminalGeneration,
        lines: usize,
    ) -> Option<String> {
        let tail = self
            .lifecycle_sink
            .output_tails
            .lock()
            .ok()?
            .get(&(id, generation))
            .cloned()?;
        let tail = tail.lock().ok()?;
        Some(tail.tail_lines(lines))
    }

    /// Types bytes into a visible pane through its own input lane, exactly as
    /// renderer keystrokes arrive.
    pub(crate) fn write_input(
        &self,
        id: TerminalId,
        generation: TerminalGeneration,
        bytes: Vec<u8>,
    ) -> Result<(), DispatchError> {
        let frame = TerminalDataFrame::new(
            id.as_uuid(),
            generation.0,
            0,
            TerminalDataDirection::ElectronToHost,
            TerminalDataKind::Data,
            bytes,
        )
        .map_err(|error| {
            DispatchError::new("terminal-input-invalid", error.to_string(), false, true)
        })?;
        let key = (frame.terminal_id, frame.generation);
        let mut outcome = Ok(());
        self.remote_hub.desktop_input(key, || {
            outcome = self.enqueue_input_frame(frame);
        });
        outcome
    }

    pub(super) fn admit_output_pump(
        &self,
        request: TerminalGenerationRequest,
    ) -> Result<(), DispatchError> {
        let admission = self
            .output_pumps
            .lock()
            .map_err(|_| terminal_state_error())?
            .get_mut(&(request.id, request.generation))
            .and_then(|pump| pump.admission.take())
            .ok_or_else(|| {
                DispatchError::new(
                    "terminal-output-unavailable",
                    "That terminal generation lost its output owner before it became visible.",
                    false,
                    false,
                )
            })?;
        admission.send(()).map_err(|()| {
            DispatchError::new(
                "terminal-output-unavailable",
                "That terminal generation's output owner stopped unexpectedly.",
                false,
                false,
            )
        })
    }

    pub(super) fn cancel_output_pump(&self, request: TerminalGenerationRequest) {
        if let Ok(mut pumps) = self.output_pumps.lock()
            && let Some(pump) = pumps.get_mut(&(request.id, request.generation))
        {
            pump.admission.take();
        }
    }

    /// Hands one Electron-to-host frame to its terminal's own input lane.
    ///
    /// The lane is bounded, so a terminal whose child has stopped reading its
    /// PTY discards only its own excess input — the way a tty discards what its
    /// process will not read. No other terminal's lane is touched, and the
    /// shared byte plane keeps running.
    pub(super) fn route_input_frame(&self, frame: TerminalDataFrame) {
        let key = (frame.terminal_id, frame.generation);
        self.remote_hub.desktop_input(key, || {
            if let Err(error) = self.enqueue_input_frame(frame) {
                tracing::debug!(code = %error.code, "terminal input was not queued");
            }
        });
    }

    pub(super) fn enqueue_input_frame(
        &self,
        frame: TerminalDataFrame,
    ) -> Result<(), DispatchError> {
        if self.closed.load(Ordering::Acquire) || !self.transport_available.load(Ordering::Acquire)
        {
            return Err(terminal_closed_error());
        }
        let id = TerminalId::new(frame.terminal_id);
        let generation = TerminalGeneration(frame.generation);
        // A keystroke never waits on the lifecycle lock that publish holds
        // across durable sidecar writes; it reads the admission mirror. A
        // poisoned lifecycle still refuses input.
        let Ok(mut lanes) = self.lifecycle_sink.input_lanes.lock() else {
            tracing::error!("integrated terminal input lanes are unavailable");
            return Err(terminal_state_error());
        };
        // Admission is checked under the lanes lock, so a terminal hidden
        // after this check has its lane removed after this insert, never
        // before it: no lane can outlive its terminal's visibility.
        let admitted = if !self.lifecycle_sink.state.is_poisoned()
            && let Ok(admission) = self.lifecycle_sink.admission.lock()
        {
            admission.get(&id) == Some(&generation)
        } else {
            tracing::error!(
                id = %id,
                "dropped terminal input because visible lifecycle state is unavailable"
            );
            false
        };
        if !admitted {
            tracing::debug!(
                id = %id,
                generation = generation.0,
                "dropped input for a hidden or uncommitted terminal generation"
            );
            return Err(DispatchError::new(
                "terminal-not-visible",
                "The terminal is no longer available.",
                false,
                true,
            ));
        }
        let lane = match lanes.entry(id) {
            std::collections::btree_map::Entry::Occupied(lane) => lane.into_mut(),
            std::collections::btree_map::Entry::Vacant(slot) => {
                let (sender, frames) = mpsc::channel(TERMINAL_INPUT_LANE_FRAMES);
                let service = self.service.clone();
                std::thread::Builder::new()
                    .name(format!("iface-input-{id}"))
                    .spawn(move || run_input_lane(&service, frames))
                    .map_err(|_| {
                        DispatchError::new(
                            "terminal-input-unavailable",
                            "The terminal input lane could not start.",
                            true,
                            true,
                        )
                    })?;
                slot.insert(sender)
            }
        };
        if lane.try_send(frame).is_err() {
            return Err(DispatchError::new(
                "terminal-input-full",
                "The terminal input queue is full.",
                true,
                true,
            ));
        }
        Ok(())
    }

    pub(super) fn close_input_lanes(&self) {
        if let Ok(mut lanes) = self.lifecycle_sink.input_lanes.lock() {
            lanes.clear();
        }
    }

    /// Forgets finished pumps. Readable tails and phone replay streams are
    /// released by delivered visibility instead, so an exited pane that is
    /// still shown keeps its output and a request never waits on the
    /// lifecycle lock that publish holds across durable writes and push sends.
    pub(super) fn prune_output_pumps(&self) -> Result<(), DispatchError> {
        let mut pumps = self
            .output_pumps
            .lock()
            .map_err(|_| terminal_state_error())?;
        pumps.retain(|_, pump| !pump.task.is_finished());
        // Bound a pump that started after its pane's removal was delivered.
        if let Ok(mut tails) = self.lifecycle_sink.output_tails.lock()
            && tails.len() > MAX_RETAINED_TAILS
        {
            tails.retain(|key, _| pumps.contains_key(key));
        }
        Ok(())
    }

    pub(super) fn mark_transport_failed(&self, message: &str) {
        if !self.transport_available.swap(false, Ordering::AcqRel) {
            return;
        }
        tracing::error!(message, "integrated terminal data plane failed");
        let lifecycle_dispatch = Arc::clone(&self.lifecycle_dispatch_gate);
        let service = self.service.clone();
        // Do not await the lifecycle gate here: this method is also called by
        // a create/restart that already owns it when output admission fails.
        // The detached teardown runs as soon as that dispatch rolls back and
        // releases the gate. Concurrent delayed spawns recheck the atomic flag
        // before entering the service and again before authority commit.
        tokio::spawn(async move {
            let _lifecycle_dispatch = lifecycle_dispatch.lock().await;
            match tokio::task::spawn_blocking(move || close_active_terminals(&service)).await {
                Ok(true) => {}
                Ok(false) => tracing::error!(
                    "terminal processes did not settle before transport-failure teardown timed out"
                ),
                Err(error) => tracing::error!(%error, "terminal transport teardown worker failed"),
            }
        });
    }
}

pub(super) async fn run_input_pump(
    owner: std::sync::Weak<TerminalHostDispatcher>,
    mut inbound: mpsc::Receiver<Result<TerminalDataFrame, TerminalDataReadError>>,
) {
    while let Some(frame) = inbound.recv().await {
        let Some(owner) = owner.upgrade() else {
            return;
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                owner.mark_transport_failed(&error.to_string());
                return;
            }
        };
        owner.route_input_frame(frame);
    }
    if let Some(owner) = owner.upgrade()
        && !owner.closed.load(Ordering::Acquire)
    {
        owner.mark_transport_failed("the terminal input pipe closed");
    }
}

/// Applies one terminal's ordered input on its own blocking thread. A PTY write
/// that parks here parks nothing else.
pub(crate) fn run_input_lane(
    service: &TerminalService,
    mut frames: mpsc::Receiver<TerminalDataFrame>,
) {
    while let Some(frame) = frames.blocking_recv() {
        let request = TerminalGenerationRequest {
            id: TerminalId::new(frame.terminal_id),
            generation: TerminalGeneration(frame.generation),
        };
        let applied = match frame.kind {
            TerminalDataKind::Data => service.write(request, &frame.payload),
            TerminalDataKind::Eof => service.close(request).map(|_| ()),
            TerminalDataKind::OutputAck => continue, // consumed by the transport
        };
        match applied {
            Ok(()) => {}
            Err(TerminalError::NotFound(_) | TerminalError::StaleGeneration { .. }) => {
                tracing::debug!(id = %request.id, generation = request.generation.0, "ignored stale terminal input frame");
            }
            Err(error) => {
                tracing::warn!(%error, id = %request.id, generation = request.generation.0, "terminal input frame was refused");
            }
        }
    }
}

pub(super) async fn run_failure_monitor(
    owner: std::sync::Weak<TerminalHostDispatcher>,
    mut failures: watch::Receiver<Option<TerminalTransportFailure>>,
) {
    loop {
        let failure = failures.borrow().clone();
        if let Some(failure) = failure {
            if let Some(owner) = owner.upgrade() {
                owner.mark_transport_failed(&failure.message);
            }
            return;
        }
        if failures.changed().await.is_err() {
            return;
        }
    }
}

/// Tells the phone replay that a generation's output reader has stopped.
pub(super) struct FinishReplayOnDrop(
    Arc<interface_remote::RemoteHub>,
    interface_remote::replay::StreamKey,
);

impl Drop for FinishReplayOnDrop {
    fn drop(&mut self) {
        self.0.replay.finish(self.1);
    }
}

/// Forwards one generation's output to Electron until EOF, copying the bytes
/// into the pane's readable tail and the phone replay when they exist.
///
/// Chunks already queued behind the first are merged into one frame, so a
/// backlog costs fewer frames, credits and acknowledgements; a lone chunk is
/// still sent as soon as it is read. A single chunk larger than one frame
/// (a larger read size or a filter that grew it) is split rather than ending
/// the pane's output. Frames are numbered here, from zero and without gaps,
/// and EOF carries the number of data frames sent.
pub(crate) fn forward_output_observed(
    reader: &interface_terminal::TerminalOutputReader,
    sender: &TerminalDataSender,
    tail: Option<&Mutex<TailBuffer>>,
    remote: Option<&interface_remote::RemoteHub>,
) {
    let (id, generation) = (reader.id(), reader.generation());
    let batch_bytes = sender.output_batch_bytes().max(1);
    let mut next_sequence = 0;
    let mut send = |payload: Vec<u8>| {
        let Ok(frame) = TerminalDataFrame::new(
            id.as_uuid(),
            generation.0,
            next_sequence,
            TerminalDataDirection::HostToElectron,
            TerminalDataKind::Data,
            payload,
        ) else {
            tracing::error!(id = %id, "terminal core produced an invalid output frame");
            return false;
        };
        next_sequence += 1;
        enqueue_with_backpressure(sender, frame)
    };
    while let Some(bytes) = reader.read_batch(batch_bytes) {
        // The tail first: a phone read seeding the replay from the tail may
        // then find the tail a batch ahead, never behind.
        if let Some(Ok(mut tail)) = tail.map(Mutex::lock) {
            tail.push(&bytes);
        }
        if let Some(remote) = remote {
            remote.replay.append((id.as_uuid(), generation.0), &bytes);
        }
        let delivered = if bytes.len() <= batch_bytes {
            send(bytes)
        } else {
            bytes
                .chunks(batch_bytes)
                .all(|payload| send(payload.to_vec()))
        };
        if !delivered {
            return;
        }
    }
    let frame = TerminalDataFrame::new(
        id.as_uuid(),
        generation.0,
        next_sequence,
        TerminalDataDirection::HostToElectron,
        TerminalDataKind::Eof,
        Vec::new(),
    );
    if let Ok(frame) = frame {
        let _ = enqueue_with_backpressure(sender, frame);
    }
}

pub(super) fn enqueue_with_backpressure(
    sender: &TerminalDataSender,
    frame: TerminalDataFrame,
) -> bool {
    let terminal_id = frame.terminal_id;
    match sender.enqueue_output_waiting(frame, OUTPUT_BACKPRESSURE_RETRY) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, id = %terminal_id, "terminal output plane stopped accepting bytes");
            false
        }
    }
}
