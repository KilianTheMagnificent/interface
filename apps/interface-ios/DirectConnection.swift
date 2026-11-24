import Foundation
import CryptoKit

struct SavedMacConnection: Codable, Equatable {
    let version: Int
    let macId: String
    let macName: String
    let profile: String
    let publicKey: String
    let relayOrigin: String?
    init(_ invitation: PairingInvitation) {
        version = 5; macId = invitation.macId; macName = invitation.macName
        profile = invitation.profile; publicKey = invitation.publicKey; relayOrigin = invitation.relayOrigin
    }
    var virtualOrigin: URL { URL(string: "https://\(macId).interface.invalid/")! }
    var valid: Bool {
        guard version == 5, ["production", "development"].contains(profile),
              !macName.isEmpty, macName.utf8.count <= 80,
              relayOrigin == nil || PairingInvitation.relayURL(relayOrigin!) != nil else { return false }
        return ConnectionCrypto.identity(publicKey: publicKey, matches: macId)
    }
    static func load() -> SavedMacConnection? {
        guard let data = UserDefaults.standard.data(forKey: "interface.macConnection.v5"), data.count <= 2048,
              let value = try? JSONDecoder().decode(Self.self, from: data), value.valid else { return nil }
        return value
    }
}

// Public, untrusted route hints live separately from the frozen v5 identity.
// A changed network may require a fresh QR; a route never changes the pinned Mac.
struct NearbyRoutesV1: Codable {
    let version: Int
    let macId: String
    let origins: [String]
    private static let storageKey = "interface.nearbyRoutes.v1"
    static func save(macId: String, origins: [String], defaults: UserDefaults = .standard) {
        guard PairingInvitation.validNearbyOrigins(origins) else { return }
        let value = Self(version: 1, macId: macId, origins: origins)
        defaults.set(try? JSONEncoder().encode(value), forKey: storageKey)
    }
    static func load(macId: String, defaults: UserDefaults = .standard) -> [String] {
        guard let data = defaults.data(forKey: storageKey), data.count <= 1024,
              let value = try? JSONDecoder().decode(Self.self, from: data),
              value.version == 1, value.macId == macId,
              PairingInvitation.validNearbyOrigins(value.origins) else { return [] }
        return value.origins
    }
}

/// Nearby routes the paired Mac sent over the authenticated connection after
/// unlock. Refreshed each unlock, so an address change never needs a new QR;
/// every route still authenticates the pinned Mac. A list saved from a Mac
/// that still sent its tailnet address is not used; the next unlock replaces it.
struct MacRoutesV1: Codable {
    let version: Int
    let macId: String
    let origins: [String]
    private static let storageKey = "interface.macRoutes.v1"
    static func save(macId: String, origins: [String], defaults: UserDefaults = .standard) {
        guard PairingInvitation.validNearbyOrigins(origins) else { return }
        defaults.set(try? JSONEncoder().encode(Self(version: 1, macId: macId, origins: origins)), forKey: storageKey)
    }
    static func load(macId: String, defaults: UserDefaults = .standard) -> [String] {
        guard let data = defaults.data(forKey: storageKey), data.count <= 1024,
              let value = try? JSONDecoder().decode(Self.self, from: data),
              value.version == 1, value.macId == macId,
              PairingInvitation.validNearbyOrigins(value.origins) else { return [] }
        return value.origins
    }
    /// The Mac's latest routes first, then the QR's, at most eight.
    static func merged(macId: String, defaults: UserDefaults = .standard) -> [String] {
        var seen = Set<String>()
        return Array((load(macId: macId, defaults: defaults) + NearbyRoutesV1.load(macId: macId, defaults: defaults))
            .filter { seen.insert($0).inserted }.prefix(8))
    }
}
struct MacRouteList: Decodable { let version: Int; let origins: [String] }

