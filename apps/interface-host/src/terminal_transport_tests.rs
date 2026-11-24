use super::*;
use super::{TerminalOutboundShared, TerminalOutboundState};
use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, watch};

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

#[test]
fn v2_frames_and_windows_match_the_electron_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/terminal-data-v2.json"
    ))
    .unwrap();
    assert_eq!(fixture["version"], TERMINAL_DATA_VERSION);
    assert_eq!(fixture["windowBytes"], TERMINAL_OUTPUT_WINDOW_BYTES);
    assert_eq!(fixture["windowFrames"], TERMINAL_OUTPUT_WINDOW_FRAMES);
    for item in fixture["frames"].as_array().unwrap() {
        let payload: Vec<u8> = serde_json::from_value(item["payload"].clone()).unwrap();
        let direction = u8::try_from(item["direction"].as_u64().unwrap()).unwrap();
        let kind = u8::try_from(item["kind"].as_u64().unwrap()).unwrap();
        let frame = TerminalDataFrame::new(
            Uuid::parse_str(fixture["terminalId"].as_str().unwrap()).unwrap(),
            fixture["generation"].as_u64().unwrap(),
            item["sequence"].as_u64().unwrap(),
            TerminalDataDirection::try_from(direction).unwrap(),
            TerminalDataKind::try_from(kind).unwrap(),
            payload,
        )
        .unwrap();
        let hex = item["hex"].as_str().unwrap();
        let expected = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(frame.encode().unwrap(), expected);
    }
}

#[test]
fn output_credits_survive_writer_drain_and_isolate_siblings_and_generations() {
    let sender = test_sender(TerminalQueueLimits::default());
    for sequence in 0..64 {
        sender
            .enqueue_output(frame(
                id(1),
                sequence,
                TerminalDataDirection::HostToElectron,
                b"x",
            ))
            .unwrap();
        // Writing to the pipe must not grant downstream consumption credit.
        sender.shared.state.lock().unwrap().queue.pop().unwrap();
    }
    let blocked = frame(id(1), 64, TerminalDataDirection::HostToElectron, b"x");
    assert!(sender.enqueue_output(blocked.clone()).is_err());
    sender
        .enqueue_output(frame(
            id(2),
            0,
            TerminalDataDirection::HostToElectron,
            b"sibling",
        ))
        .unwrap();
    let ack = |generation, sequence| {
        TerminalDataFrame::new(
            id(1),
            generation,
            sequence,
            TerminalDataDirection::ElectronToHost,
            TerminalDataKind::OutputAck,
            Vec::new(),
        )
        .unwrap()
    };
    sender.acknowledge(&ack(2, 63)).unwrap();
    assert!(sender.enqueue_output(blocked.clone()).is_err());
    assert!(sender.acknowledge(&ack(1, 64)).is_err());
    sender.acknowledge(&ack(1, 31)).unwrap();
    sender.enqueue_output(blocked).unwrap();
    sender.acknowledge(&ack(1, 31)).unwrap(); // duplicate grants nothing
    let key = TerminalStreamKey::from(&frame(id(1), 0, TerminalDataDirection::HostToElectron, b""));
    assert_eq!(
        sender.shared.state.lock().unwrap().outstanding[&key]
            .frames
            .len(),
        33
    );
}

