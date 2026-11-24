import Foundation

// MARK: - Host -> helper

struct Incoming: Decodable, Sendable {
    let type: String
    var endSilenceMs: Int?
    var locale: String?
    var id: String?
    var text: String?
    var voice: String?
}

// MARK: - Helper -> host

struct VoiceInfo: Encodable {
    let id: String
    let name: String
    /// Omitted by backends whose voices have no quality tiers.
    let quality: String?
    let language: String
}

struct ReadyMsg: Encodable {
    let type = "ready"
    let `protocol` = 1
    let voices: [VoiceInfo]
    let defaultVoice: String?
}

struct PermissionMsg: Encodable {
    let type = "permission"
    let microphone: String
}

struct AssetMsg: Encodable {
    let type = "asset"
    let status: String
    let progress: Double
}

struct StateMsg: Encodable {
    let type = "state"
    let listening: Bool
    let speaking: Bool
}

/// `partial` and `final`.
struct TextMsg: Encodable {
    let type: String
    let text: String
}

/// `speechStarted`, `speechFinished`, `speechCancelled`.
struct SpeechMsg: Encodable {
    let type: String
    let id: String
}

/// `interrupted`.
struct SimpleMsg: Encodable {
    let type: String
}

struct ErrorMsg: Encodable {
    let type = "error"
    let message: String
    let fatal: Bool
}

struct LevelMsg: Encodable {
    let type = "level"
    let rms: Double
}

struct SelfTestMsg: Encodable {
    let type = "selftest"
    let spoken: String
    let heard: String
    let voice: String
    let transcribeMs: Int
}

// MARK: - Output

private let encoder: JSONEncoder = {
    let e = JSONEncoder()
    e.outputFormatting = [.withoutEscapingSlashes]
    return e
}()
private let outLock = NSLock()

/// Writes one JSON object plus a newline to stdout. `FileHandle` writes are
/// unbuffered, so every message reaches the host immediately.
func emit<T: Encodable>(_ message: T) {
    outLock.lock()
    defer { outLock.unlock() }
    guard var data = try? encoder.encode(message) else { return }
    data.append(0x0A)
    try? FileHandle.standardOutput.write(contentsOf: data)
}

/// Diagnostics go to stderr only; stdout is the protocol channel.
func log(_ message: String) {
    try? FileHandle.standardError.write(contentsOf: Data("interface-voice: \(message)\n".utf8))
}
