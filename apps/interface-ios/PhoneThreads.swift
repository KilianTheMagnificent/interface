import Foundation

// Phone Threads v1 and roster v4 (review D6). The Mac's Rust host decides what
// needs you, the Threads order and whether an approval can still be answered;
// this file only decodes its answers and carries one deliberate effect.
// Shared fixture: electron/src/fixtures/remote-phone-v4.json.

/// Host protocol v44's `TerminalSummary.attention`: the session needs you.
struct PhoneAttention: Decodable, Hashable { let version: Int; let since: String }

struct PhoneThread: Decodable, Identifiable, Hashable {
    let key: String
    let engine: String
    let sessionId: String
    let title: String
    let project: String?
    /// `needsYou`, `working`, `idle` or `failed`.
    let status: String
    /// The Mac's Threads section: `pinned`, `active`, `snoozed` or `settled`.
    let section: String
    let unread: Bool
    let lastActivity: String
    var id: String { key }
    var needsYou: Bool { status == "needsYou" }
    var working: Bool { status == "working" }
    /// The phone addresses a thread by its two halves; neither may carry a separator.
    var path: String? {
        let allowedEngine = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyz0123456789:-")
        let allowedSession = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-")
        guard !engine.isEmpty, engine.count <= 32, !sessionId.isEmpty, sessionId.count <= 128,
              engine.unicodeScalars.allSatisfy(allowedEngine.contains),
              sessionId.unicodeScalars.allSatisfy(allowedSession.contains),
              key == "\(engine)#\(sessionId)" else { return nil }
        return "api/v1/threads/\(engine)/\(sessionId)"
    }
    /// The terminal profile whose mark the row shows.
    var profileId: String { engine.hasPrefix("acp:") ? String(engine.dropFirst(4)) : engine }
    var projectName: String? { project?.split(separator: "/").last.map(String.init) }
}

struct PhoneThreadList: Decodable { let version: Int; let threads: [PhoneThread] }
struct PhoneInbox: Decodable, Equatable {
    let version: Int
    let items: [PhoneInboxItem]
    let unavailable: String?
}
struct PhoneInboxItem: Decodable, Identifiable, Hashable {
    let id: String
    let source: String
    let title: String
    let status: String
    let detail: String
    let canSend: Bool
    let agentName: String?
    let thread: PhoneThread?
}
struct PhoneThreadTurn: Decodable, Hashable { let prompt: String; let reply: String; let tools: Int; let clipped: Bool }
struct PhoneThreadApproval: Decodable, Hashable { let id: String; let tool: String; let summary: String; let answerable: Bool }
struct PhoneThreadDetail: Decodable, Equatable {
    let version: Int
    let thread: PhoneThread
    let turns: [PhoneThreadTurn]
    let olderTurns: Int
    let approval: PhoneThreadApproval?
    let unavailable: String?
}

/// Exactly the Rust `RemoteThreadApprovalResponseV1`.
struct PhoneThreadApprovalResponse: Encodable, Equatable {
    let version = 1
    let approvalId: String
    let decision: String
    private enum CodingKeys: String, CodingKey { case version, approvalId, decision }
}

enum PhoneThreadAnswer: Equatable {
    /// The Mac delivered it.
    case sent
    /// The Mac refused it before anything reached the agent.
    case refused(String)
    /// It may have reached the agent. Never sent again from here.
    case unknown
}