#[tokio::test]
async fn real_pipe_credit_wait_preserves_sibling_input_and_wakes_on_ack_or_close() {
    let (mut electron_input, host_input) = tokio::io::duplex(65_536);
    let (host_output, mut electron_output) = tokio::io::duplex(65_536);
    let mut plane = TerminalDataPlane::start(
        host_input,
        host_output,
        TerminalDataTransportConfig::default(),
    );
    for sequence in 0..64 {
        plane
            .outbound
            .enqueue_output(frame(
                id(1),
                sequence,
                TerminalDataDirection::HostToElectron,
                b"x",
            ))
            .unwrap();
    }
    let mut delivered = vec![0; 64 * (TERMINAL_DATA_HEADER_BYTES + 1)];
    tokio::time::timeout(
        Duration::from_secs(2),
        electron_output.read_exact(&mut delivered),
    )
    .await
    .unwrap()
    .unwrap();
    let sender = plane.outbound.clone();
    let parked = tokio::task::spawn_blocking(move || {
        sender.enqueue_output_waiting(
            frame(id(1), 64, TerminalDataDirection::HostToElectron, b"x"),
            Duration::from_millis(10),
        )
    });
    assert!(!parked.is_finished());
    electron_input
        .write_all(
            &frame(id(2), 0, TerminalDataDirection::ElectronToHost, b"ping")
                .encode()
                .unwrap(),
        )
        .await
        .unwrap();
    let sibling = tokio::time::timeout(Duration::from_secs(2), plane.inbound.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(sibling.terminal_id, id(2));
    assert_eq!(sibling.payload, b"ping");
    let ack = TerminalDataFrame::new(
        id(1),
        1,
        0,
        TerminalDataDirection::ElectronToHost,
        TerminalDataKind::OutputAck,
        Vec::new(),
    )
    .unwrap();
    electron_input
        .write_all(&ack.encode().unwrap())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), parked)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        plane.inbound.try_recv().is_err(),
        "ACK leaked into the PTY input lane"
    );
    let sender = plane.outbound.clone();
    let parked = tokio::task::spawn_blocking(move || {
        sender.enqueue_output_waiting(
            frame(id(1), 65, TerminalDataDirection::HostToElectron, b"x"),
            Duration::from_millis(10),
        )
    });
    plane.outbound.close();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), parked)
            .await
            .unwrap()
            .unwrap(),
        Err(TerminalDataSendError::Closed)
    );
    plane.shutdown().await;
}

fn frame(
    terminal_id: Uuid,
    sequence: u64,
    direction: TerminalDataDirection,
    payload: &[u8],
) -> TerminalDataFrame {
    TerminalDataFrame::new(
        terminal_id,
        1,
        sequence,
        direction,
        TerminalDataKind::Data,
        payload.to_vec(),
    )
    .expect("test frame should be valid")
}

#[test]
fn decoder_accepts_fragmented_and_coalesced_ordered_frames() {
    let first = frame(id(1), 0, TerminalDataDirection::HostToElectron, b"one");
    let second = frame(id(1), 1, TerminalDataDirection::HostToElectron, b"two");
    let mut bytes = first.encode().unwrap();
    bytes.extend(second.encode().unwrap());
    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    let mut received_frames = Vec::new();
    for chunk in bytes.chunks(7) {
        received_frames.extend(decoder.push(chunk).unwrap());
    }
    decoder.finish().unwrap();
    assert_eq!(received_frames, vec![first, second]);
}

#[test]
fn decoder_rejects_invalid_oversized_and_out_of_order_frames() {
    let valid = frame(id(2), 0, TerminalDataDirection::HostToElectron, b"x");

    let mut invalid_magic = valid.encode().unwrap();
    invalid_magic[0] = b'X';
    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    assert_eq!(
        decoder.push(&invalid_magic).unwrap_err(),
        TerminalDataProtocolError::InvalidMagic
    );

    let mut oversized = valid.encode().unwrap();
    oversized[PAYLOAD_LENGTH_OFFSET..TERMINAL_DATA_HEADER_BYTES]
        .copy_from_slice(&65_537_u32.to_le_bytes());
    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    assert!(matches!(
        decoder.push(&oversized),
        Err(TerminalDataProtocolError::PayloadTooLarge { .. })
    ));

    let out_of_order = frame(id(2), 1, TerminalDataDirection::HostToElectron, b"x");
    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    assert_eq!(
        decoder.push(&out_of_order.encode().unwrap()).unwrap_err(),
        TerminalDataProtocolError::InvalidSequence {
            expected: 0,
            actual: 1
        }
    );
}

#[test]
fn decoder_enforces_eof_and_reports_partial_pipe_eof() {
    let eof = TerminalDataFrame::new(
        id(3),
        1,
        0,
        TerminalDataDirection::HostToElectron,
        TerminalDataKind::Eof,
        Vec::new(),
    )
    .unwrap();
    let after = frame(id(3), 1, TerminalDataDirection::HostToElectron, b"late");
    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    assert_eq!(decoder.push(&eof.encode().unwrap()).unwrap(), vec![eof]);
    assert_eq!(
        decoder.push(&after.encode().unwrap()).unwrap_err(),
        TerminalDataProtocolError::FrameAfterEof
    );

    let encoded = frame(id(4), 0, TerminalDataDirection::HostToElectron, b"partial")
        .encode()
        .unwrap();
    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    decoder.push(&encoded[..13]).unwrap();
    assert_eq!(
        decoder.finish().unwrap_err(),
        TerminalDataProtocolError::TruncatedFrame { buffered: 13 }
    );
}

