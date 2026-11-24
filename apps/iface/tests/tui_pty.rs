#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use portable_pty::{CommandBuilder, ExitStatus, PtySize, native_pty_system};

const DEADLINE: Duration = Duration::from_secs(15);

fn temporary_root() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("iface-pty-")
        .tempdir_in("/tmp")
        .expect("short temporary root")
}

struct PtyChild {
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<Vec<u8>>>,
    reader: Option<thread::JoinHandle<()>>,
}

impl PtyChild {
    fn spawn(root: &Path, arguments: &[&str]) -> Self {
        Self::spawn_with_provider(root, arguments, None)
    }

    fn spawn_with_provider(root: &Path, arguments: &[&str], provider: Option<&Path>) -> Self {
        let home = root.join("home");
        let projects = home.join("Desktop/Projects");
        fs::create_dir_all(&projects).expect("projects");
        let data = root.join("data");
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("pty");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_iface"));
        command.args(arguments);
        command.cwd(&projects);
        command.env("HOME", &home);
        command.env("IFACE_DATA_DIR", &data);
        command.env("PATH", "/usr/bin:/bin");
        command.env("TERM", "xterm-256color");
        command.env("NO_COLOR", "1");
        if let Some(provider) = provider {
            command.env("CLAUDE_CODE_EXECUTABLE", provider);
            command.env("CODEX_EXECUTABLE", provider);
        }
        let child = pair.slave.spawn_command(command).expect("spawn iface");
        drop(pair.slave);
        let mut input = pair.master.try_clone_reader().expect("reader");
        let writer = pair.master.take_writer().expect("writer");
        let output = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&output);
        let reader = thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            while let Ok(length) = input.read(&mut chunk) {
                if length == 0 {
                    break;
                }
                captured
                    .lock()
                    .expect("output")
                    .extend_from_slice(&chunk[..length]);
            }
        });
        Self {
            master: pair.master,
            child,
            writer,
            output,
            reader: Some(reader),
        }
    }

    fn wait_for_text(&self, needle: &str) {
        let deadline = Instant::now() + DEADLINE;
        while Instant::now() < deadline {
            if self.text().contains(needle) {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("PTY output did not contain {needle:?}: {}", self.text());
    }

    fn wait_for_path(&self, path: &Path) {
        let deadline = Instant::now() + DEADLINE;
        while Instant::now() < deadline {
            if path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "PTY child did not create {}: {}",
            path.display(),
            self.text()
        );
    }

    fn send(&mut self, value: &[u8]) {
        self.writer.write_all(value).expect("pty write");
        self.writer.flush().expect("pty flush");
    }

    fn resize(&self, columns: u16, rows: u16) {
        self.master
            .resize(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("resize");
    }

    fn pid(&self) -> u32 {
        self.child.process_id().expect("pid")
    }

    fn wait(mut self) -> (ExitStatus, String) {
        let deadline = Instant::now() + DEADLINE;
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("try wait") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("PTY child did not exit: {}", self.text());
            }
            thread::sleep(Duration::from_millis(25));
        };
        let output = Arc::clone(&self.output);
        drop(self.writer);
        drop(self.master);
        if let Some(reader) = self.reader.take() {
            reader.join().expect("reader join");
        }
        let text = String::from_utf8_lossy(&output.lock().expect("output")).into_owned();
        (status, text)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().expect("output")).into_owned()
    }
}

#[test]
fn first_paint_does_not_wait_for_slow_provider_discovery() {
    let root = temporary_root();
    let provider = root.path().join("slow-provider");
    // A provider-shaped process, with no authentication or network contact.
    // Discovery takes 30 s and first paint has 15 s: a wide margin, so a
    // loaded machine scanning a freshly linked binary cannot fake a failure.
    fs::write(&provider, "#!/bin/sh\nexec /bin/sleep 30\n").expect("provider");
    fs::set_permissions(&provider, fs::Permissions::from_mode(0o700)).expect("mode");
    let started = Instant::now();
    let mut child =
        PtyChild::spawn_with_provider(root.path(), &["--tui", "fullscreen"], Some(&provider));
    child.wait_for_text("Welcome to iface");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "first paint waited for provider discovery"
    );
    child.send(b"/quit\r");
    let (status, output) = child.wait();
    assert_eq!(status.exit_code(), 0, "{output}");
    assert!(!root.path().join("data/instance.lock").exists());
}

