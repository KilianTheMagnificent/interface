import Foundation

/// Framing is bounded before JSON decoding. An oversized command is fatal;
/// silently dropping a cancel or shutdown would violate command ordering.
struct VoiceLineFramer {
    static let maxBytes = 1024 * 1024
    enum Failure: Error { case oversized }
    private var pending = Data()

    mutating func push(_ data: Data) throws -> [Data] {
        var lines: [Data] = []
        for byte in data {
            if byte == 0x0A {
                if !pending.isEmpty { lines.append(pending) }
                pending = Data()
            } else {
                guard pending.count < Self.maxBytes else { throw Failure.oversized }
                pending.append(byte)
            }
        }
        return lines
    }

    mutating func finish() -> Data? {
        defer { pending = Data() }
        return pending.isEmpty ? nil : pending
    }
}

struct SpeechRequest {
    let id: String
    let text: String
    var bytes: Int { id.utf8.count + text.utf8.count }
}

/// An explicit FIFO budget independent of synthesizer speed. Reject new work
/// on overflow; accepted speech retains its order and cancellation semantics.
struct SpeechQueue {
    static let maxCount = 128
    static let maxBytes = 256 * 1024
    static let maxTextBytes = 16 * 1024
    private var slots = [SpeechRequest?](repeating: nil, count: maxCount)
    private var head = 0
    private(set) var count = 0
    private(set) var bytes = 0
    var isEmpty: Bool { count == 0 }

    mutating func append(_ request: SpeechRequest) -> Bool {
        guard count < Self.maxCount, request.text.utf8.count <= Self.maxTextBytes,
              request.bytes <= Self.maxBytes - bytes else { return false }
        slots[(head + count) % Self.maxCount] = request
        count += 1
        bytes += request.bytes
        return true
    }

    mutating func popFirst() -> SpeechRequest? {
        guard count > 0, let request = slots[head] else { return nil }
        slots[head] = nil
        head = (head + 1) % Self.maxCount
        count -= 1
        bytes -= request.bytes
        return request
    }
}