#[test]
fn queue_is_round_robin_fair_and_applies_per_terminal_backpressure() {
    let frame_bytes = TERMINAL_DATA_HEADER_BYTES + 1;
    let mut queue = FairTerminalFrameQueue::new(TerminalQueueLimits {
        total_bytes: frame_bytes * 4,
        per_terminal_bytes: frame_bytes * 2,
    });
    let a0 = frame(id(10), 0, TerminalDataDirection::HostToElectron, b"a");
    let a1 = frame(id(10), 1, TerminalDataDirection::HostToElectron, b"b");
    let b0 = frame(id(11), 0, TerminalDataDirection::HostToElectron, b"c");
    queue.try_push(a0.clone()).unwrap();
    queue.try_push(a1.clone()).unwrap();
    let overflow = frame(id(10), 2, TerminalDataDirection::HostToElectron, b"d");
    match queue.try_push(overflow.clone()) {
        Err((returned, TerminalQueueError::Backpressure)) => {
            assert_eq!(returned, overflow);
        }
        other => panic!("expected backpressure returning the frame, got {other:?}"),
    }
    queue.try_push(b0.clone()).unwrap();
    assert_eq!(queue.pop(), Some(a0));
    assert_eq!(queue.pop(), Some(b0));
    assert_eq!(queue.pop(), Some(a1));
    assert!(queue.is_empty());
    assert_eq!(queue.total_bytes(), 0);
}

#[tokio::test]
async fn data_plane_delivers_input_and_clean_pipe_eof() {
    let (mut electron_input, host_input) = tokio::io::duplex(4096);
    let (host_output, _electron_output) = tokio::io::duplex(4096);
    let mut plane = TerminalDataPlane::start(
        host_input,
        host_output,
        TerminalDataTransportConfig::default(),
    );
    let input = frame(id(20), 0, TerminalDataDirection::ElectronToHost, b"input");
    let encoded = input.encode().unwrap();
    electron_input.write_all(&encoded[..9]).await.unwrap();
    electron_input.write_all(&encoded[9..]).await.unwrap();
    electron_input.shutdown().await.unwrap();
    assert_eq!(plane.inbound.recv().await.unwrap().unwrap(), input);
    assert!(plane.inbound.recv().await.is_none());
    plane.shutdown().await;
}

#[tokio::test]
async fn data_plane_writes_fair_order_and_surfaces_truncated_eof() {
    let (mut electron_input, host_input) = tokio::io::duplex(4096);
    let (host_output, mut electron_output) = tokio::io::duplex(4096);
    let mut plane = TerminalDataPlane::start(
        host_input,
        host_output,
        TerminalDataTransportConfig::default(),
    );
    let a0 = frame(id(30), 0, TerminalDataDirection::HostToElectron, b"a");
    let a1 = frame(id(30), 1, TerminalDataDirection::HostToElectron, b"b");
    let b0 = frame(id(31), 0, TerminalDataDirection::HostToElectron, b"c");
    plane.outbound.enqueue_output(a0.clone()).unwrap();
    plane.outbound.enqueue_output(a1.clone()).unwrap();
    plane.outbound.enqueue_output(b0.clone()).unwrap();
    let expected_bytes = a0.encoded_len() + a1.encoded_len() + b0.encoded_len();
    let mut output = vec![0_u8; expected_bytes];
    electron_output.read_exact(&mut output).await.unwrap();
    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    assert_eq!(decoder.push(&output).unwrap(), vec![a0, b0, a1]);

    electron_input.write_all(b"ITRM").await.unwrap();
    electron_input.shutdown().await.unwrap();
    assert!(matches!(
        plane.inbound.recv().await.unwrap(),
        Err(TerminalDataReadError::Protocol(
            TerminalDataProtocolError::TruncatedFrame { buffered: 4 }
        ))
    ));
    let mut failures = plane.subscribe_failures();
    if failures.borrow().is_none() {
        failures.changed().await.unwrap();
    }
    assert_eq!(
        failures.borrow().as_ref().map(|failure| failure.side),
        Some(TerminalTransportSide::Read)
    );
    plane.shutdown().await;
}