#[test]
fn fullscreen_resize_help_and_quit_restore_the_terminal_and_remove_control_files() {
    let root = temporary_root();
    let data = root.path().join("data");
    fs::create_dir(&data).expect("legacy data root");
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).expect("data mode");
    let legacy_history = b"{\"at\":\"2026-08-27T00:00:00.000Z\",\"engine\":\"claude\",\"cwd\":\"/repo\",\"text\":\"hello\"}\n";
    fs::write(data.join("history"), legacy_history).expect("legacy prompt history");
    fs::set_permissions(data.join("history"), fs::Permissions::from_mode(0o600))
        .expect("history mode");
    let mut child = PtyChild::spawn(root.path(), &["--tui", "fullscreen"]);
    child.wait_for_text("Welcome to iface");
    child.wait_for_path(&data.join("instance.lock"));
    child.wait_for_path(&data.join("iface.sock"));
    child.resize(38, 12);
    child.send(b"/help\r");
    child.wait_for_text("Enter sends when");
    child.send(b"\r");
    thread::sleep(Duration::from_millis(100));
    child.send(b"/quit\r");
    let (status, output) = child.wait();
    assert_eq!(status.exit_code(), 0, "{output}");
    assert!(output.contains("\u{1b}[?1049h"), "{output}");
    assert!(output.contains("\u{1b}[?1049l"), "{output}");
    assert!(!data.join("instance.lock").exists());
    assert!(!data.join("iface.sock").exists());
    assert_eq!(
        fs::read(data.join("history")).expect("legacy history preserved"),
        legacy_history
    );
}

#[test]
fn narrow_cwd_and_slash_palettes_follow_real_pty_input_and_restore_terminal() {
    let root = temporary_root();
    let data = root.path().join("data");
    let projects = root.path().join("home/Desktop/Projects");
    fs::create_dir_all(projects.join("Alpha Project")).expect("cwd fixture");
    let mut child = PtyChild::spawn(root.path(), &["--tui", "fullscreen"]);
    child.wait_for_text("Welcome to iface");
    child.wait_for_path(&data.join("instance.lock"));
    child.wait_for_path(&data.join("iface.sock"));

    child.resize(40, 12);
    child.send(b"/cwd A\r");
    child.wait_for_text("Launch directory");
    child.wait_for_text("Alpha Project");
    child.send(b"\x1b");
    child.send(b"/");
    child.wait_for_text("enter insert");
    child.wait_for_text("/sessions");
    child.send(b"\x03");

    let (status, output) = child.wait();
    assert_eq!(status.exit_code(), 0, "{output}");
    assert!(output.contains("\u{1b}[?1049h"), "{output}");
    assert!(output.contains("\u{1b}[?1049l"), "{output}");
    assert!(!data.join("instance.lock").exists());
    assert!(!data.join("iface.sock").exists());
}