/// An answer the phone sent whose outcome it could not learn, kept in the same
/// protected marker file as terminal input. It holds only the opaque approval
/// and command ids, scoped to one Mac and thread, and blocks answering that
/// request again until the Mac shows it is gone or replaced. A store that
/// cannot be read blocks too.
enum ThreadAnswerMarkers {
    private static let separator: Character = "\u{1f}"
    static func key(macId: String, thread: String) -> String { "\(macId)|thread|\(thread)" }
    /// Recorded before the request leaves, so an app or view lost mid-request cannot forget it.
    static func begin(macId: String, thread: String, approvalId: String, command: String) throws {
        try InputMarkers.set(key(macId: macId, thread: thread), command: "\(approvalId)\(separator)\(command)")
    }
    static func clear(macId: String, thread: String) {
        try? InputMarkers.set(key(macId: macId, thread: thread), command: nil)
    }
    /// Whether an uncertain answer to `approvalId` is outstanding. A marker for
    /// any other request, or for none (the Mac shows no request now), is resolved.
    static func blocks(macId: String, thread: String, approvalId: String?) -> Bool {
        switch InputMarkers.value(key(macId: macId, thread: thread)) {
        case .failure: return true
        case .success(nil): return false
        case .success(let marker?):
            if let approvalId, marker.split(separator: separator, maxSplits: 1).first.map(String.init) == approvalId { return true }
            clear(macId: macId, thread: thread)
            return false
        }
    }
}

/// A follow-up the phone sent whose outcome it could not learn: further sends
/// to that thread wait until the Mac shows a newer turn, or the owner says
/// they checked. Only the turn count then and the opaque command id are kept.
enum ThreadSendMarkers {
    private static let separator: Character = "\u{1f}"
    static func key(macId: String, thread: String) -> String { "\(macId)|thread-send|\(thread)" }
    static func begin(macId: String, thread: String, turns: Int, command: String) throws {
        try InputMarkers.set(key(macId: macId, thread: thread), command: "\(turns)\(separator)\(command)")
    }
    static func clear(macId: String, thread: String) {
        try? InputMarkers.set(key(macId: macId, thread: thread), command: nil)
    }
    /// Whether an uncertain send still blocks the next one, given how many
    /// turns the Mac shows now. A store that cannot be read blocks.
    static func blocks(macId: String, thread: String, turns: Int) -> Bool {
        switch InputMarkers.value(key(macId: macId, thread: thread)) {
        case .failure: return true
        case .success(nil): return false
        case .success(let marker?):
            if let sent = marker.split(separator: separator, maxSplits: 1).first.flatMap({ Int($0) }), turns > sent {
                clear(macId: macId, thread: thread)
                return false
            }
            return true
        }
    }
}

extension PhoneThreadDetail {
    /// Every turn the thread has, sent or not: a new one means the Mac moved on.
    var turnCount: Int { turns.count + olderTurns }
}

extension MacClient {
    func inbox(cookie: HTTPCookie) async throws -> PhoneInbox? {
        do {
            let (data, _) = try await request("api/v2/inbox", cookie: cookie)
            let inbox = try JSONDecoder().decode(PhoneInbox.self, from: data)
            guard inbox.version == 2 else { throw PhoneError.protocolVersion }
            return inbox
        } catch PhoneError.protocolVersion { return nil }
    }
    /// One request over the pinned encrypted connection, with the control lease
    /// and command id an effect needs. The Mac's refusal is returned, not thrown.
    func exchange(_ path: String, method: String, body: Data = Data(), cookie: HTTPCookie,
                  control: String? = nil, command: String? = nil) async throws -> (Data, HTTPURLResponse) {
        guard let direct else { throw PhoneError.protocolVersion }
        let (data, response) = try await direct.request(path: "/" + path, method: method, body: body, cookie: cookie,
                                                         control: control, command: command)
        guard response.statusCode != 401 else { throw PhoneError.refused }
        return (data, response)
    }

    /// The Mac's Threads list, or nil from a Mac without phone Threads.
    func threads(cookie: HTTPCookie) async throws -> [PhoneThread]? {
        do {
            let (data, _) = try await request("api/v1/threads", cookie: cookie)
            let list = try JSONDecoder().decode(PhoneThreadList.self, from: data)
            guard list.version == 1 else { throw PhoneError.protocolVersion }
            return list.threads
        } catch PhoneError.protocolVersion { return nil }
    }

    func threadDetail(_ thread: PhoneThread, cookie: HTTPCookie) async throws -> PhoneThreadDetail {
        guard let path = thread.path else { throw PhoneError.protocolVersion }
        let (data, _) = try await request(path, cookie: cookie)
        let detail = try JSONDecoder().decode(PhoneThreadDetail.self, from: data)
        guard detail.version == 1, detail.thread.key == thread.key else { throw PhoneError.protocolVersion }
        return detail
    }