fn one_frame_sender() -> TerminalDataSender {
    let frame_bytes = TERMINAL_DATA_HEADER_BYTES + 1;
    test_sender(TerminalQueueLimits {
        total_bytes: frame_bytes,
        per_terminal_bytes: frame_bytes,
    })
}

fn test_sender(limits: TerminalQueueLimits) -> TerminalDataSender {
    test_sender_with_failures(limits).0
}

fn test_sender_with_failures(
    limits: TerminalQueueLimits,
) -> (
    TerminalDataSender,
    watch::Sender<Option<TerminalTransportFailure>>,
) {
    let (failures_tx, failures) = watch::channel(None);
    let sender = TerminalDataSender {
        shared: Arc::new(TerminalOutboundShared {
            state: Mutex::new(TerminalOutboundState::new(limits)),
            notify: Notify::new(),
            capacity: Condvar::new(),
            failures,
        }),
        maximum_payload: TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    };
    (sender, failures_tx)
}

#[test]
fn encode_into_matches_encode_and_concatenates() {
    let first = frame(id(1), 0, TerminalDataDirection::HostToElectron, b"one");
    let second = frame(id(1), 1, TerminalDataDirection::HostToElectron, b"two");
    let mut scratch = Vec::new();
    first.encode_into(&mut scratch).unwrap();
    assert_eq!(scratch, first.encode().unwrap());
    second.encode_into(&mut scratch).unwrap();
    let mut expected = first.encode().unwrap();
    expected.extend(second.encode().unwrap());
    assert_eq!(scratch, expected);
}

#[test]
fn decoder_drains_many_coalesced_frames_once() {
    let mut bytes = Vec::new();
    let mut expected = Vec::new();
    for sequence in 0..24 {
        let next = frame(id(5), sequence, TerminalDataDirection::HostToElectron, b"x");
        bytes.extend(next.encode().unwrap());
        expected.push(next);
    }
    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    assert_eq!(decoder.push(&bytes).unwrap(), expected);
    decoder.finish().unwrap();
}

#[test]
fn enqueue_output_returns_frame_on_backpressure_without_advancing_sequence() {
    let sender = one_frame_sender();
    let first = frame(id(40), 0, TerminalDataDirection::HostToElectron, b"a");
    let second = frame(id(40), 1, TerminalDataDirection::HostToElectron, b"b");
    sender.enqueue_output(first.clone()).unwrap();
    match sender.enqueue_output(second.clone()) {
        Err((Some(returned), TerminalDataSendError::Queue(TerminalQueueError::Backpressure))) => {
            assert_eq!(returned, second);
        }
        other => panic!("expected returned frame on backpressure, got {other:?}"),
    }
    {
        let mut state = sender.shared.state.lock().expect("outbound lock");
        assert_eq!(state.queue.pop(), Some(first));
    }
    sender.enqueue_output(second.clone()).unwrap();
    let mut state = sender.shared.state.lock().expect("outbound lock");
    assert_eq!(state.queue.pop(), Some(second));
    assert!(state.queue.is_empty());
}

#[test]
fn enqueue_output_does_not_return_frame_on_protocol_errors() {
    let sender = test_sender(TerminalQueueLimits::default());
    let eof = TerminalDataFrame::new(
        id(41),
        1,
        0,
        TerminalDataDirection::HostToElectron,
        TerminalDataKind::Eof,
        Vec::new(),
    )
    .unwrap();
    sender.enqueue_output(eof).unwrap();
    let after = frame(id(41), 1, TerminalDataDirection::HostToElectron, b"late");
    match sender.enqueue_output(after) {
        Err((None, TerminalDataSendError::Protocol(TerminalDataProtocolError::FrameAfterEof))) => {}
        other => panic!("expected FrameAfterEof without a returned frame, got {other:?}"),
    }
    let wrong_way = frame(id(42), 0, TerminalDataDirection::ElectronToHost, b"x");
    match sender.enqueue_output(wrong_way) {
        Err((
            None,
            TerminalDataSendError::Protocol(TerminalDataProtocolError::UnexpectedDirection),
        )) => {}
        other => panic!("expected UnexpectedDirection without a returned frame, got {other:?}"),
    }
}