struct EncryptedPacket: Codable {
    let version: Int
    let macId: String
    let requestId: String
    let senderKey: String
    let encapsulatedKey: String
    let ciphertext: String
}
struct TunnelRequest: Encodable {
    let version = 5
    let issuedAt: UInt64
    let expiresAt: UInt64
    let method: String
    let path: String
    let body: String
    let cookie: String?
    let control: String?
    let command: String?
    let invitationId: String?
    private enum CodingKeys: String, CodingKey { case version, issuedAt, expiresAt, method, path, body, cookie, control, command, invitationId }
    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(version, forKey: .version)
        try values.encode(issuedAt, forKey: .issuedAt)
        try values.encode(expiresAt, forKey: .expiresAt)
        try values.encode(method, forKey: .method)
        try values.encode(path, forKey: .path)
        try values.encode(body, forKey: .body)
        try values.encode(cookie, forKey: .cookie)
        try values.encode(control, forKey: .control)
        try values.encode(command, forKey: .command)
        try values.encode(invitationId, forKey: .invitationId)
    }
}
struct TunnelResponse: Codable {
    let version: Int
    let status: Int
    let body: String
    let contentType: String
    let setCookie: String?
}

enum ConnectionCrypto {
    static func hex(_ bytes: Data) -> String { bytes.map { String(format: "%02x", $0) }.joined() }
    static func unhex(_ text: String, count: Int) throws -> Data {
        let bytes = Array(text.utf8)
        guard bytes.count == count * 2, bytes.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else { throw PhoneError.refused }
        func nibble(_ b: UInt8) -> UInt8 { b <= 57 ? b - 48 : b - 87 }
        return Data(stride(from: 0, to: bytes.count, by: 2).map { nibble(bytes[$0]) * 16 + nibble(bytes[$0 + 1]) })
    }
    /// The pinned Mac identity: `macId` is the SHA-256 of this valid P-256 public key.
    static func identity(publicKey: String, matches macId: String) -> Bool {
        guard let bytes = try? unhex(publicKey, count: 65),
              (try? P256.KeyAgreement.PublicKey(x963Representation: bytes)) != nil else { return false }
        return hex(Data(SHA256.hash(data: bytes))) == macId
    }
    private static func context(mac: String, request: String, direction: String, sender: String) -> Data {
        Data("Interface connection v5|\(direction)|\(mac)|\(request)|\(sender)".utf8)
    }
    static func seal(_ plaintext: Data, key: P256.KeyAgreement.PrivateKey, recipient: String, macId: String, requestId: String, direction: String = "request") throws -> EncryptedPacket {
        guard plaintext.count <= 3 * 1024 * 1024 else { throw PhoneError.unavailable }
        let publicKey = try P256.KeyAgreement.PublicKey(x963Representation: unhex(recipient, count: 65))
        let senderKey = hex(key.publicKey.x963Representation)
        let info = context(mac: macId, request: requestId, direction: direction, sender: senderKey)
        var sender = try HPKE.Sender(recipientKey: publicKey, ciphersuite: .P256_SHA256_AES_GCM_256,
                                     info: info, authenticatedBy: key)
        let ciphertext = try sender.seal(plaintext, authenticating: info)
        return EncryptedPacket(version: 5, macId: macId, requestId: requestId, senderKey: senderKey,
                               encapsulatedKey: hex(sender.encapsulatedKey), ciphertext: ciphertext.base64EncodedString())
    }
    static func open(_ packet: EncryptedPacket, key: P256.KeyAgreement.PrivateKey, macId: String, requestId: String, expectedSender: String, direction: String = "response") throws -> Data {
        guard packet.version == 5, packet.macId == macId, packet.requestId == requestId,
              packet.senderKey == expectedSender, UUID(uuidString: requestId)?.uuidString.lowercased() == requestId,
              packet.ciphertext.utf8.count <= 4 * 1024 * 1024 - 1024,
              let ciphertext = Data(base64Encoded: packet.ciphertext), ciphertext.count <= 3 * 1024 * 1024 + 16 else { throw PhoneError.refused }
        let sender = try P256.KeyAgreement.PublicKey(x963Representation: unhex(expectedSender, count: 65))
        let info = context(mac: macId, request: requestId, direction: direction, sender: expectedSender)
        var recipient = try HPKE.Recipient(privateKey: key, ciphersuite: .P256_SHA256_AES_GCM_256, info: info,
                                          encapsulatedKey: unhex(packet.encapsulatedKey, count: 65), authenticatedBy: sender)
        return try recipient.open(ciphertext, authenticating: info)
    }
}

