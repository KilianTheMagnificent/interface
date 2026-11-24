import AVFoundation
import Speech

private let selfTestPhrase = "Interface voice helper ready"

/// Thread-safe one-shot flag; the synthesiser signals completion off the main actor.
private final class Flag: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false
    func set() { lock.lock(); value = true; lock.unlock() }
    var isSet: Bool { lock.lock(); defer { lock.unlock() }; return value }
}

private func wait(for flag: Flag, timeout: TimeInterval) async {
    let deadline = Date().addingTimeInterval(timeout)
    while !flag.isSet, Date() < deadline {
        try? await Task.sleep(for: .milliseconds(25))
    }
}

private final class SpeechDone: NSObject, AVSpeechSynthesizerDelegate, @unchecked Sendable {
    let flag = Flag()
    func speechSynthesizer(_ s: AVSpeechSynthesizer, didFinish u: AVSpeechUtterance) { flag.set() }
    func speechSynthesizer(_ s: AVSpeechSynthesizer, didCancel u: AVSpeechUtterance) { flag.set() }
}

/// Collects the synthesiser's PCM buffers into a WAV file. A zero-length buffer
/// marks the end of synthesis.
private final class WavSink: @unchecked Sendable {
    private let url: URL
    private let lock = NSLock()
    private var file: AVAudioFile?
    let done = Flag()
    private(set) var frames: AVAudioFramePosition = 0
    private(set) var failure: (any Error)?

    init(url: URL) { self.url = url }

    func append(_ buffer: AVAudioBuffer) {
        lock.lock()
        defer { lock.unlock() }
        guard !done.isSet, let pcm = buffer as? AVAudioPCMBuffer else { return }
        guard pcm.frameLength > 0 else { done.set(); return }
        do {
            if file == nil {
                let format = pcm.format
                file = try AVAudioFile(forWriting: url, settings: format.settings,
                                       commonFormat: format.commonFormat,
                                       interleaved: format.isInterleaved)
            }
            try file?.write(from: pcm)
            frames += AVAudioFramePosition(pcm.frameLength)
        } catch {
            failure = error
            done.set()
        }
    }

    func close() { lock.lock(); file = nil; lock.unlock() }
}

@MainActor
private func speakAloud(_ text: String, voice: AVSpeechSynthesisVoice?) async {
    let synth = AVSpeechSynthesizer()
    let observer = SpeechDone()
    synth.delegate = observer
    let utterance = AVSpeechUtterance(string: text)
    utterance.voice = voice
    synth.speak(utterance)
    await wait(for: observer.flag, timeout: 30)
    withExtendedLifetime(synth) {}
}

@MainActor
private func synthesizeWAV(_ text: String, voice: AVSpeechSynthesisVoice?, to url: URL) async throws {
    try? FileManager.default.removeItem(at: url)
    let synth = AVSpeechSynthesizer()
    let sink = WavSink(url: url)
    let utterance = AVSpeechUtterance(string: text)
    utterance.voice = voice
    synth.write(utterance) { buffer in sink.append(buffer) }
    await wait(for: sink.done, timeout: 30)
    sink.close()
    withExtendedLifetime(synth) {}
    if let failure = sink.failure { throw failure }
    guard sink.frames > 0 else {
        throw NSError(domain: "interface-voice", code: 2, userInfo: [
            NSLocalizedDescriptionKey: "synthesiser produced no audio for voice \(voice?.identifier ?? "default")"
        ])
    }
}

/// The same SpeechAnalyzer pipeline the listener uses, fed from a file.
func transcribe(fileAt url: URL, locale: String) async throws -> String {
    let transcriber = await makeTranscriber(locale: locale)
    let modules: [any SpeechModule] = [transcriber]
    try await ensureAssets(for: modules)

    let file = try AVAudioFile(forReading: url)
    let analyzer = SpeechAnalyzer(modules: modules)
    let collector = Task {
        var text = ""
        for try await result in transcriber.results where result.isFinal {
            text += plainText(result.text)
        }
        return text
    }
    _ = try await analyzer.analyzeSequence(from: file)
    try await analyzer.finalizeAndFinishThroughEndOfInput()
    return try await collector.value.trimmingCharacters(in: .whitespacesAndNewlines)
}

private func words(_ text: String) -> [String] {
    text.lowercased()
        .split { !$0.isLetter && !$0.isNumber }
        .map(String.init)
}

@MainActor
func runSelfTest(wav url: URL) async -> Never {
    let voice = bestVoice()
    log("voice: \(voice?.identifier ?? "system default"), wav: \(url.path)")
    do {
        await speakAloud(selfTestPhrase, voice: voice)
        try await synthesizeWAV(selfTestPhrase, voice: voice, to: url)
        let started = Date()
        let heard = try await transcribe(fileAt: url, locale: "en-US")
        let elapsed = Int(Date().timeIntervalSince(started) * 1000)
        emit(SelfTestMsg(spoken: selfTestPhrase, heard: heard,
                         voice: voice?.identifier ?? "system default", transcribeMs: elapsed))
        let expected = words(selfTestPhrase)
        let matched = Set(words(heard)).intersection(expected).count
        log("matched \(matched)/\(expected.count) words")
        exit(matched >= 3 ? 0 : 1)
    } catch {
        emit(ErrorMsg(message: "selftest: \(error)", fatal: true))
        exit(2)
    }
}