#[test]
fn sigterm_returns_143_after_terminal_socket_and_writer_lock_teardown() {
    let root = temporary_root();
    let data = root.path().join("data");
    let child = PtyChild::spawn(root.path(), &["--tui", "fullscreen"]);
    child.wait_for_text("Welcome to iface");
    child.wait_for_path(&data.join("instance.lock"));
    child.wait_for_path(&data.join("iface.sock"));
    let status = Command::new("/bin/kill")
        .args(["-TERM", &child.pid().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("send SIGTERM");
    assert!(status.success());
    let (status, output) = child.wait();
    assert_eq!(status.exit_code(), 143, "{output}");
    assert!(output.contains("\u{1b}[?1049l"), "{output}");
    assert!(!data.join("instance.lock").exists());
    assert!(!data.join("iface.sock").exists());
}

#[test]
fn inline_sigint_returns_130_without_alternate_screen_and_tears_down_controls() {
    let root = temporary_root();
    let data = root.path().join("data");
    let child = PtyChild::spawn(root.path(), &["--tui", "inline"]);
    child.wait_for_text("Welcome to iface");
    child.wait_for_path(&data.join("instance.lock"));
    child.wait_for_path(&data.join("iface.sock"));
    let signal = Command::new("/bin/kill")
        .args(["-INT", &child.pid().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("send SIGINT");
    assert!(signal.success());
    let (status, output) = child.wait();
    assert_eq!(status.exit_code(), 130, "{output}");
    assert!(!output.contains("\u{1b}[?1049h"), "{output}");
    assert!(!data.join("instance.lock").exists());
    assert!(!data.join("iface.sock").exists());
}

#[test]
fn inline_double_sigint_force_exit_never_emits_alternate_screen_sequences() {
    let root = temporary_root();
    let data = root.path().join("data");
    let child = PtyChild::spawn(root.path(), &["--tui", "inline"]);
    child.wait_for_text("Welcome to iface");
    child.wait_for_path(&data.join("iface.sock"));
    let pid = child.pid().to_string();
    for _ in 0..2 {
        // The second signal may land during the shutdown drain (force exit)
        // or after it; neither path may emit an alternate-screen sequence.
        let _ = Command::new("/bin/kill")
            .args(["-INT", &pid])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let (status, output) = child.wait();
    assert_eq!(status.exit_code(), 130, "{output}");
    assert!(!output.contains("\u{1b}[?1049h"), "{output}");
    assert!(!output.contains("\u{1b}[?1049l"), "{output}");
}

#[test]
fn keyboard_remains_responsive_during_delayed_provider_discovery() {
    let root = temporary_root();
    let provider = root.path().join("slow-provider");
    fs::write(&provider, "#!/bin/sh\nexec /bin/sleep 4\n").unwrap();
    fs::set_permissions(&provider, fs::Permissions::from_mode(0o700)).unwrap();
    let mut child =
        PtyChild::spawn_with_provider(root.path(), &["--tui", "fullscreen"], Some(&provider));
    child.wait_for_text("Welcome to iface");
    thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    child.resize(100, 30);
    child.send(b"\x1b[200~draft-during-discovery\x1b[201~");
    child.wait_for_text("draft-during-discovery");
    let latency = started.elapsed();
    child.send(b"\x15/quit\r");
    let (status, output) = child.wait();
    assert_eq!(status.exit_code(), 0, "{output}");
    eprintln!("delayed discovery keyboard latency: {latency:?}");
    assert!(
        latency < Duration::from_secs(1),
        "provider discovery blocked the keyboard for {latency:?}"
    );
}

/// A Claude-shaped fake: answers `--version`, then speaks just enough of the
/// stream-json protocol to become ready and finish each turn at once. After
/// each prompt it stops reading stdin while `hold` exists, so a large send
/// stays in flight, and it records every prompt in `received`.
const HOLDING_PROVIDER: &str = r#"#!/bin/sh
case "${1:-}" in --version|-v|version) printf '2.1.0 (Claude Code)\n'; exit 0 ;; esac
root=${0%/*}
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      id=${line#*\"request_id\":\"}
      id=${id%%\"*}
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"commands":[],"agents":[],"models":[]}}}\n' "$id"
      ;;
    *'"type":"user"'*)
      printf 'prompt\n' >> "$root/received"
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"ok"}'
      while [ -e "$root/hold" ]; do /bin/sleep 0.05; done
      ;;
  esac
done
"#;

fn provider_script(root: &Path, name: &str, source: &str) -> std::path::PathBuf {
    let provider = root.join(name);
    fs::write(&provider, source).expect("provider");
    fs::set_permissions(&provider, fs::Permissions::from_mode(0o700)).expect("provider mode");
    provider
}

fn received_prompts(root: &Path) -> usize {
    fs::read_to_string(root.join("received"))
        .map(|text| text.lines().count())
        .unwrap_or_default()
}

fn wait_for_prompts(root: &Path, count: usize) {
    let deadline = Instant::now() + DEADLINE;
    while received_prompts(root) < count {
        assert!(
            Instant::now() < deadline,
            "the provider received {} of {count} prompts",
            received_prompts(root)
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn ctrl_c_during_a_slow_provider_start_exits_without_waiting_for_it() {
    let root = temporary_root();
    let data = root.path().join("data");
    // Never initializes: the start waits on Claude's initialize deadline.
    let provider = provider_script(
        root.path(),
        "slow-start",
        "#!/bin/sh\ncase \"${1:-}\" in --version|-v|version) printf '2.1.0 (Claude Code)\\n'; exit 0 ;; esac\nexec /bin/sleep 20\n",
    );
    let mut child =
        PtyChild::spawn_with_provider(root.path(), &["--tui", "fullscreen"], Some(&provider));
    child.wait_for_text("Welcome to iface");
    child.wait_for_path(&data.join("iface.sock"));
    child.send(b"hello\r");
    // The frame after Enter draws while the start is still pending.
    child.wait_for_text("Starting Claude");
    let pressed = Instant::now();
    child.send(b"\x03");
    let (status, output) = child.wait();
    let waited = pressed.elapsed();
    assert_eq!(status.exit_code(), 0, "{output}");
    assert!(
        waited < Duration::from_secs(6),
        "Ctrl+C waited {waited:?} for the provider start"
    );
    assert!(output.contains("\u{1b}[?1049l"), "{output}");
    assert!(!data.join("instance.lock").exists());
    assert!(!data.join("iface.sock").exists());
}

#[test]
fn a_second_enter_during_an_in_flight_send_sends_the_prompt_once() {
    let root = temporary_root();
    let hold = root.path().join("hold");
    fs::write(&hold, b"").expect("hold");
    let provider = provider_script(root.path(), "holding-provider", HOLDING_PROVIDER);
    let mut child =
        PtyChild::spawn_with_provider(root.path(), &["--tui", "fullscreen"], Some(&provider));
    child.wait_for_text("Welcome to iface");
    child.send(b"first\r");
    wait_for_prompts(root.path(), 1);
    child.wait_for_text("idle");

    // Larger than a pipe buffer: while the provider holds, the write cannot
    // finish, so the send stays in flight.
    let mut paste = b"\x1b[200~".to_vec();
    paste.extend(b"large prompt ".repeat(10_000));
    paste.extend(b"\x1b[201~\r");
    child.send(&paste);
    child.wait_for_text("Sending");
    child.send(b"\r");
    thread::sleep(Duration::from_millis(300));
    fs::remove_file(&hold).expect("release the provider");
    wait_for_prompts(root.path(), 2);
    thread::sleep(Duration::from_millis(500));
    assert_eq!(
        received_prompts(root.path()),
        2,
        "the second Enter sent nothing"
    );

    child.send(b"/quit\r");
    let (status, output) = child.wait();
    assert_eq!(status.exit_code(), 0, "{output}");
}

#[test]
fn ctrl_y_writes_the_fullscreen_transcript_to_native_scrollback() {
    let root = temporary_root();
    let provider = provider_script(root.path(), "holding-provider", HOLDING_PROVIDER);
    let mut child =
        PtyChild::spawn_with_provider(root.path(), &["--tui", "fullscreen"], Some(&provider));
    child.wait_for_text("Welcome to iface");
    child.send(b"dump-marker\r");
    wait_for_prompts(root.path(), 1);
    child.wait_for_text("idle");
    child.send(b"\x19");
    child.wait_for_text("to scrollback.");
    child.send(b"/quit\r");
    let (status, output) = child.wait();
    assert_eq!(status.exit_code(), 0, "{output}");
    // As in 0.1.169: leave the alternate screen, print the plain transcript
    // on the primary screen (CRLF, as raw mode translates nothing), return.
    let left = output
        .find("\u{1b}[?1049l\r\n")
        .expect("the dump leaves the alternate screen");
    let dump_start = left + "\u{1b}[?1049l\r\n".len();
    let dump_end = dump_start
        + output[dump_start..]
            .find("\r\n\u{1b}[?1049h")
            .expect("the dump returns to the alternate screen");
    let dump = &output[dump_start..dump_end];
    assert!(dump.contains("› dump-marker"), "{dump:?}");
    assert!(!dump.contains('\u{1b}'), "{dump:?}");
    assert!(!dump.replace("\r\n", "").contains('\n'), "{dump:?}");
    // The frame after it shows only a short count in the footer.
    assert!(output[dump_end..].contains("to scrollback."), "{output}");
}
