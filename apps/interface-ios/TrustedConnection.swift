import Foundation
@preconcurrency import LocalAuthentication
import Security
import WebKit

// Only this Keychain payload contains the durable credential. Never log or put it in a URL.
struct DeviceCredential: Codable {
    let id: String
    let secret: String
    static func create() throws -> DeviceCredential {
        var bytes = [UInt8](repeating: 0, count: 32)
        let randomStatus = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        guard randomStatus == errSecSuccess else {
            throw NSError(domain: "InterfaceRandom", code: Int(randomStatus), userInfo: [NSLocalizedDescriptionKey: "Secure random generation was unavailable."])
        }
        return DeviceCredential(id: UUID().uuidString.lowercased(), secret: ConnectionCrypto.hex(Data(bytes)))
    }
}
enum PhoneError: LocalizedError {
    case storage, locked, unavailable, refused, protocolVersion, expired, identityMissing
    var errorDescription: String? {
        switch self {
        case .storage: return "The secure connection could not be saved. Set a device passcode and try again."
        case .locked: return "Interface is locked. Use Face ID or your device passcode to continue."
        case .unavailable: return "Your Mac could not be reached. Keep Interface open and your Mac awake, then try again."
        case .refused: return "This connection is no longer approved. Review trusted iPhones in Interface Settings on your Mac."
        case .protocolVersion: return "Update Interface on your Mac to use trusted iPhone access."
        case .expired: return "The approval request expired or was declined. Request a new connection when your Mac is ready."
        case .identityMissing: return "This iPhone needs its own approval. Scan a new pairing code from Interface Settings on your Mac."
        }
    }
}

enum DeviceKeychain {
    private static func query(_ origin: URL) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: "com.interface.phone.trusted.v1",
         kSecAttrAccount as String: origin.absoluteString]
    }
    static func save(_ value: DeviceCredential, origin: URL) throws {
        var failure: Unmanaged<CFError>?
        guard let access = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly,
                                                         .userPresence, &failure) else { throw PhoneError.storage }
        var item = query(origin)
        item[kSecAttrAccessControl as String] = access
        item[kSecValueData as String] = try JSONEncoder().encode(value)
        // Do not silently replace an existing trusted phone identity.
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else { throw PhoneError.storage }
    }
    static func read(origin: URL, context: LAContext) async throws -> DeviceCredential {
        // Explicit authentication also makes Simulator behavior faithful: its Keychain
        // does not enforce the device hardware ACL. On iPhone, the same context must
        // additionally satisfy the Keychain item's userPresence access control.
        do {
            guard try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: "Unlock your Interface sessions") else { throw PhoneError.locked }
        } catch { throw PhoneError.locked }
        return try await lookup(origin: origin, context: context)
    }
    /// Only an absent item is `identityMissing`: a restored or transferred iPhone keeps
    /// the app's preferences but never this device-only item. Anything else stays locked.
    static func lookup(origin: URL, context: LAContext) async throws -> DeviceCredential {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                var request = query(origin)
                request[kSecReturnData as String] = true
                request[kSecMatchLimit as String] = kSecMatchLimitOne
                request[kSecUseAuthenticationContext as String] = context
                var result: CFTypeRef?
                let status = SecItemCopyMatching(request as CFDictionary, &result)
                guard status == errSecSuccess, let data = result as? Data,
                      let value = try? JSONDecoder().decode(DeviceCredential.self, from: data) else {
                    continuation.resume(throwing: status == errSecItemNotFound ? PhoneError.identityMissing : PhoneError.locked)
                    return
                }
                continuation.resume(returning: value)
            }
        }
    }
    static func forget(origin: URL) throws {
        let status = SecItemDelete(query(origin) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw PhoneError.storage }
    }
}

