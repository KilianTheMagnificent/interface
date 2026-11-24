import Foundation

enum PairingInvitationError: LocalizedError {
    case invalid, expired, identity
    var errorDescription: String? {
        switch self {
        case .invalid: return "This is not an Interface pairing code. Open iPhone remote access in Interface Settings on your Mac."
        case .expired: return "This pairing code has expired. Create a fresh code on your Mac."
        case .identity: return "This code could not verify your Mac. Create a fresh pairing code in Interface Settings."
        }
    }
}

/// Public identity plus a short-lived invitation. Never log or persist the scanned payload.
struct PairingInvitation: Codable, Equatable {
    let version: UInt8
    let kind: String
    let macId: String
    let macName: String
    let profile: String
    let publicKey: String
    let invitationId: String
    let issuedAt: UInt64
    let expiresAt: UInt64
    let relayOrigin: String?
    // Transient v6 routing hints. The nested v5 invitation's exact bytes stay frozen.
    var nearbyOrigins: [String] = []

    private enum CodingKeys: String, CodingKey {
        case version, kind, macId, macName, profile, publicKey, invitationId, issuedAt, expiresAt, relayOrigin
    }
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(version, forKey: .version)
        try container.encode(kind, forKey: .kind)
        try container.encode(macId, forKey: .macId)
        try container.encode(macName, forKey: .macName)
        try container.encode(profile, forKey: .profile)
        try container.encode(publicKey, forKey: .publicKey)
        try container.encode(invitationId, forKey: .invitationId)
        try container.encode(issuedAt, forKey: .issuedAt)
        try container.encode(expiresAt, forKey: .expiresAt)
        if let relayOrigin { try container.encode(relayOrigin, forKey: .relayOrigin) }
        else { try container.encodeNil(forKey: .relayOrigin) }
    }

    static func parse(_ qr: String, now: UInt64) throws -> PairingInvitation {
        if qr.hasPrefix("interface-pair:v6:") {
            return try parseDirect(qr, now: now)
        }
        let prefix = "interface-pair:v5:"
        guard qr.utf8.count <= 2048, qr.hasPrefix(prefix),
              let bytes = String(qr.dropFirst(prefix.count)).data(using: .utf8),
              let value = try? JSONDecoder().decode(Self.self, from: bytes) else {
            throw PairingInvitationError.invalid
        }
        // Exact canonical bytes reject duplicate/unknown keys and parser differences.
        // This format is emitted by Rust; the scanner is not a general JSON editor.
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        guard try encoder.encode(value) == bytes,
              value.version == 5, value.kind == "interfacePairing",
              ["production", "development"].contains(value.profile),
              lowerHex(value.macId, bytes: 32), lowerHex(value.invitationId, bytes: 32),
              lowerHex(value.publicKey, bytes: 65), value.publicKey.hasPrefix("04"),
              !value.macName.isEmpty, value.macName.utf8.count <= 80,
              value.macName.trimmingCharacters(in: .whitespacesAndNewlines) == value.macName,
              !value.macName.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }),
              value.issuedAt > 0, value.issuedAt <= 9_007_199_254_740_991 - 300,
              value.expiresAt > value.issuedAt, value.expiresAt - value.issuedAt <= 300,
              value.relayOrigin == nil || relayURL(value.relayOrigin!) != nil else {
            throw PairingInvitationError.invalid
        }
        guard now >= value.issuedAt, now < value.expiresAt else { throw PairingInvitationError.expired }
        guard ConnectionCrypto.identity(publicKey: value.publicKey, matches: value.macId) else {
            throw PairingInvitationError.identity
        }
        return value
    }

    private struct DirectInvitation: Codable {
        let version: UInt8
        let invitation: PairingInvitation
        let nearbyOrigins: [String]
    }
    private static func parseDirect(_ qr: String, now: UInt64) throws -> PairingInvitation {
        guard qr.utf8.count <= 2048,
              let data = String(qr.dropFirst("interface-pair:v6:".count)).data(using: .utf8),
              let wrapper = try? JSONDecoder().decode(DirectInvitation.self, from: data) else {
            throw PairingInvitationError.invalid
        }
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        guard try encoder.encode(wrapper) == data, wrapper.version == 6,
              validNearbyOrigins(wrapper.nearbyOrigins) else { throw PairingInvitationError.invalid }
        var invitation = try parse("interface-pair:v5:" + String(decoding: encoder.encode(wrapper.invitation), as: UTF8.self), now: now)
        invitation.nearbyOrigins = wrapper.nearbyOrigins
        return invitation
    }
    static func validNearbyOrigins(_ origins: [String]) -> Bool {
        origins.count <= 8 && Set(origins).count == origins.count && origins.allSatisfy { nearbyURL($0) != nil }
    }
    /// A nearby route: from the QR code, or one the paired Mac sent after
    /// unlock. A tailnet (100.64.0.0/10) address is not one: the Tailscale
    /// route was retired on 2026-10-14.
    static func nearbyURL(_ text: String) -> URL? {
        guard text.hasPrefix("http://"), text.utf8.count <= 40 else { return nil }
        let fields = text.dropFirst(7).split(separator: ":", omittingEmptySubsequences: false)
        guard fields.count == 2, let port = UInt16(fields[1]), port >= 1024 else { return nil }
        let octets = fields[0].split(separator: ".", omittingEmptySubsequences: false)
        let numbers = octets.compactMap { UInt8($0) }
        guard numbers.count == 4, octets.count == 4,
              zip(octets, numbers).allSatisfy({ String($0.0) == String($0.1) }),
              numbers[0] == 10 || (numbers[0] == 172 && (16...31).contains(numbers[1])) ||
                (numbers[0] == 192 && numbers[1] == 168) || (numbers[0] == 169 && numbers[1] == 254),
              text == "http://\(fields[0]):\(port)" else { return nil }
        return URL(string: text)
    }

    static func relayURL(_ text: String) -> URL? {
        guard text.utf8.count <= 253, text.unicodeScalars.allSatisfy({ $0.isASCII }),
              let parts = URLComponents(string: text), parts.scheme == "https",
              parts.user == nil, parts.password == nil, parts.query == nil, parts.fragment == nil,
              parts.port == nil, parts.path.isEmpty,
              let host = parts.host, host.contains("."), !host.hasSuffix("."),
              host == host.lowercased(), text == "https://\(host)",
              host.split(separator: ".", omittingEmptySubsequences: false).allSatisfy({ label in
                  !label.isEmpty && !label.hasPrefix("-") && !label.hasSuffix("-") &&
                  label.utf8.allSatisfy({ (97...122).contains($0) || (48...57).contains($0) || $0 == 45 })
              }),
              // Numeric authorities are not relay DNS names, including shorthand IPv4.
              !(host.split(separator: ".").last?.allSatisfy({ $0.isNumber }) ?? true)
        else { return nil }
        return parts.url
    }
    private static func lowerHex(_ text: String, bytes: Int) -> Bool {
        (try? ConnectionCrypto.unhex(text, count: bytes)) != nil
    }
}
