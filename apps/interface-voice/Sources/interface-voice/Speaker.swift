import AVFoundation

func qualityName(_ q: AVSpeechSynthesisVoiceQuality) -> String {
    switch q {
    case .premium: return "premium"
    case .enhanced: return "enhanced"
    default: return "default"
    }
}

/// Legacy Eloquence and novelty voices report the same "default" quality as the
/// modern catalog but sound nothing like speech you would ship, so the catalog
/// wins any tie inside a quality tier.
private func familyRank(_ identifier: String) -> Int {
    if identifier.hasPrefix("com.apple.voice.") { return 2 }
    if identifier.hasPrefix("com.apple.ttsbundle.") { return 1 }
    return 0
}

/// Best voice for a language: premium > enhanced > default, then the modern
/// catalog, then identifier order so the choice is stable across launches.
func bestVoice(language: String = "en-US") -> AVSpeechSynthesisVoice? {
    AVSpeechSynthesisVoice.speechVoices()
        .filter { $0.language == language }
        .sorted { a, b in
            let ka = (a.quality.rawValue, familyRank(a.identifier))
            let kb = (b.quality.rawValue, familyRank(b.identifier))
            return ka == kb ? a.identifier < b.identifier : ka > kb
        }
        .first
}

/// Utterance-level synthesis. `Speaker` is the only implementation today; a
/// neural backend would own its own playback and advertise its own catalog.
@MainActor
protocol SpeechBackend: AnyObject {
    var speaking: Bool { get }
    var defaultVoiceIdentifier: String? { get }
    var onStateChange: @MainActor () -> Void { get set }
    func voices() -> [VoiceInfo]
    func speak(id: String, text: String)
    func setVoice(_ identifier: String)
    func cancel()
}

@MainActor
final class Speaker: NSObject, SpeechBackend, AVSpeechSynthesizerDelegate {
    private let synth = AVSpeechSynthesizer()
    private var voice: AVSpeechSynthesisVoice? = bestVoice()

    /// The queue is ours, not the synthesiser's: only one utterance is ever
    /// handed over at a time. `stopSpeaking` is a no-op on an utterance that
    /// has not started yet, so owning the queue is what makes `cancel`
    /// reliable immediately after `speak`.
    private var queued = SpeechQueue()
    private var current: (id: String, key: ObjectIdentifier)?
    private var cancelCurrent = false

    private(set) var speaking = false
    var onStateChange: @MainActor () -> Void = {}

    override init() {
        super.init()
        synth.delegate = self
    }

    var defaultVoiceIdentifier: String? { voice?.identifier }

    func voices() -> [VoiceInfo] {
        AVSpeechSynthesisVoice.speechVoices().map {
            VoiceInfo(id: $0.identifier, name: $0.name,
                      quality: qualityName($0.quality), language: $0.language)
        }
    }

    func setVoice(_ identifier: String) {
        guard let v = AVSpeechSynthesisVoice(identifier: identifier) else {
            emit(ErrorMsg(message: "unknown or uninstalled voice: \(identifier)", fatal: false))
            return
        }
        voice = v
    }

    func speak(id: String, text: String) {
        guard queued.append(SpeechRequest(id: id, text: text)) else {
            emit(ErrorMsg(message: "speech backlog limit exceeded", fatal: false))
            emit(SpeechMsg(type: "speechCancelled", id: id))
            return
        }
        syncState()
        dispatchNext()
    }

    /// Drops everything queued and stops the utterance in flight.
    func cancel() {
        while let dropped = queued.popFirst() {
            emit(SpeechMsg(type: "speechCancelled", id: dropped.id))
        }
        if current != nil {
            cancelCurrent = true
            // Returns false when the utterance has not started; `didStart`
            // stops it then.
            _ = synth.stopSpeaking(at: .immediate)
        }
        syncState()
    }

    private func dispatchNext() {
        guard current == nil, let next = queued.popFirst() else { return }
        let utterance = AVSpeechUtterance(string: next.text)
        utterance.voice = voice
        current = (next.id, ObjectIdentifier(utterance))
        cancelCurrent = false
        synth.speak(utterance)
    }

    private func syncState() {
        let nowSpeaking = current != nil || !queued.isEmpty
        guard nowSpeaking != speaking else { return }
        speaking = nowSpeaking
        onStateChange()
    }

    private func started(_ key: ObjectIdentifier) {
        guard let current, current.key == key else { return }
        guard !cancelCurrent else {
            _ = synth.stopSpeaking(at: .immediate)
            return
        }
        emit(SpeechMsg(type: "speechStarted", id: current.id))
    }

    private func ended(_ key: ObjectIdentifier, cancelled: Bool) {
        guard let finished = current, finished.key == key else { return }
        let wasCancelled = cancelled || cancelCurrent
        current = nil
        cancelCurrent = false
        emit(SpeechMsg(type: wasCancelled ? "speechCancelled" : "speechFinished", id: finished.id))
        dispatchNext()
        syncState()
    }

    // MARK: - Delegate (called off the main actor)

    nonisolated func speechSynthesizer(_ s: AVSpeechSynthesizer, didStart u: AVSpeechUtterance) {
        let key = ObjectIdentifier(u)
        Task { @MainActor in self.started(key) }
    }

    nonisolated func speechSynthesizer(_ s: AVSpeechSynthesizer, didFinish u: AVSpeechUtterance) {
        let key = ObjectIdentifier(u)
        Task { @MainActor in self.ended(key, cancelled: false) }
    }

    nonisolated func speechSynthesizer(_ s: AVSpeechSynthesizer, didCancel u: AVSpeechUtterance) {
        let key = ObjectIdentifier(u)
        Task { @MainActor in self.ended(key, cancelled: true) }
    }
}