actor DirectTransport {
    let connection: SavedMacConnection
    let session: URLSession
    private var endpoint: URL?
    private var resolving: Task<URL, Error>?
    private let nearbyOrigins: [String]
    private let discover: (String) async -> URL?
    private let probe: ((URL) async -> Bool)?
    /// Nearby probes in flight at once, beside Bonjour's. Every route shares the
    /// Mac's small request perimeter, so a burst must not refuse its own probes.
    static let nearbyProbeWidth = 3
    /// `endpoint` seeds a route another transport to the same Mac already proved;
    /// every request over it is still sealed to the pinned key.
    init(connection: SavedMacConnection, session: URLSession, nearbyOrigins: [String],
         discover: @escaping (String) async -> URL? = NearbyMacDiscovery.find,
         probe: ((URL) async -> Bool)? = nil, endpoint: URL? = nil) {
        self.connection = connection; self.session = session
        self.nearbyOrigins = PairingInvitation.validNearbyOrigins(nearbyOrigins) ? nearbyOrigins : []
        self.discover = discover
        self.probe = probe
        self.endpoint = endpoint
    }
    /// The route the last request proved, if it has not failed since.
    var currentEndpoint: URL? { endpoint }
    var routeName: String? {
        endpoint.map { $0.scheme == "https" ? "Connected through relay" : "Connected nearby" }
    }

    func request(path: String, method: String, body: Data, cookie: HTTPCookie?, control: String? = nil, command: String? = nil, invitationId: String? = nil) async throws -> (Data, HTTPURLResponse) {
        let (encoded, response) = try await requestEncoded(path: path, method: method, body: body, cookie: cookie,
                                                           control: control, command: command, invitationId: invitationId)
        guard let data = Data(base64Encoded: encoded) else { throw PhoneError.protocolVersion }
        return (data, response)
    }
    /// The reply body still base64-encoded, as the tunnel carried it. The session
    /// web view hands it to JavaScript unchanged instead of decoding and
    /// re-encoding up to 2 MiB per poll; its decoded size is bounded here.
    func requestEncoded(path: String, method: String, body: Data, cookie: HTTPCookie?, control: String? = nil, command: String? = nil, invitationId: String? = nil) async throws -> (String, HTTPURLResponse) {
        guard body.count <= 16384 else { throw PhoneError.refused }
        let endpoint = try await selectEndpoint()
        let reply: TunnelResponse
        do {
            reply = try await exchange(endpoint: endpoint, path: path, method: method, body: body, cookie: cookie,
                                       control: control, command: command, invitationId: invitationId)
        } catch {
            self.endpoint = nil
            // A failed exchange might already have reached the Mac. No write replay.
            throw error
        }
        // The reply opened under the pinned key, so this route works; a malformed
        // reply is the Mac's answer and does not reset the endpoint.
        guard reply.version == 5, (100...599).contains(reply.status),
              let size = Self.decodedLength(reply.body), size <= 2 * 1024 * 1024 else { throw PhoneError.protocolVersion }
        var headers = ["Content-Type": reply.contentType]
        if let cookie = reply.setCookie { headers["Set-Cookie"] = cookie }
        guard let response = HTTPURLResponse(url: connection.virtualOrigin, statusCode: reply.status, httpVersion: "HTTP/1.1", headerFields: headers) else { throw PhoneError.protocolVersion }
        return (reply.body, response)
    }
    /// Sends a request that must reach the Mac at most once, such as the lock's revoke.
    /// When the route this transport already held provably never received it, that route
    /// is forgotten and the request is sent once more over a route found exactly as a new
    /// transport finds one. Any other failure might have been delivered and is not resent.
    func requestOnce(path: String, method: String, body: Data, cookie: HTTPCookie?) async throws -> (Data, HTTPURLResponse) {
        let held = endpoint != nil
        do {
            return try await request(path: path, method: method, body: body, cookie: cookie)
        } catch where held && Self.neverSent(error) {
            return try await request(path: path, method: method, body: body, cookie: cookie)
        }
    }
    /// Whether a failed exchange provably never reached a Mac: the connection was refused
    /// or had no route, or TLS failed, before a byte of the request was written. A timeout
    /// or a lost connection may follow delivery, so neither counts.
    static func neverSent(_ error: Error) -> Bool {
        guard let error = error as? URLError else { return false }
        switch error.code {
        case .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed, .notConnectedToInternet,
             .internationalRoamingOff, .callIsActive, .dataNotAllowed,
             .appTransportSecurityRequiresSecureConnection, .secureConnectionFailed,
             .serverCertificateHasBadDate, .serverCertificateUntrusted, .serverCertificateHasUnknownRoot,
             .serverCertificateNotYetValid, .clientCertificateRejected, .clientCertificateRequired:
            return true
        default:
            return false
        }
    }
    /// Decoded size of padded standard base64, or nil when the length cannot be base64.
    static func decodedLength(_ encoded: String) -> Int? {
        let count = encoded.utf8.count
        guard count % 4 == 0 else { return nil }
        let padding = encoded.utf8.reversed().prefix(2).prefix { $0 == UInt8(ascii: "=") }.count
        return count / 4 * 3 - padding
    }
    func selectEndpoint() async throws -> URL {
        if let endpoint { return endpoint }
        if let resolving { return try await resolving.value }
        let task = Task<URL, Error> {
            if let nearby = await nearbyEndpoint() { return nearby }
            try Task.checkCancellation()
            // The relay is tried only once every nearby route has failed, so a phone on
            // the Mac's network never sends the relay operator anything.
            if let origin = connection.relayOrigin, let relay = PairingInvitation.relayURL(origin), await authenticates(relay) {
                return relay
            }
            throw PhoneError.unavailable
        }
        resolving = task
        defer { resolving = nil }
        let selected = try await task.value
        endpoint = selected
        return selected
    }
    /// Probes the QR routes and the Bonjour hint together and returns the first
    /// that authenticates, cancelling the rest. Off the Mac's network this costs
    /// one probe timeout instead of one per route plus discovery.
    private func nearbyEndpoint() async -> URL? {
        var pending = nearbyOrigins.compactMap(PairingInvitation.nearbyURL)[...]
        let (results, sink) = AsyncStream.makeStream(of: URL?.self)
        var probes: [Task<Void, Never>] = []
        let macId = connection.macId
        let discover = self.discover
        // A read-only encrypted probe authenticates a discovery hint before it
        // becomes the route for an enrollment, credential or session request.
        probes.append(Task {
            guard let hint = await discover(macId), !Task.isCancelled else { sink.yield(nil); return }
            sink.yield(await self.authenticates(hint) ? hint : nil)
        })
        var running = 1
        func startNext() {
            guard let candidate = pending.popFirst() else { return }
            running += 1
            probes.append(Task { sink.yield(await self.authenticates(candidate) ? candidate : nil) })
        }
        for _ in 0..<Self.nearbyProbeWidth { startNext() }
        defer { probes.forEach { $0.cancel() }; sink.finish() }
        for await result in results {
            running -= 1
            if let result { return result }
            startNext()
            if running == 0 { break }
        }
        return nil
    }
    private func authenticates(_ candidate: URL) async -> Bool {
        if let probe { return await probe(candidate) }
        guard let probe = try? await exchange(endpoint: candidate, path: "/api/v5/ping", method: "GET", body: Data(), cookie: nil, timeout: 3) else { return false }
        return probe.version == 5 && probe.status == 200
    }
    private func exchange(endpoint: URL, path: String, method: String, body: Data, cookie: HTTPCookie?, control: String? = nil, command: String? = nil, invitationId: String? = nil, timeout: TimeInterval = 10) async throws -> TunnelResponse {
        let seconds = UInt64(Date().timeIntervalSince1970)
        let request = TunnelRequest(issuedAt: seconds, expiresAt: seconds + 90, method: method, path: path,
                                    body: body.base64EncodedString(), cookie: cookie.map { "\($0.name)=\($0.value)" },
                                    control: control, command: command, invitationId: invitationId)
        let key = P256.KeyAgreement.PrivateKey()
        let id = UUID().uuidString.lowercased()
        let packet = try ConnectionCrypto.seal(JSONEncoder().encode(request), key: key, recipient: connection.publicKey, macId: connection.macId, requestId: id)
        var http = URLRequest(url: endpoint.appendingPathComponent("v5/packet"), cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: timeout)
        http.httpMethod = "POST"; http.httpBody = try JSONEncoder().encode(packet)
        http.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let (data, response) = try await BoundedBody.read(session, http, cap: 4 * 1024 * 1024)
        guard let response = response as? HTTPURLResponse, response.statusCode == 200 else { throw PhoneError.unavailable }
        try Task.checkCancellation()
        let received = try JSONDecoder().decode(EncryptedPacket.self, from: data)
        let plaintext = try ConnectionCrypto.open(received, key: key, macId: connection.macId, requestId: id, expectedSender: connection.publicKey)
        return try JSONDecoder().decode(TunnelResponse.self, from: plaintext)
    }
}

