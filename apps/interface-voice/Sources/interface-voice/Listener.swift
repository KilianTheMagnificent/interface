import AVFoundation
import Speech

// MARK: - Shared speech-model helpers

func microphoneStatus() -> String {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized: return "granted"
    case .notDetermined: return "undetermined"
    default: return "denied"
    }
}

/// Makes sure Apple's on-device model for these modules is present, reporting
/// download progress on the way. Throws only if the install itself fails.
func ensureAssets(for modules: [any SpeechModule]) async throws {
    if await AssetInventory.status(forModules: modules) == .installed {
        emit(AssetMsg(status: "installed", progress: 1.0))
        return
    }
    guard let request = try await AssetInventory.assetInstallationRequest(supporting: modules) else {
        emit(AssetMsg(status: "installed", progress: 1.0))
        return
    }
    emit(AssetMsg(status: "installing", progress: 0.0))
    let poll = Task {
        while !Task.isCancelled {
            try? await Task.sleep(for: .milliseconds(500))
            guard !Task.isCancelled else { return }
            emit(AssetMsg(status: "installing", progress: request.progress.fractionCompleted))
        }
    }
    defer { poll.cancel() }
    try await request.downloadAndInstall()
    emit(AssetMsg(status: "installed", progress: 1.0))
}

func makeTranscriber(locale identifier: String) async -> SpeechTranscriber {
    let requested = Locale(identifier: identifier)
    let locale = await SpeechTranscriber.supportedLocale(equivalentTo: requested) ?? requested
    return SpeechTranscriber(
        locale: locale,
        transcriptionOptions: [],
        reportingOptions: [.volatileResults],
        attributeOptions: []
    )
}

func plainText(_ attributed: AttributedString) -> String {
    String(attributed.characters)
}

/// Mutable state the audio tap owns. The tap block runs on one render thread,
/// so the unchecked conformance is the honest description.
private final class TapContext: @unchecked Sendable {
    let converter: AVAudioConverter
    let outputFormat: AVAudioFormat
    let levels = LevelTelemetry()
    let pendingInput = InputBox()
    var lastLevelAt: CFAbsoluteTime = 0
    init(converter: AVAudioConverter, outputFormat: AVAudioFormat) {
        self.converter = converter
        self.outputFormat = outputFormat
    }
}

// MARK: - Listener

@MainActor
final class Listener {
    var isSpeaking: @MainActor () -> Bool = { false }
    var onBargeIn: @MainActor () -> Void = {}
    var onStateChange: @MainActor () -> Void = {}

    private(set) var listening = false
    private var starting = false
    private var endSilenceMs = 800

    private var engine: AVAudioEngine?
    private var analyzer: SpeechAnalyzer?
    private var continuation: AsyncStream<AnalyzerInput>.Continuation?
    private var tasks: [Task<Void, Never>] = []
    private var endTimer: Task<Void, Never>?

    private var finalized = ""
    private var volatileText = ""
    private var vadRun: Double = 0

    // MARK: Lifecycle

    func start(locale: String, endSilenceMs: Int) async {
        self.endSilenceMs = endSilenceMs
        guard !listening, !starting else { return }
        starting = true
        defer { starting = false }

        if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
            _ = await AVCaptureDevice.requestAccess(for: .audio)
        }
        let permission = microphoneStatus()
        emit(PermissionMsg(microphone: permission))
        guard permission == "granted" else {
            emit(ErrorMsg(message: "microphone permission \(permission)", fatal: false))
            return
        }

        guard SpeechTranscriber.isAvailable else {
            emit(ErrorMsg(message: "SpeechTranscriber is unavailable on this device", fatal: true))
            return
        }

        do {
            let transcriber = await makeTranscriber(locale: locale)
            let detector = SpeechDetector(
                detectionOptions: .init(sensitivityLevel: .medium),
                reportResults: true
            )
            let modules: [any SpeechModule] = [transcriber, detector]
            try await ensureAssets(for: modules)

            guard let analyzerFormat = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: modules) else {
                emit(ErrorMsg(message: "no audio format compatible with the speech modules", fatal: true))
                return
            }

            let (stream, continuation) = AsyncStream<AnalyzerInput>.makeStream(
                bufferingPolicy: .bufferingNewest(64)
            )
            self.continuation = continuation

            let analyzer = SpeechAnalyzer(modules: modules)
            self.analyzer = analyzer

            tasks.append(Task { @MainActor [weak self] in
                do {
                    for try await result in transcriber.results {
                        self?.handle(transcript: result)
                    }
                } catch {
                    emit(ErrorMsg(message: "transcriber: \(error)", fatal: false))
                }
            })
            tasks.append(Task { @MainActor [weak self] in
                do {
                    for try await result in detector.results {
                        self?.handle(detection: result)
                    }
                } catch {
                    emit(ErrorMsg(message: "detector: \(error)", fatal: false))
                }
            })

            try startEngine(analyzerFormat: analyzerFormat, continuation: continuation)
            try await analyzer.start(inputSequence: stream)