struct PhoneWorkspaceSnapshot: Decodable { let version: Int; let workspaces: [PhoneWorkspace] }
/// Roster v3 adds the project the Mac sidebar files each terminal under; null is "Other".
/// Roster v4 has the same shape, and its terminals carry the host's `attention`.
struct PhoneWorkspaceSnapshotV3: Decodable {
    let version: Int; let workspaces: [PhoneWorkspace]; let projects: [String: String?]
    var filed: [PhoneWorkspace] {
        workspaces.map { workspace in
            PhoneWorkspace(id: workspace.id, name: workspace.name, root: workspace.root, terminals: workspace.terminals.map { terminal in
                var terminal = terminal
                if let project = projects[terminal.id] { terminal.project = project.map(PhoneProjectKey.folder) ?? .other }
                return terminal
            })
        }
    }
}
enum PhoneProjectKey: Hashable { case folder(String), other }
struct PhoneWorkspace: Decodable, Identifiable, Equatable {
    let id: String; let name: String; let root: String; let terminals: [PhoneTerminal]
}
struct PhoneTerminal: Decodable, Identifiable, Hashable {
    let id: String; let generation: UInt64; let title: String; let cwd: String; let state: String
    let launch: Launch?
    /// The folder Rust authorized for this session; its project when the Mac sends none.
    var root: String? = nil
    var updatedAt: String? = nil
    /// The Mac sidebar's project for this session (roster v3); never decoded from a terminal.
    var project: PhoneProjectKey? = nil
    /// Present while the session needs you (roster v4; Rust decides it).
    var attention: PhoneAttention? = nil
    private enum CodingKeys: String, CodingKey { case id, generation, title, cwd, state, launch, root, updatedAt, attention }
    struct Launch: Decodable, Hashable { let profileId: String }
    var provider: String { launch?.profileId.capitalized ?? "Terminal" }
    var displayTitle: String { title.isEmpty ? "Untitled session" : title }
    var conversationSupported: Bool { launch?.profileId == "claude" || launch?.profileId == "codex" }
    var needsYou: Bool { attention != nil && state == "running" }
}
struct EnrollmentReply: Decodable { let version: Int; let approved: Bool; let verification: String }
struct UnlockReply: Decodable { let version: Int; let expiresIn: Int }

