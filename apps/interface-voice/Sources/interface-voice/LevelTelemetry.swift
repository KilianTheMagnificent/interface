import Foundation
import Synchronization

/// A single atomic slot: the audio render thread never waits for protocol I/O,
/// encodes JSON, or allocates a task. A slow consumer observes the latest meter
/// sample instead of accumulating stale samples behind transcript messages.
final class LevelMailbox: Sendable {
    private let bits = Atomic<UInt64>(Double.nan.bitPattern)
    private let conversionFailed = Atomic<Bool>(false)
    func publish(_ rms: Double) { bits.store(rms.bitPattern, ordering: .relaxed) }
    func take() -> Double? {
        let value = Double(bitPattern: bits.exchange(Double.nan.bitPattern, ordering: .relaxed))
        return value.isFinite ? value : nil
    }
    func reportConversionFailure() { conversionFailed.store(true, ordering: .relaxed) }
    func takeConversionFailure() -> Bool { conversionFailed.exchange(false, ordering: .relaxed) }
}

final class LevelTelemetry: @unchecked Sendable {
    let mailbox = LevelMailbox()
    private let timer: DispatchSourceTimer
    init() {
        let mailbox = self.mailbox
        timer = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "interface-voice.level"))
        timer.schedule(deadline: .now() + .milliseconds(100), repeating: .milliseconds(100))
        timer.setEventHandler {
            if mailbox.takeConversionFailure() { log("audio conversion failed") }
            if let rms = mailbox.take() { emit(LevelMsg(rms: rms)) }
        }
        timer.resume()
    }
    deinit { timer.cancel() }
}