            listening = true
            onStateChange()
        } catch {
            emit(ErrorMsg(message: "start: \(error)", fatal: false))
            await stop()
        }
    }

    func stop() async {
        endTimer?.cancel()
        endTimer = nil
        flushFinal()

        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        engine = nil

        continuation?.finish()
        continuation = nil

        if let analyzer {
            try? await analyzer.finalizeAndFinishThroughEndOfInput()
            self.analyzer = nil
        }
        for task in tasks { task.cancel() }
        tasks.removeAll()

        finalized = ""
        volatileText = ""
        vadRun = 0
        if listening {
            listening = false
            onStateChange()
        }
    }

    // MARK: Audio

    private func startEngine(
        analyzerFormat: AVAudioFormat,
        continuation: AsyncStream<AnalyzerInput>.Continuation
    ) throws {
        let engine = AVAudioEngine()
        let input = engine.inputNode
        do {
            // Echo cancellation: keeps the assistant's own speech out of the mic.
            try input.setVoiceProcessingEnabled(true)
        } catch {
            emit(ErrorMsg(message: "voice processing unavailable: \(error)", fatal: false))
        }

        let inputFormat = input.outputFormat(forBus: 0)
        guard let converter = AVAudioConverter(from: inputFormat, to: analyzerFormat) else {
            throw NSError(domain: "interface-voice", code: 1, userInfo: [
                NSLocalizedDescriptionKey: "cannot convert \(inputFormat) to \(analyzerFormat)"
            ])
        }
        let context = TapContext(converter: converter, outputFormat: analyzerFormat)

        input.installTap(onBus: 0, bufferSize: 4096, format: inputFormat) { buffer, _ in
            emitLevel(buffer, context: context)
            if let converted = convert(buffer, with: context) {
                continuation.yield(AnalyzerInput(buffer: converted))
            }
        }
        engine.prepare()
        try engine.start()
        self.engine = engine
    }

    // MARK: Results

    private func handle(transcript result: SpeechTranscriber.Result) {
        let text = plainText(result.text)
        if result.isFinal {
            finalized += text
            volatileText = ""
        } else {
            volatileText = text
        }
        let combined = (finalized + volatileText).trimmingCharacters(in: .whitespacesAndNewlines)
        if !combined.isEmpty {
            emit(TextMsg(type: "partial", text: combined))
        }
        // Barge-in: two words is enough to know the user is talking over us.
        if isSpeaking(), combined.split(separator: " ").count >= 2 {
            onBargeIn()
        }
        armEndTimer()
    }

    private func handle(detection result: SpeechDetector.Result) {
        if result.speechDetected {
            vadRun += result.range.duration.seconds
            armEndTimer()
            if isSpeaking(), vadRun >= 0.3 {
                onBargeIn()
            }
        } else {
            vadRun = 0
        }
    }

    /// The utterance ends `endSilenceMs` after the last transcriber result or
    /// detected speech, and only once the transcriber has finalised something.
    private func armEndTimer() {
        endTimer?.cancel()
        guard !finalized.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        let delay = endSilenceMs
        endTimer = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(delay))
            guard !Task.isCancelled else { return }
            self?.flushFinal()
        }
    }

    private func flushFinal() {
        let text = (finalized + volatileText).trimmingCharacters(in: .whitespacesAndNewlines)
        finalized = ""
        volatileText = ""
        vadRun = 0
        guard !text.isEmpty else { return }
        emit(TextMsg(type: "final", text: text))
    }
}

// MARK: - Tap helpers (called on the audio render thread)

private func convert(_ buffer: AVAudioPCMBuffer, with context: TapContext) -> AVAudioPCMBuffer? {
    let ratio = context.outputFormat.sampleRate / buffer.format.sampleRate
    let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 1024
    guard let output = AVAudioPCMBuffer(pcmFormat: context.outputFormat, frameCapacity: capacity) else {
        return nil
    }
    // The converter's input block is `@Sendable`, but it runs synchronously on
    // this thread; the box hands the buffer over exactly once.
    let pending = context.pendingInput
    pending.set(buffer)
    var error: NSError?
    context.converter.convert(to: output, error: &error) { _, status in
        guard let next = pending.take() else {
            status.pointee = .noDataNow
            return nil
        }
        status.pointee = .haveData
        return next
    }
    // Do not synchronously log on the audio thread if conversion fails.
    if error != nil {
        context.levels.mailbox.reportConversionFailure()
        return nil
    }
    return output.frameLength > 0 ? output : nil
}

private final class InputBox: @unchecked Sendable {
    private var buffer: AVAudioPCMBuffer?
    func set(_ buffer: AVAudioPCMBuffer) { self.buffer = buffer }
    func take() -> AVAudioPCMBuffer? {
        defer { buffer = nil }
        return buffer
    }
}

private func emitLevel(_ buffer: AVAudioPCMBuffer, context: TapContext) {
    let now = CFAbsoluteTimeGetCurrent()
    guard now - context.lastLevelAt >= 0.1 else { return }
    context.lastLevelAt = now
    guard let channel = buffer.floatChannelData?[0] else { return }
    let count = Int(buffer.frameLength)
    guard count > 0 else { return }
    var sum: Float = 0
    for i in 0..<count { sum += channel[i] * channel[i] }
    context.levels.mailbox.publish(Double((sum / Float(count)).squareRoot()))
}