    /// Answers one pending approval: a 30-second control lease taken on this
    /// tap, then one command id the Mac records before it acts. A lost reply
    /// is `unknown` and is never sent again.
    func answer(_ thread: PhoneThread, approvalId: String, allow: Bool, cookie: HTTPCookie) async -> PhoneThreadAnswer {
        guard let path = thread.path, let macId = connectionMacId else { return .refused("Answer this on your Mac.") }
        guard !ThreadAnswerMarkers.blocks(macId: macId, thread: thread.key, approvalId: approvalId) else { return .unknown }
        let body: Data
        do {
            body = try JSONEncoder().encode(PhoneThreadApprovalResponse(approvalId: approvalId, decision: allow ? "allowOnce" : "deny"))
        } catch { return .refused("This answer could not be prepared. Nothing was sent.") }
        return await effect(path: path, kind: "approval", body: body, cookie: cookie,
                            record: { command in try ThreadAnswerMarkers.begin(macId: macId, thread: thread.key, approvalId: approvalId, command: command) },
                            forget: { ThreadAnswerMarkers.clear(macId: macId, thread: thread.key) })
    }

    /// Sends a follow-up to a thread at rest, under the same lease and
    /// command-id rules as an answer. `turns` is how many the phone saw.
    func send(_ thread: PhoneThread, text: String, turns: Int, cookie: HTTPCookie) async -> PhoneThreadAnswer {
        guard let path = thread.path, let macId = connectionMacId else { return .refused("Send this on your Mac.") }
        guard !ThreadSendMarkers.blocks(macId: macId, thread: thread.key, turns: turns) else { return .unknown }
        let body: Data
        do {
            body = try JSONSerialization.data(withJSONObject: ["version": 1, "text": text])
        } catch { return .refused("This message could not be prepared. Nothing was sent.") }
        guard body.count <= 14 * 1024 else { return .refused("This message is too long for the phone.") }
        return await effect(path: path, kind: "send", body: body, cookie: cookie,
                            record: { command in try ThreadSendMarkers.begin(macId: macId, thread: thread.key, turns: turns, command: command) },
                            forget: { ThreadSendMarkers.clear(macId: macId, thread: thread.key) })
    }

    /// One effect: the thread's lease on this tap, a marker recorded before
    /// the request leaves, and the marker kept unless the Mac says it ran or
    /// that nothing ran.
    private func effect(path: String, kind: String, body: Data, cookie: HTTPCookie,
                        record: (String) throws -> Void, forget: () -> Void) async -> PhoneThreadAnswer {
        let lease: String
        do {
            let (data, response) = try await exchange(path + "/control", method: "POST", body: Data("{}".utf8), cookie: cookie)
            guard response.statusCode == 200,
                  let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let value = object["lease"] as? String, UUID(uuidString: value) != nil else {
                return .refused(Self.refusal(data) ?? "Your Mac did not hand this thread to the phone. Try again.")
            }
            lease = value
        } catch { return .refused("Your Mac could not be reached. Nothing was sent.") }
        let command = UUID().uuidString.lowercased()
        do { try record(command) } catch { return .refused("This iPhone could not record it safely. Nothing was sent.") }
        defer { Task { _ = try? await self.exchange(path + "/control", method: "DELETE", cookie: cookie) } }
        do {
            let (data, response) = try await exchange(path + "/" + kind, method: "POST", body: body, cookie: cookie,
                                                      control: lease, command: command)
            if response.statusCode == 202 { forget(); return .sent }
            // Only the Mac's own "nothing ran" lets the phone forget it.
            if let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               object["replaySafe"] as? Bool == true {
                forget()
                return .refused(Self.refusal(data) ?? "Your Mac did not take it.")
            }
            return .unknown
        } catch { return .unknown }
    }

    private static func refusal(_ data: Data) -> String? {
        (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["message"] as? String
    }
}