/// Reads one response body in whole chunks and cancels the request as soon as
/// it would exceed `cap`. `URLSession.bytes` hands over one byte per async
/// step, which made multi-megabyte history replies the slowest part of a poll.
final class BoundedBody: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    private let cap: Int
    private let lock = NSLock()
    private var body = Data()
    private var response: URLResponse?
    private var exceeded = false
    private var continuation: CheckedContinuation<(Data, URLResponse), Error>?

    private init(cap: Int) { self.cap = cap }

    static func read(_ session: URLSession, _ request: URLRequest, cap: Int) async throws -> (Data, URLResponse) {
        let reader = BoundedBody(cap: cap)
        let task = session.dataTask(with: request)
        task.delegate = reader
        do {
            return try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { continuation in
                    reader.lock.withLock { reader.continuation = continuation }
                    task.resume()
                }
            } onCancel: { task.cancel() }
        } catch {
            if Task.isCancelled { throw CancellationError() }
            throw error
        }
    }
    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse,
                    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        lock.withLock { self.response = response }
        completionHandler(.allow)
    }
    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        let over = lock.withLock { () -> Bool in
            guard !exceeded else { return false }
            guard body.count + data.count <= cap else { exceeded = true; body = Data(); return true }
            body.append(data)
            return false
        }
        if over { dataTask.cancel() }
    }
    // Redirects are refused here too, exactly as the session's RefuseRedirects does.
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        let (continuation, result) = lock.withLock { () -> (CheckedContinuation<(Data, URLResponse), Error>?, Result<(Data, URLResponse), Error>) in
            let waiting = self.continuation
            self.continuation = nil
            if exceeded { return (waiting, .failure(PhoneError.unavailable)) }
            if let error { return (waiting, .failure(error)) }
            guard let response else { return (waiting, .failure(PhoneError.unavailable)) }
            return (waiting, .success((body, response)))
        }
        continuation?.resume(with: result)
    }
}