#[test]
fn enqueue_output_waiting_unblocks_when_capacity_is_freed() {
    let sender = one_frame_sender();
    let first = frame(id(43), 0, TerminalDataDirection::HostToElectron, b"a");
    let second = frame(id(43), 1, TerminalDataDirection::HostToElectron, b"b");
    sender.enqueue_output(first).unwrap();
    let waiter = sender.clone();
    let parked = std::thread::spawn(move || {
        waiter.enqueue_output_waiting(second, Duration::from_millis(50))
    });
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(1) {
        if sender
            .shared
            .state
            .lock()
            .expect("outbound lock")
            .queue
            .is_empty()
        {
            break;
        }
        let mut state = sender.shared.state.lock().expect("outbound lock");
        if state.queue.pop().is_some() {
            drop(state);
            sender.shared.capacity.notify_all();
            break;
        }
        std::thread::yield_now();
    }
    parked
        .join()
        .expect("waiting enqueue thread")
        .expect("waiting enqueue should succeed after drain");
}

#[test]
fn a_transport_failure_wakes_a_parked_pump_before_its_backstop() {
    let frame_bytes = TERMINAL_DATA_HEADER_BYTES + 1;
    let (sender, failures) = test_sender_with_failures(TerminalQueueLimits {
        total_bytes: frame_bytes,
        per_terminal_bytes: frame_bytes,
    });
    let first = frame(id(45), 0, TerminalDataDirection::HostToElectron, b"a");
    let second = frame(id(45), 1, TerminalDataDirection::HostToElectron, b"b");
    sender.enqueue_output(first).unwrap();
    let waiter = sender.clone();
    let parked = std::thread::spawn(move || {
        // The backstop is far longer than the test may take.
        waiter.enqueue_output_waiting(second, Duration::from_secs(120))
    });
    std::thread::sleep(Duration::from_millis(20));
    let started = std::time::Instant::now();
    report_failure(
        &failures,
        TerminalTransportSide::Write,
        "pipe closed".to_owned(),
        &sender.shared,
    );
    let error = parked
        .join()
        .expect("waiting enqueue thread")
        .expect_err("a failed transport refuses the parked frame");
    assert_eq!(
        error,
        TerminalDataSendError::Failed("pipe closed".to_owned())
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn enqueue_output_waiting_unblocks_on_close() {
    let sender = one_frame_sender();
    let first = frame(id(44), 0, TerminalDataDirection::HostToElectron, b"a");
    let second = frame(id(44), 1, TerminalDataDirection::HostToElectron, b"b");
    sender.enqueue_output(first).unwrap();
    let waiter = sender.clone();
    let parked = std::thread::spawn(move || {
        waiter.enqueue_output_waiting(second, Duration::from_millis(50))
    });
    sender.close();
    let error = parked
        .join()
        .expect("waiting enqueue thread")
        .expect_err("close should refuse the parked frame");
    assert_eq!(error, TerminalDataSendError::Closed);
}

/// Drives a plane over inherited-style descriptors: the host ends are opened
/// through `/dev/fd` exactly as `from_process_pipes` opens fds 3 and 4.
async fn exchange_over_descriptors(
    host_input: std::fs::File,
    host_output: std::fs::File,
    mut electron_input: impl std::io::Write + Send + 'static,
    mut electron_output: impl std::io::Read + Send + 'static,
) {
    let mut plane = TerminalDataPlane::start(
        process_reader(host_input).expect("readiness reader"),
        process_writer(host_output).expect("readiness writer"),
        TerminalDataTransportConfig::default(),
    );
    // Many small frames plus a full one: more slices than one writev takes
    // and more bytes than one socket buffer, so partial writes are exercised.
    let mut expected = Vec::new();
    for terminal in 0_u128..40 {
        let output = frame(
            id(terminal + 100),
            0,
            TerminalDataDirection::HostToElectron,
            b"x",
        );
        plane.outbound.enqueue_output(output.clone()).unwrap();
        expected.push(output);
    }
    let large = frame(
        id(99),
        0,
        TerminalDataDirection::HostToElectron,
        &vec![b'y'; TERMINAL_DATA_MAX_PAYLOAD_BYTES],
    );
    plane.outbound.enqueue_output(large.clone()).unwrap();
    expected.push(large);
    let expected_bytes = expected.iter().map(TerminalDataFrame::encoded_len).sum();
    let delivered = tokio::task::spawn_blocking(move || {
        let mut bytes = vec![0_u8; expected_bytes];
        electron_output.read_exact(&mut bytes).map(|()| bytes)
    });
    let delivered = tokio::time::timeout(Duration::from_secs(5), delivered)
        .await
        .expect("output arrives")
        .expect("reader thread")
        .expect("output bytes");
    let mut decoder = TerminalDataDecoder::new(
        TerminalDataDirection::HostToElectron,
        TERMINAL_DATA_MAX_PAYLOAD_BYTES,
    );
    let mut frames = decoder.push(&delivered).unwrap();
    frames.sort_by_key(|frame| frame.terminal_id);
    expected.sort_by_key(|frame| frame.terminal_id);
    assert_eq!(frames, expected);

    let input = frame(id(7), 0, TerminalDataDirection::ElectronToHost, b"typed");
    let encoded = input.encode().unwrap();
    tokio::task::spawn_blocking(move || electron_input.write_all(&encoded))
        .await
        .expect("writer thread")
        .expect("input bytes");
    let received = tokio::time::timeout(Duration::from_secs(5), plane.inbound.recv())
        .await
        .expect("input arrives")
        .expect("inbound open")
        .expect("valid input frame");
    assert_eq!(received, input);
    plane.shutdown().await;
}

#[tokio::test]
async fn inherited_socket_descriptors_carry_frames_both_ways() {
    use std::os::fd::AsRawFd;
    let (host_input, electron_input) = std::os::unix::net::UnixStream::pair().unwrap();
    let (host_output, electron_output) = std::os::unix::net::UnixStream::pair().unwrap();
    let input = open_process_descriptor(host_input.as_raw_fd(), true).expect("fd4 via /dev/fd");
    let output = open_process_descriptor(host_output.as_raw_fd(), false).expect("fd3 via /dev/fd");
    drop((host_input, host_output));
    exchange_over_descriptors(input, output, electron_input, electron_output).await;
}

#[tokio::test]
async fn inherited_pipe_descriptors_carry_frames_both_ways() {
    let (host_input, electron_input) = std::io::pipe().unwrap();
    let (electron_output, host_output) = std::io::pipe().unwrap();
    exchange_over_descriptors(
        std::fs::File::from(std::os::fd::OwnedFd::from(host_input)),
        std::fs::File::from(std::os::fd::OwnedFd::from(host_output)),
        electron_input,
        electron_output,
    )
    .await;
}

#[tokio::test]
async fn a_regular_file_is_not_a_terminal_data_descriptor() {
    let file = tempfile::tempfile().unwrap();
    let error = process_reader(file.try_clone().unwrap())
        .err()
        .expect("regular file refused");
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(process_writer(file).is_err());
}

#[test]
fn credit_for_one_stream_wakes_only_that_streams_pump() {
    let sender = test_sender(TerminalQueueLimits::default());
    let streams: Vec<Uuid> = (60..64).map(id).collect();
    for terminal in &streams {
        for sequence in 0..TERMINAL_OUTPUT_WINDOW_FRAMES as u64 {
            sender
                .enqueue_output(frame(
                    *terminal,
                    sequence,
                    TerminalDataDirection::HostToElectron,
                    b"x",
                ))
                .unwrap();
        }
    }
    // Every stream's window is full: each pump parks on its own credit.
    let pumps: Vec<_> = streams
        .iter()
        .map(|terminal| {
            let waiter = sender.clone();
            let next = frame(
                *terminal,
                TERMINAL_OUTPUT_WINDOW_FRAMES as u64,
                TerminalDataDirection::HostToElectron,
                b"y",
            );
            std::thread::spawn(move || {
                waiter.enqueue_output_waiting(next, Duration::from_secs(120))
            })
        })
        .collect();
    let parked = || sender.shared.state.lock().unwrap().credit_waiters.len();
    let started = std::time::Instant::now();
    while parked() < streams.len() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "pumps did not park"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let ack = frame(streams[0], 0, TerminalDataDirection::ElectronToHost, b"");
    sender.acknowledge(&ack).unwrap();
    let mut pumps = pumps.into_iter();
    pumps.next().unwrap().join().unwrap().unwrap();
    {
        let state = sender.shared.state.lock().unwrap();
        assert_eq!(state.wakeups, 1, "credit for one stream woke other pumps");
        assert_eq!(state.credit_waiters.len(), streams.len() - 1);
    }
    sender.close();
    for pump in pumps {
        assert!(matches!(
            pump.join().unwrap(),
            Err(TerminalDataSendError::Closed)
        ));
    }
}