// Redirects are refused, including same-site redirects: no credential POST may change its destination.
final class RefuseRedirects: NSObject, URLSessionTaskDelegate {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}
final class MacClient {
    let origin: URL
    let session: URLSession
    let direct: DirectTransport?
    let invitationId: String?
    private let macId: String?
    /// The pinned Mac this client speaks to, for markers scoped to it.
    var connectionMacId: String? { macId }
    init(origin: URL, connection: SavedMacConnection? = nil, invitationId: String? = nil,
         nearbyOrigins: [String]? = nil, discover: @escaping (String) async -> URL? = NearbyMacDiscovery.find,
         endpoint: URL? = nil) {
        self.origin = origin
        self.invitationId = invitationId
        let config = URLSessionConfiguration.ephemeral
        config.httpCookieStorage = nil; config.httpShouldSetCookies = false; config.urlCache = nil
        config.timeoutIntervalForRequest = 10; config.timeoutIntervalForResource = 15
        let transportSession = URLSession(configuration: config, delegate: RefuseRedirects(), delegateQueue: nil)
        session = transportSession
        direct = connection.map { DirectTransport(connection: $0, session: transportSession,
            nearbyOrigins: nearbyOrigins ?? MacRoutesV1.merged(macId: $0.macId), discover: discover, endpoint: endpoint) }
        macId = connection?.macId
    }
    func cancel() { session.invalidateAndCancel() }
    func request(_ path: String, body: [String: Any]? = nil, cookie: HTTPCookie? = nil) async throws -> (Data, HTTPURLResponse) {
        if let direct {
            let (data, response) = try await direct.request(path: "/" + path, method: body == nil ? "GET" : "POST",
                body: try body.map { try JSONSerialization.data(withJSONObject: $0) } ?? Data(), cookie: cookie, invitationId: invitationId)
            guard response.statusCode != 401 else { throw PhoneError.refused }
            guard (200..<300).contains(response.statusCode) else {
                let code = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["code"] as? String
                if code == "invitation-expired" { throw PhoneError.expired }
                // An older Mac refuses a request it does not know this way.
                if code == "invalid-request" { throw PhoneError.protocolVersion }
                throw PhoneError.unavailable
            }
            return (data, response)
        }
        let url = origin.appendingPathComponent(path)
        guard MacAddress.contains(url, origin: origin) else { throw PhoneError.refused }
        var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData)
        request.setValue(origin.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/")), forHTTPHeaderField: "Origin")
        if let body {
            request.httpMethod = "POST"; request.httpBody = try JSONSerialization.data(withJSONObject: body)
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        if let cookie { request.setValue("\(cookie.name)=\(cookie.value)", forHTTPHeaderField: "Cookie") }
        do {
            let (data, response) = try await BoundedBody.read(session, request, cap: 2 * 1024 * 1024)
            guard let response = response as? HTTPURLResponse else { throw PhoneError.unavailable }
            guard response.statusCode != 401 else { throw PhoneError.refused }
            guard (200..<300).contains(response.statusCode) else {
                throw response.statusCode == 404 ? PhoneError.protocolVersion : PhoneError.unavailable
            }
            try Task.checkCancellation()
            return (data, response)
        } catch let error as PhoneError { throw error }
          catch is CancellationError { throw CancellationError() }
          catch { throw PhoneError.unavailable }
    }
    /// The lock's best-effort grant revoke. A direct route resends it only over a newly
    /// discovered route, and only when the held one provably never received it.
    func revoke(cookie: HTTPCookie) async {
        guard let direct else {
            _ = try? await request("api/v3/lock", body: [:], cookie: cookie)
            return
        }
        _ = try? await direct.requestOnce(path: "/api/v3/lock", method: "POST", body: Data("{}".utf8), cookie: cookie)
    }
    func enrollment(_ credential: DeviceCredential, create: Bool) async throws -> EnrollmentReply {
        var body: [String: Any] = ["version": 3, "id": credential.id, "secret": credential.secret]
        if create { body["name"] = "Interface on iPhone" }
        let (data, _) = try await request(create ? "api/v3/enrollment" : "api/v3/enrollment/status", body: body)
        let reply = try JSONDecoder().decode(EnrollmentReply.self, from: data)
        guard reply.version == 3 else { throw PhoneError.protocolVersion }
        return reply
    }
    func unlock(_ credential: DeviceCredential) async throws -> HTTPCookie {
        let (data, response) = try await request("api/v3/unlock", body: ["version": 3, "id": credential.id, "secret": credential.secret])
        let reply = try JSONDecoder().decode(UnlockReply.self, from: data)
        guard reply.version == 3 && reply.expiresIn == 900 else { throw PhoneError.protocolVersion }
        let fields = response.allHeaderFields.reduce(into: [String: String]()) { if let key = $1.key as? String, let value = $1.value as? String { $0[key] = value } }
        guard let cookie = HTTPCookie.cookies(withResponseHeaderFields: fields, for: origin).first(where: {
            $0.name == "interface_remote" && $0.path == "/" && $0.domain == origin.host && $0.value.count == 32 && $0.isHTTPOnly
        }), origin.scheme != "https" || cookie.isSecure else { throw PhoneError.refused }
        return cookie
    }
    /// Saves the Mac's current nearby addresses for the next
    /// connection. Best effort: a Mac without a route list keeps the QR's.
    func refreshRoutes(cookie: HTTPCookie) async {
        guard let macId, direct != nil,
              let (data, _) = try? await request("api/v1/routes", cookie: cookie),
              let routes = try? JSONDecoder().decode(MacRouteList.self, from: data), routes.version == 1 else { return }
        MacRoutesV1.save(macId: macId, origins: routes.origins)
    }
    /// The newest roster this Mac answers: 4 (needs you), then 3 (projects), then 2.
    private var roster = 4
    func workspaces(cookie: HTTPCookie) async throws -> [PhoneWorkspace] {
        while roster >= 3 {
            let version = roster
            do {
                let (data, _) = try await request("api/v\(version)/workspaces", cookie: cookie)
                let snapshot = try JSONDecoder().decode(PhoneWorkspaceSnapshotV3.self, from: data)
                guard snapshot.version == version else { throw PhoneError.protocolVersion }
                return snapshot.filed
            } catch PhoneError.protocolVersion {
                // An older Mac: v3 has no needs-you; v2 groups sessions by their own folder.
                roster = version - 1
            }
        }
        let (data, _) = try await request("api/v2/workspaces", cookie: cookie)
        let snapshot = try JSONDecoder().decode(PhoneWorkspaceSnapshot.self, from: data)
        guard snapshot.version == 2 else { throw PhoneError.protocolVersion }
        return snapshot.workspaces
    }
}
