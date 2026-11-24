import Foundation

@MainActor
final class App {
    private let speaker: any SpeechBackend = Speaker()
    private let listener = Listener()
    private var shuttingDown = false

    func start() {
        listener.isSpeaking = { [speaker] in speaker.speaking }
        listener.onBargeIn = { [speaker] in
            guard speaker.speaking else { return }
            speaker.cancel()
            emit(SimpleMsg(type: "interrupted"))
        }
        listener.onStateChange = { [weak self] in self?.emitState() }
        speaker.onStateChange = { [weak self] in self?.emitState() }

        emit(ReadyMsg(voices: speaker.voices(), defaultVoice: speaker.defaultVoiceIdentifier))
        readStdin()
    }

    private func emitState() {
        emit(StateMsg(listening: listener.listening, speaking: speaker.speaking))
    }

    private func readStdin() {
        Thread.detachNewThread { [self] in
            let handled = DispatchSemaphore(value: 0)
            func deliver(_ data: Data) {
                guard let message = try? JSONDecoder().decode(Incoming.self, from: data) else {
                    emit(ErrorMsg(message: "malformed message", fatal: false))
                    return
                }
                Task { @MainActor in
                    await self.handle(message)
                    handled.signal()
                }
                // Only this reader thread waits. The main actor stays free for
                // speech/microphone callbacks, and commands cannot pile up as
                // an unbounded collection of suspended Tasks.
                handled.wait()
            }
            var framer = VoiceLineFramer()
            do {
                while let data = try FileHandle.standardInput.read(upToCount: 4096), !data.isEmpty {
                    for line in try framer.push(data) { deliver(line) }
                }
                if let final = framer.finish() { deliver(final) }
            } catch {
                emit(ErrorMsg(message: "voice command stream exceeded its limit or failed", fatal: true))
                exit(1)
            }
            Task { @MainActor in await self.shutdown() }
        }
    }

    private func handle(_ message: Incoming) async {
        switch message.type {
        case "start":
            await listener.start(locale: message.locale ?? "en-US",
                                 endSilenceMs: message.endSilenceMs ?? 800)
        case "stop":
            await listener.stop()
        case "speak":
            guard let id = message.id, let text = message.text else {
                emit(ErrorMsg(message: "speak requires id and text", fatal: false))
                return
            }
            speaker.speak(id: id, text: text)
        case "cancel":
            speaker.cancel()
        case "setVoice":
            guard let voice = message.voice else {
                emit(ErrorMsg(message: "setVoice requires voice", fatal: false))
                return
            }
            speaker.setVoice(voice)
        case "shutdown":
            await shutdown()
        default:
            emit(ErrorMsg(message: "unknown message type: \(message.type)", fatal: false))
        }
    }

    /// Stops the microphone, lets queued speech finish, then exits.
    /// Use `cancel` first if you want the queue dropped instead.
    private func shutdown() async {
        guard !shuttingDown else { return }
        shuttingDown = true
        await listener.stop()
        let deadline = Date().addingTimeInterval(30)
        while speaker.speaking, Date() < deadline {
            try? await Task.sleep(for: .milliseconds(25))
        }
        exit(0)
    }
}

let arguments = CommandLine.arguments
let isSelfTest = arguments.contains("--selftest")
// A top-level binding, so the app outlives `start()` and the stdin reader
// thread always has something to hop back to.
let app = isSelfTest ? nil : App()

if isSelfTest {
    let wav = arguments.firstIndex(of: "--wav")
        .map { $0 + 1 }
        .flatMap { $0 < arguments.count ? URL(fileURLWithPath: arguments[$0]) : nil }
        ?? FileManager.default.temporaryDirectory
            .appendingPathComponent("interface-voice-selftest.wav")
    Task { await runSelfTest(wav: wav) }
} else {
    app?.start()
}
// AVFoundation delivers its callbacks through the main run loop, which also
// drains main-actor work. `exit` ends the process.
RunLoop.main.run()
