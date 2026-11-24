import XCTest
import CryptoKit
import SwiftUI
import WebKit
import Vision
@testable import InterfacePhone

final class UnlockLifecycleTests: XCTestCase {
    func testFaceIDDismissalAfterRefusalDoesNotStartAnotherPrompt() {
        var lifecycle = UnlockLifecycle()
        XCTAssertTrue(lifecycle.takeAutomaticUnlock(stored: true, busy: false, unlocked: false))
        // The prompt closes, the Mac refuses the old pairing, and the scene
        // reports active again after busy has cleared. Repeated events stay inert.
        for _ in 0..<20 {
            XCTAssertFalse(lifecycle.takeAutomaticUnlock(stored: true, busy: false, unlocked: false))
        }
    }
    func testBackgroundDuringCancelledAuthenticationDoesNotRearm() {
        var lifecycle = UnlockLifecycle()
        lifecycle.suppressAutomaticUnlock() // explicit Unlock or Request approval
        lifecycle.enteredBackground(hadSession: false)
        XCTAssertFalse(lifecycle.takeAutomaticUnlock(stored: true, busy: false, unlocked: false))
    }
    func testCompletedSessionUnlocksOnceAfterReturningFromBackground() {
        var lifecycle = UnlockLifecycle()
        lifecycle.suppressAutomaticUnlock()
        lifecycle.enteredBackground(hadSession: true)
        XCTAssertTrue(lifecycle.takeAutomaticUnlock(stored: true, busy: false, unlocked: false))
        XCTAssertFalse(lifecycle.takeAutomaticUnlock(stored: true, busy: false, unlocked: false))
        // A later successfully connected visit can re-arm the next return.
        lifecycle.enteredBackground(hadSession: true)
        XCTAssertTrue(lifecycle.takeAutomaticUnlock(stored: true, busy: false, unlocked: false))
    }
    func testManualLockAndInitialEnrollmentCannotTriggerAutomaticPrompts() {
        var lifecycle = UnlockLifecycle()
        XCTAssertFalse(lifecycle.takeAutomaticUnlock(stored: false, busy: false, unlocked: false))
        lifecycle.suppressAutomaticUnlock()
        XCTAssertFalse(lifecycle.takeAutomaticUnlock(stored: true, busy: false, unlocked: false))
        lifecycle.enteredBackground(hadSession: false)
        XCTAssertFalse(lifecycle.takeAutomaticUnlock(stored: true, busy: false, unlocked: false))
    }
}

final class MacAddressTests: XCTestCase {
    func testNavigationCannotLeaveSelectedMac() throws {
        let origin = try XCTUnwrap(URL(string: "https://mac.example.test/"))
        XCTAssertTrue(MacAddress.contains(try XCTUnwrap(URL(string: "https://mac.example.test:443/session")), origin: origin))
        for destination in ["https://other.example.test", "https://mac.example.test:8443/",
                            "http://mac.example.test/", "https://user@mac.example.test/", "javascript:alert(1)"] {
            XCTAssertFalse(MacAddress.contains(try XCTUnwrap(URL(string: destination)), origin: origin))
        }
    }
}

final class TrustedProtocolTests: XCTestCase {
    private func fixture(_ name: String) throws -> Data {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        return try Data(contentsOf: root.appendingPathComponent("electron/src/fixtures/\(name).json"))
    }
    func testNativeWorkspaceDTOUsesTheRustTypeScriptFixture() throws {
        let container = try JSONSerialization.jsonObject(with: fixture("remote-companion-v2")) as! [String: Any]
        let data = try JSONSerialization.data(withJSONObject: container["snapshot"]!)
        let snapshot = try JSONDecoder().decode(PhoneWorkspaceSnapshot.self, from: data)
        XCTAssertEqual(snapshot.version, 2)
        XCTAssertEqual(snapshot.workspaces[0].terminals[0].launch?.profileId, "claude")
        XCTAssertGreaterThan(snapshot.workspaces[0].terminals[0].generation, 0)
        XCTAssertEqual(snapshot.workspaces[0].terminals[0].root, "/Projects/Interface")
        XCTAssertEqual(PhoneProject.group(snapshot.workspaces).map(\.name), ["Interface"])
    }
    func testRosterV3FilesSessionsLikeTheMacSidebar() throws {
        let shared = try JSONSerialization.jsonObject(with: fixture("remote-projects-v3")) as! [String: Any]
        let roster: [String: Any] = ["version": 3, "projects": shared["expected"]!, "workspaces": [
            ["id": "w", "name": "Workspace 31", "root": "/Users/me/Kite App", "terminals": shared["terminals"]!]]]
        let snapshot = try JSONDecoder().decode(PhoneWorkspaceSnapshotV3.self, from: JSONSerialization.data(withJSONObject: roster))
        let projects = PhoneProject.group(snapshot.filed)
        XCTAssertEqual(projects.map(\.name), ["Harbor App", "Interface-Rust", "Journal App", "Kite App", "lib", "website", "Other"])
        XCTAssertEqual(projects.last?.terminals.map(\.title), ["Chat outside any repository"])
        XCTAssertEqual(projects.first { $0.name == "Kite App" }?.terminals.count, 2)
    }
    func testMacRoutesAreNearbyOnlySinceTheTailnetRouteWasRetired() throws {
        let shared = try JSONSerialization.jsonObject(with: fixture("remote-routes-v1")) as! [String: Any]
        for origin in try XCTUnwrap(shared["validOrigins"] as? [String]) { XCTAssertNotNil(PairingInvitation.nearbyURL(origin), origin) }
        for origin in try XCTUnwrap(shared["invalidOrigins"] as? [String]) { XCTAssertNil(PairingInvitation.nearbyURL(origin), origin) }
        let routes = try JSONDecoder().decode(MacRouteList.self, from: JSONSerialization.data(withJSONObject: XCTUnwrap(shared["routes"])))
        let defaults = try XCTUnwrap(UserDefaults(suiteName: "interface-routes-\(UUID().uuidString)"))
        NearbyRoutesV1.save(macId: "mac", origins: ["http://10.232.186.60:58747", "http://192.168.1.5:58747"], defaults: defaults)
        MacRoutesV1.save(macId: "mac", origins: routes.origins, defaults: defaults)
        XCTAssertEqual(MacRoutesV1.merged(macId: "mac", defaults: defaults),
                       ["http://10.232.186.60:58747", "http://192.168.1.20:58747", "http://192.168.1.5:58747"])
        XCTAssertEqual(MacRoutesV1.merged(macId: "another-mac", defaults: defaults), [])
        MacRoutesV1.save(macId: "mac", origins: ["http://8.8.8.8:58747"], defaults: defaults)
        XCTAssertEqual(MacRoutesV1.load(macId: "mac", defaults: defaults), routes.origins, "An invalid list never replaces saved routes")
        MacRoutesV1.save(macId: "mac", origins: ["http://10.232.186.60:58747", "http://100.101.102.103:58747"], defaults: defaults)
        XCTAssertEqual(MacRoutesV1.load(macId: "mac", defaults: defaults), routes.origins, "A Tailscale-era list is refused")
    }
    func testSessionsAreGroupedByProjectAcrossWorkspaces() {
        func terminal(_ id: String, _ root: String?, _ state: String = "running", at time: String? = nil) -> PhoneTerminal {
            PhoneTerminal(id: id, generation: 1, title: id, cwd: root ?? "/w", state: state, launch: nil, root: root, updatedAt: time)
        }
        let workspaces = [
            PhoneWorkspace(id: "a", name: "Workspace 31", root: "/Users/me/Kite App", terminals: [
                terminal("old", "/Users/me/Kite App", at: "2026-09-26T10:00:00Z"),
                terminal("rust", "/Users/me/Developer/Interface-Rust/"),
                terminal("done", "/Users/me/Kite App", "exited", at: "2026-09-26T23:00:00Z")]),
            PhoneWorkspace(id: "b", name: "Code review", root: "/Users/me/Kite App", terminals: [
                terminal("new", "/Users/me/Kite App", at: "2026-09-26T12:00:00Z"),
                terminal("legacy", nil),
                // A terminal listed twice is shown once.
                terminal("rust", "/Users/me/Developer/Interface-Rust")])
        ]
        let projects = PhoneProject.group(workspaces)
        XCTAssertEqual(projects.map(\.name), ["Interface-Rust", "Kite App"])
        XCTAssertEqual(projects.map(\.id), ["/Users/me/Developer/Interface-Rust", "/Users/me/Kite App"])
        XCTAssertEqual(projects[0].terminals.map(\.id), ["rust"])
        // Live first, then the most recently changed; an older host's terminal
        // without a root joins its workspace's folder.
        XCTAssertEqual(projects[1].terminals.map(\.id), ["new", "old", "legacy", "done"])
    }
    func testSessionsThatNeedYouComeFirstAndLiftTheirProject() {
        func terminal(_ id: String, _ root: String, waiting: Bool = false, at time: String) -> PhoneTerminal {
            PhoneTerminal(id: id, generation: 1, title: id, cwd: root, state: "running", launch: nil, root: root, updatedAt: time,
                          attention: waiting ? PhoneAttention(version: 1, since: time) : nil)
        }
        let workspaces = [PhoneWorkspace(id: "w", name: "W", root: "/p/A", terminals: [
            terminal("a-new", "/p/A", at: "2026-10-14T10:00:00Z"),
            terminal("z-old-waiting", "/p/Z", waiting: true, at: "2026-10-14T08:00:00Z"),
            terminal("z-new", "/p/Z", at: "2026-10-14T11:00:00Z")])]
        let projects = PhoneProject.group(workspaces)
        // Rust decides what needs you; the phone puts it first, project and row.
        XCTAssertEqual(projects.map(\.name), ["Z", "A"])
        XCTAssertEqual(projects[0].terminals.map(\.id), ["z-old-waiting", "z-new"])
        XCTAssertTrue(projects[0].needsYou); XCTAssertFalse(projects[1].needsYou)
    }
    func testNativeApprovalUsesTheSharedV3Fixture() throws {
        let container = try JSONSerialization.jsonObject(with: fixture("remote-trust-v3")) as! [String: Any]
        let data = try JSONSerialization.data(withJSONObject: container["reply"]!)
        let reply = try JSONDecoder().decode(EnrollmentReply.self, from: data)
        XCTAssertEqual(reply.version, 3); XCTAssertFalse(reply.approved); XCTAssertEqual(reply.verification, "12345678")
    }
    func testInputUncertaintySurvivesANewEphemeralViewAndIsScopedToMacAndGeneration() throws {
        let key = "fixture-\(UUID().uuidString)|generation-1"
        let other = key + "-2"
        defer { try? InputMarkers.set(key, command: nil) }
        XCTAssertFalse(InputMarkers.pending(key))
        try InputMarkers.set(key, command: UUID().uuidString)
        XCTAssertTrue(InputMarkers.pending(key)); XCTAssertFalse(InputMarkers.pending(other))
        try InputMarkers.set(key, command: nil); XCTAssertFalse(InputMarkers.pending(key))
    }
    @MainActor
    func testBackgroundLockImmediatelyDropsSessionsAndCancelsAnInFlightAttempt() {
        let model = PhoneModel()
        model.workspaces = [PhoneWorkspace(id: "workspace", name: "Fixture", root: "/fixture", terminals: [])]
        model.busy = true; model.waiting = "fixture"; model.online = true
        model.lock()
        XCTAssertNil(model.cookie); XCTAssertTrue(model.workspaces.isEmpty)
        XCTAssertFalse(model.busy); XCTAssertNil(model.waiting); XCTAssertFalse(model.online)
    }
}

final class PhoneThreadsTests: XCTestCase {
    private func fixture() throws -> [String: Any] {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let data = try Data(contentsOf: root.appendingPathComponent("electron/src/fixtures/remote-phone-v4.json"))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }
    func testSharedInboxDecodesThreadAndAgentProgressWithoutInventingSessionActions() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let data = try Data(contentsOf: root.appendingPathComponent("electron/src/fixtures/remote-inbox-v2.json"))
        let inbox = try JSONDecoder().decode(PhoneInbox.self, from: data)
        XCTAssertEqual(inbox.version, 2)
        XCTAssertEqual(inbox.items.map(\.source), ["thread", "agentTask", "agentTask"])
        XCTAssertNil(inbox.items[1].thread)
        XCTAssertFalse(inbox.items[1].canSend)
        XCTAssertFalse(inbox.items[2].canSend)
        XCTAssertEqual(inbox.items[2].thread?.path, "api/v1/threads/claude/agent-done")
    }
    func testRosterV4CarriesTheMacsNeedsYou() throws {
        let shared = try fixture()
        let snapshot = try JSONDecoder().decode(PhoneWorkspaceSnapshotV3.self, from: JSONSerialization.data(withJSONObject: XCTUnwrap(shared["roster"])))
        XCTAssertEqual(snapshot.version, 4)
        let terminals = snapshot.filed[0].terminals
        XCTAssertEqual(terminals.map(\.needsYou), [true, false])
        XCTAssertEqual(terminals[0].attention?.since, "2026-10-14T09:12:00.000Z")
        XCTAssertEqual(terminals[1].project, .other)
        let live = terminals[0]
        let exited = PhoneTerminal(id: live.id, generation: live.generation, title: live.title, cwd: live.cwd,
                                   state: "exited", launch: live.launch, attention: live.attention)
        XCTAssertFalse(exited.needsYou, "A session that ended no longer needs you")
    }
    func testThreadsUseTheSharedFixtureAndAddressOnlyWellFormedKeys() throws {
        let shared = try fixture()
        let list = try JSONDecoder().decode(PhoneThreadList.self, from: JSONSerialization.data(withJSONObject: XCTUnwrap(shared["threads"])))
        XCTAssertEqual(list.version, 1)
        XCTAssertEqual(list.threads.map(\.status), ["needsYou", "working", "idle", "failed"])
        XCTAssertTrue(list.threads[0].needsYou); XCTAssertTrue(list.threads[1].working)
        XCTAssertEqual(list.threads[0].projectName, "Interface"); XCTAssertNil(list.threads[2].project)
        let grok = list.threads[3]
        XCTAssertEqual(grok.path, "api/v1/threads/acp:grok/ses_fixture4"); XCTAssertEqual(grok.profileId, "grok")
        for (engine, session) in [("claude", "a/b"), ("claude", "a#b"), ("claude/x", "a"), ("", "a"), ("claude", "a?b")] {
            let forged = PhoneThread(key: "\(engine)#\(session)", engine: engine, sessionId: session, title: "x", project: nil,
                                     status: "idle", section: "active", unread: false, lastActivity: "")
            XCTAssertNil(forged.path, "\(engine) \(session)")
        }
        let detail = try JSONDecoder().decode(PhoneThreadDetail.self, from: JSONSerialization.data(withJSONObject: XCTUnwrap(shared["detail"])))
        XCTAssertEqual(detail.thread, list.threads[0])
        XCTAssertEqual(detail.turns.count, 2); XCTAssertEqual(detail.olderTurns, 3); XCTAssertTrue(detail.turns[1].clipped)
        XCTAssertEqual(detail.approval?.answerable, true); XCTAssertNil(detail.unavailable)
        let response = PhoneThreadApprovalResponse(approvalId: "toolu_fixture", decision: "allowOnce")
        let encoded = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(response)) as? NSDictionary)
        XCTAssertEqual(encoded, try XCTUnwrap(shared["approvalResponse"] as? NSDictionary))
    }
    func testAnUncertainThreadAnswerBlocksOnlyThatRequestOnThatMac() throws {
        let mac = "fixture-\(UUID().uuidString)", thread = "claude#fixture", other = "claude#other"
        defer { ThreadAnswerMarkers.clear(macId: mac, thread: thread) }
        XCTAssertFalse(ThreadAnswerMarkers.blocks(macId: mac, thread: thread, approvalId: "a1"))
        try ThreadAnswerMarkers.begin(macId: mac, thread: thread, approvalId: "a1", command: UUID().uuidString)
        XCTAssertTrue(ThreadAnswerMarkers.blocks(macId: mac, thread: thread, approvalId: "a1"))
        XCTAssertFalse(ThreadAnswerMarkers.blocks(macId: mac, thread: other, approvalId: "a1"))
        XCTAssertFalse(ThreadAnswerMarkers.blocks(macId: mac + "-2", thread: thread, approvalId: "a1"))
        XCTAssertTrue(InputMarkers.pending(ThreadAnswerMarkers.key(macId: mac, thread: thread)), "kept beside terminal input markers")
        // The Mac now shows another request (or none): the earlier answer resolved one way or the other.
        XCTAssertFalse(ThreadAnswerMarkers.blocks(macId: mac, thread: thread, approvalId: "a2"))
        XCTAssertFalse(ThreadAnswerMarkers.blocks(macId: mac, thread: thread, approvalId: "a1"))
    }
    func testAnUncertainFollowUpWaitsForANewerTurnOrTheOwnersCheck() throws {
        let mac = "fixture-\(UUID().uuidString)", thread = "codex#fixture"
        defer { ThreadSendMarkers.clear(macId: mac, thread: thread) }
        XCTAssertFalse(ThreadSendMarkers.blocks(macId: mac, thread: thread, turns: 4))
        try ThreadSendMarkers.begin(macId: mac, thread: thread, turns: 4, command: UUID().uuidString)
        XCTAssertTrue(ThreadSendMarkers.blocks(macId: mac, thread: thread, turns: 4))
        XCTAssertFalse(ThreadSendMarkers.blocks(macId: mac, thread: "codex#other", turns: 4))
        // A newer turn means the Mac moved on: the message arrived or another did.
        XCTAssertFalse(ThreadSendMarkers.blocks(macId: mac, thread: thread, turns: 5))
        try ThreadSendMarkers.begin(macId: mac, thread: thread, turns: 5, command: UUID().uuidString)
        ThreadSendMarkers.clear(macId: mac, thread: thread)
        XCTAssertFalse(ThreadSendMarkers.blocks(macId: mac, thread: thread, turns: 5), "The owner checked on the Mac")
    }
}

final class PairingInvitationTests: XCTestCase {
    private func fixture() throws -> [String: Any] {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let data = try Data(contentsOf: root.appendingPathComponent("electron/src/fixtures/remote-connection-v5.json"))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }
    private func encode(_ value: [String: Any]) throws -> String {
        "interface-pair:v5:" + String(decoding: try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .withoutEscapingSlashes]), as: UTF8.self)
    }
    func testScannerAcceptsRustQRAndPinsProductionIdentity() throws {
        let sample = try fixture()
        let qr = try XCTUnwrap(sample["qr"] as? String)
        let value = try PairingInvitation.parse(qr, now: 1789142400)
        XCTAssertEqual(value.macName, "Studio Mac")
        XCTAssertEqual(value.profile, "production")
        XCTAssertEqual(value.relayOrigin, "https://relay.example.com")
        XCTAssertThrowsError(try PairingInvitation.parse(qr, now: 1789142399))
        XCTAssertThrowsError(try PairingInvitation.parse(qr, now: 1789142700))
        XCTAssertThrowsError(try PairingInvitation.parse(qr + " ", now: 1789142400))
        let duplicate = qr.replacingOccurrences(of: "{", with: "{\"version\":5,")
        XCTAssertThrowsError(try PairingInvitation.parse(duplicate, now: 1789142400))
    }
    func testV6ScannerAndSavedRoutesUseTheSharedContractWithoutChangingV5Identity() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let sample = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("electron/src/fixtures/remote-connection-v6.json"))) as? [String: Any])
        let qr = try XCTUnwrap(sample["qr"] as? String)
        let invitation = try PairingInvitation.parse(qr, now: 1789142400)
        let legacy = try PairingInvitation.parse(XCTUnwrap(fixture()["qr"] as? String), now: 1789142400)
        XCTAssertEqual(SavedMacConnection(invitation), SavedMacConnection(legacy))
        XCTAssertTrue(legacy.nearbyOrigins.isEmpty)
        XCTAssertFalse(invitation.nearbyOrigins.isEmpty)
        let suite = "InterfaceRoutesTest-" + UUID().uuidString
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        NearbyRoutesV1.save(macId: invitation.macId, origins: invitation.nearbyOrigins, defaults: defaults)
        XCTAssertEqual(NearbyRoutesV1.load(macId: invitation.macId, defaults: defaults), invitation.nearbyOrigins)
        XCTAssertTrue(NearbyRoutesV1.load(macId: "another-mac", defaults: defaults).isEmpty)
        for origin in try XCTUnwrap(sample["validNearbyOrigins"] as? [String]) { XCTAssertNotNil(PairingInvitation.nearbyURL(origin), origin) }
        let wrapper = try XCTUnwrap(sample["invitation"] as? [String: Any])
        func encodeDirect(_ value: [String: Any]) throws -> String {
            "interface-pair:v6:" + String(decoding: try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .withoutEscapingSlashes]), as: UTF8.self)
        }
        for origin in try XCTUnwrap(sample["invalidNearbyOrigins"] as? [String]) {
            XCTAssertNil(PairingInvitation.nearbyURL(origin), origin)
            var altered = wrapper; altered["nearbyOrigins"] = [origin]
            XCTAssertThrowsError(try PairingInvitation.parse(encodeDirect(altered), now: 1789142400), origin)
        }
        for (field, value) in [("version", 5 as Any), ("secret", "unexpected" as Any),
                               ("nearbyOrigins", Array(repeating: invitation.nearbyOrigins[0], count: 2) as Any),
                               ("nearbyOrigins", (1...9).map { "http://10.0.0.\($0):5000" } as Any)] {
            var altered = wrapper; altered[field] = value
            XCTAssertThrowsError(try PairingInvitation.parse(encodeDirect(altered), now: 1789142400))
        }
        XCTAssertThrowsError(try PairingInvitation.parse(qr, now: 1789142700))
        XCTAssertThrowsError(try PairingInvitation.parse(qr + " ", now: 1789142400))
        XCTAssertThrowsError(try PairingInvitation.parse(qr.replacingOccurrences(of: "{", with: "{\"version\":6,"), now: 1789142400))
    }

    func testScannerRejectsUnknownFieldsVersionChangedIdentityAndInsecureRelay() throws {
        let sample = try fixture()
        let value = try XCTUnwrap(sample["invitation"] as? [String: Any])
        let invalid: [(String, Any)] = [
            ("version", 6), ("secret", "unexpected"), ("profile", "unknown"),
            ("macId", String(repeating: "ab", count: 32)),
            ("publicKey", "04" + String(repeating: "00", count: 64)),
            ("macName", "Mac\n"), ("expiresAt", 1789142701),
            ("invitationId", String(repeating: "A", count: 64)),
        ]
        for (key, bad) in invalid {
            var altered = value; altered[key] = bad
            XCTAssertThrowsError(try PairingInvitation.parse(encode(altered), now: 1789142400), key)
        }
        for origin in try XCTUnwrap(sample["invalidRelayOrigins"] as? [String]) {
            XCTAssertNil(PairingInvitation.relayURL(origin), origin)
            var altered = value; altered["relayOrigin"] = origin
            XCTAssertThrowsError(try PairingInvitation.parse(encode(altered), now: 1789142400), origin)
        }
        var nearby = value; nearby["relayOrigin"] = NSNull()
        XCTAssertNil(try PairingInvitation.parse(encode(nearby), now: 1789142400).relayOrigin)
    }
    func testTheScannerAndTheSavedConnectionPinTheSameMacIdentity() throws {
        let sample = try fixture()
        let value = try XCTUnwrap(sample["invitation"] as? [String: Any])
        let publicKey = try XCTUnwrap(value["publicKey"] as? String)
        let macId = try XCTUnwrap(value["macId"] as? String)
        XCTAssertTrue(ConnectionCrypto.identity(publicKey: publicKey, matches: macId))
        XCTAssertFalse(ConnectionCrypto.identity(publicKey: publicKey, matches: String(repeating: "ab", count: 32)))
        XCTAssertFalse(ConnectionCrypto.identity(publicKey: publicKey.uppercased(), matches: macId))
        XCTAssertFalse(ConnectionCrypto.identity(publicKey: "04" + String(repeating: "00", count: 64), matches: macId))
        var altered = value; altered["macId"] = String(repeating: "ab", count: 32)
        XCTAssertThrowsError(try PairingInvitation.parse(encode(altered), now: 1789142400)) { error in
            guard case PairingInvitationError.identity = error else { return XCTFail("\(error)") }
        }
        let saved = SavedMacConnection(try PairingInvitation.parse(XCTUnwrap(sample["qr"] as? String), now: 1789142400))
        XCTAssertTrue(saved.valid)
        var stored = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(saved)) as? [String: Any])
        stored["macId"] = String(repeating: "ab", count: 32)
        let tampered = try JSONDecoder().decode(SavedMacConnection.self, from: JSONSerialization.data(withJSONObject: stored))
        XCTAssertFalse(tampered.valid)
        let credential = try DeviceCredential.create()
        XCTAssertEqual(try ConnectionCrypto.unhex(credential.secret, count: 32).count, 32)
    }
}

final class EncryptedConnectionTests: XCTestCase {
    func testRequestEncoderMatchesTheRustV5FixtureIncludingNulls() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("electron/src/fixtures/remote-connection-v5.json"))) as? [String: Any])
        let request = TunnelRequest(issuedAt: 1789142400, expiresAt: 1789142490, method: "GET", path: "/api/v2/workspaces", body: "", cookie: nil, control: nil, command: nil, invitationId: nil)
        let actual = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as? NSDictionary)
        XCTAssertEqual(actual, try XCTUnwrap(fixture["request"] as? NSDictionary))
        let response = try JSONDecoder().decode(TunnelResponse.self, from: JSONSerialization.data(withJSONObject: fixture["response"]!))
        XCTAssertEqual(response.version, 5); XCTAssertEqual(response.status, 204); XCTAssertNil(response.setCookie)
    }
    func testLiveDirectAddressEncryptedEnrollmentWorkspacesHistoryAndLock() async throws {
        let descriptor = URL(fileURLWithPath: "/private/tmp/interface-phone-v5-fixture.json")
        guard FileManager.default.fileExists(atPath: descriptor.path) else { throw XCTSkip("Start the disposable phone_fixture example for the live Bonjour test.") }
        let data = try Data(contentsOf: descriptor)
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(fixture["fixture"] as? Bool, true)
        let invitation = try PairingInvitation.parse(XCTUnwrap(fixture["qr"] as? String), now: UInt64(Date().timeIntervalSince1970))
        let rows = try XCTUnwrap(fixture["rows"] as? [String])
        let size = CGFloat(rows.count + 8) * 4
        let qr = await MainActor.run {
            UIGraphicsImageRenderer(size: CGSize(width: size, height: size)).image { context in
                UIColor.white.setFill(); context.fill(CGRect(x: 0, y: 0, width: size, height: size))
                UIColor.black.setFill()
                for (y, row) in rows.enumerated() {
                    for (x, bit) in row.enumerated() where bit == "1" {
                        context.fill(CGRect(x: (x + 4) * 4, y: (y + 4) * 4, width: 4, height: 4))
                    }
                }
            }
        }
        let barcode = VNDetectBarcodesRequest(); barcode.symbologies = [.qr]
        #if targetEnvironment(simulator)
        // Revision 3 uses an ML inference context unavailable in this Simulator.
        // Exercise the same Rust QR pixels with Vision's non-ML QR decoder.
        barcode.revision = VNDetectBarcodesRequestRevision1
        #endif
        try VNImageRequestHandler(cgImage: XCTUnwrap(qr.cgImage)).perform([barcode])
        XCTAssertEqual(barcode.results?.first?.payloadStringValue, fixture["qr"] as? String, "Apple Vision must decode the actual Rust QR matrix.")
        let publicTestIdentity = try P256.KeyAgreement.PrivateKey(rawRepresentation: Data(repeating: 0, count: 31) + Data([1]))
        XCTAssertEqual(invitation.publicKey, ConnectionCrypto.hex(publicTestIdentity.publicKey.x963Representation))
        let connection = SavedMacConnection(invitation)
        XCTAssertFalse(invitation.nearbyOrigins.isEmpty)
        let noRoute = MacClient(origin: connection.virtualOrigin, connection: connection,
            nearbyOrigins: [], discover: { _ in nil })
        do { _ = try await noRoute.request("api/v5/ping"); XCTFail("The old discovery-only route must reproduce the failure.") }
        catch PhoneError.unavailable { }
        noRoute.cancel()
        // Reproduce the physical failure: discovery never returns a hostname.
        let client = MacClient(origin: connection.virtualOrigin, connection: connection, invitationId: invitation.invitationId,
            nearbyOrigins: invitation.nearbyOrigins, discover: { _ in nil })
        defer { client.cancel() }
        let credential = try DeviceCredential.create()
        var enrollment = try await client.enrollment(credential, create: true)
        for _ in 0..<30 where !enrollment.approved {
            try await Task.sleep(for: .milliseconds(100))
            enrollment = try await client.enrollment(credential, create: false)
        }
        XCTAssertTrue(enrollment.approved)
        let cookie = try await client.unlock(credential)
        let workspaces = try await client.workspaces(cookie: cookie)
        let terminal = try XCTUnwrap(workspaces.first?.terminals.first)
        XCTAssertEqual(terminal.id, "00000000-0000-4000-8000-000000000002")
        XCTAssertTrue(terminal.needsYou, "Roster v4 carries the Mac's needs-you over the encrypted route")
        let listed = try await client.threads(cookie: cookie)
        let sharedInbox = try await client.inbox(cookie: cookie)
        XCTAssertEqual(sharedInbox?.version, 2)
        XCTAssertEqual(sharedInbox?.items[1].source, "agentTask")
        XCTAssertFalse(try XCTUnwrap(sharedInbox?.items[1].canSend))
        let threads = try XCTUnwrap(listed)
        XCTAssertEqual(threads.first?.status, "needsYou")
        let detail = try await client.threadDetail(threads[0], cookie: cookie)
        let approval = try XCTUnwrap(detail.approval)
        let gone = await client.answer(threads[0], approvalId: "toolu_gone", allow: true, cookie: cookie)
        guard case .refused = gone else { return XCTFail("A changed request is refused before anything runs: \(gone)") }
        let sent = await client.answer(threads[0], approvalId: approval.id, allow: true, cookie: cookie)
        XCTAssertEqual(sent, .sent)
        let followUp = await client.send(threads[2], text: "Thanks, carry on.", turns: 0, cookie: cookie)
        XCTAssertEqual(followUp, .sent)
        let busy = await client.send(threads[0], text: "Hello?", turns: 0, cookie: cookie)
        guard case .refused = busy else { return XCTFail("A thread that is not at rest takes no follow-up: \(busy)") }
        let direct = try XCTUnwrap(client.direct)
        let (history, response) = try await direct.request(path: "/api/v2/terminals/\(terminal.id)/\(terminal.generation)/history?limit=500", method: "GET", body: Data(), cookie: cookie)
        XCTAssertEqual(response.statusCode, 200)
        XCTAssertTrue(String(decoding: history, as: UTF8.self).contains("Show my Interface sessions on iPhone."))
        let route = await direct.routeName
        XCTAssertEqual(route, "Connected nearby")
        try await NativeSessionViewTests.assertLiveConversation(client: client, cookie: cookie, terminal: terminal)
        _ = try await client.request("api/v3/lock", body: [:], cookie: cookie)
        do { _ = try await client.workspaces(cookie: cookie); XCTFail("A locked grant must be refused.") }
        catch PhoneError.refused { }
    }
    func testRustAndCryptoKitVectorsInBothDirections() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let data = try Data(contentsOf: root.appendingPathComponent("electron/src/fixtures/remote-connection-crypto-v5.json"))
        let fixtures = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let mac = try P256.KeyAgreement.PrivateKey(rawRepresentation: Data(repeating: 0, count: 31) + Data([1]))
        let phone = try P256.KeyAgreement.PrivateKey(rawRepresentation: Data(repeating: 0, count: 31) + Data([2]))
        for name in ["rust", "swift"] {
            let value = try XCTUnwrap(fixtures[name] as? [String: Any])
            let macId = try XCTUnwrap(value["macId"] as? String)
            let id = try XCTUnwrap(value["requestId"] as? String)
            let request = try JSONDecoder().decode(EncryptedPacket.self, from: JSONSerialization.data(withJSONObject: value["request"]!))
            let response = try JSONDecoder().decode(EncryptedPacket.self, from: JSONSerialization.data(withJSONObject: value["response"]!))
            XCTAssertEqual(try ConnectionCrypto.open(request, key: mac, macId: macId, requestId: id, expectedSender: ConnectionCrypto.hex(phone.publicKey.x963Representation), direction: "request"), Data("Interface cross-language request".utf8))
            XCTAssertEqual(try ConnectionCrypto.open(response, key: phone, macId: macId, requestId: id, expectedSender: ConnectionCrypto.hex(mac.publicKey.x963Representation)), Data("Interface cross-language response".utf8))
            XCTAssertThrowsError(try ConnectionCrypto.open(response, key: phone, macId: macId, requestId: UUID().uuidString.lowercased(), expectedSender: ConnectionCrypto.hex(mac.publicKey.x963Representation)))
            XCTAssertThrowsError(try ConnectionCrypto.open(request, key: mac, macId: macId, requestId: id, expectedSender: ConnectionCrypto.hex(phone.publicKey.x963Representation)))
        }
    }
    func testNativeBridgeCannotReachOtherSessionsOrCredentialEndpoints() {
        let terminal = PhoneTerminal(id: "12345678-1234-4234-8234-123456789abc", generation: 2, title: "Fixture", cwd: "/fixture", state: "running", launch: .init(profileId: "codex"))
        let target = "/terminals/\(terminal.id)/2/"
        XCTAssertTrue(NativeSessionRequest.allowed(path: "/api/v5" + target + "output?cursor=12", method: "GET", terminal: terminal))
        XCTAssertTrue(NativeSessionRequest.allowed(path: "/api/v1" + target + "input", method: "POST", terminal: terminal))
        XCTAssertTrue(NativeSessionRequest.allowed(path: "/api/v1" + target + "draft", method: "GET", terminal: terminal))
        XCTAssertTrue(NativeSessionRequest.allowed(path: "/api/v1" + target + "draft", method: "POST", terminal: terminal))
        XCTAssertTrue(NativeSessionRequest.allowed(path: "/api/v1" + target + "results", method: "GET", terminal: terminal))
        XCTAssertFalse(NativeSessionRequest.allowed(path: "/api/v1" + target + "results", method: "POST", terminal: terminal))
        XCTAssertFalse(NativeSessionRequest.allowed(path: "/api/v1" + target + "draft?generation=99", method: "POST", terminal: terminal))
        for path in ["/api/v3/unlock", "/api/v3/enrollment", "/api/v1/pair", "https://evil.example/api/v2/workspaces", "/api/v1/terminals/\(terminal.id)/3/input", "/api/v1" + target + "input?extra=1", "/api/v2" + target + "history?limit=4&path=/tmp", "/api/v5" + target + "output?cursor=0#fragment"] {
            XCTAssertFalse(NativeSessionRequest.allowed(path: path, method: "POST", terminal: terminal), path)
            XCTAssertFalse(NativeSessionRequest.allowed(path: path, method: "GET", terminal: terminal), path)
        }
    }
}

final class NativeSessionViewTests: XCTestCase {
    @MainActor
    static func assertLiveConversation(client: MacClient, cookie: HTTPCookie, terminal: PhoneTerminal) async throws {
        let origin = client.origin
        let view = SessionWebView(origin: origin, cookie: cookie, terminal: terminal, reload: 0, error: .constant(nil), loading: .constant(true), client: client)
        let controller = UIHostingController(rootView: view)
        let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 390, height: 844))
        window.rootViewController = controller; window.makeKeyAndVisible(); controller.view.layoutIfNeeded()
        defer { window.isHidden = true; window.rootViewController = nil }
        func webView(_ view: UIView) -> WKWebView? {
            if let web = view as? WKWebView { return web }
            return view.subviews.lazy.compactMap(webView).first
        }
        var rendered = false
        for _ in 0..<100 {
            if let web = webView(controller.view),
               let ready = try? await web.evaluateJavaScript("document.body.innerText.includes('Show my Interface sessions on iPhone.')"), ready as? Bool == true {
                XCTAssertEqual(web.url?.scheme, "interface-session")
                rendered = true; break
            }
            try await Task.sleep(for: .milliseconds(100))
        }
        if !rendered, let web = webView(controller.view) {
            let observed = (try? await web.evaluateJavaScript("document.body.innerText")) as? String ?? "WebKit unavailable"
            XCTFail("Live fixture did not render: \(observed.prefix(1500))")
        }
        XCTAssertTrue(rendered, "The actual shared conversation must load from Rust through the numeric route, HPKE and the restricted WebKit bridge.")
    }
    @MainActor
    func testBundledConversationBootsThroughTheRestrictedNativeBridge() async throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("electron/src/fixtures/remote-connection-v5.json"))) as? [String: Any])
        let invitation = try PairingInvitation.parse(XCTUnwrap(fixture["qr"] as? String), now: 1789142400)
        let connection = SavedMacConnection(invitation)
        let client = MacClient(origin: connection.virtualOrigin, connection: connection)
        defer { client.cancel() }
        let cookie = try XCTUnwrap(HTTPCookie(properties: [.name:"interface_remote", .value:String(repeating: "a", count: 32), .domain:connection.virtualOrigin.host!, .path:"/", .secure:"TRUE"]))
        let terminal = PhoneTerminal(id: "12345678-1234-4234-8234-123456789abc", generation: 2, title: "Fixture", cwd: "/fixture", state: "running", launch: .init(profileId: "codex"))
        let view = SessionWebView(origin: connection.virtualOrigin, cookie: cookie, terminal: terminal, reload: 0, error: .constant(nil), loading: .constant(true), client: client)
        let controller = UIHostingController(rootView: view)
        let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 390, height: 844))
        window.rootViewController = controller; window.makeKeyAndVisible(); controller.view.layoutIfNeeded()
        defer { window.isHidden = true; window.rootViewController = nil }
        func webView(_ view: UIView) -> WKWebView? {
            if let web = view as? WKWebView { return web }
            return view.subviews.lazy.compactMap(webView).first
        }
        var booted = false
        for _ in 0..<100 {
            if let web = webView(controller.view),
               let ready = try? await web.evaluateJavaScript("window.__interfaceConnectionVersion === 5 && !!document.querySelector('.remote-app')"), ready as? Bool == true {
                XCTAssertEqual(web.url?.scheme, "interface-session")
                let color = try await web.evaluateJavaScript("getComputedStyle(document.body).backgroundColor") as? String
                XCTAssertNotEqual(color, "rgba(0, 0, 0, 0)")
                booted = true; break
            }
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertTrue(booted, "The actual bundled JavaScript/CSS must render through the native custom scheme.")
    }
}

final class ProviderMarkTests: XCTestCase {
    func testMuseCodeSessionsShowTheMuseMarkInMuseBlue() throws {
        for profile in ["muse", "acp:muse"] {
            let mark = try XCTUnwrap(ProviderMark.marks[profile], "\(profile) must not fall back to the terminal glyph.")
            XCTAssertEqual(mark.asset, "ProviderMuse")
            var red: CGFloat = 0, green: CGFloat = 0, blue: CGFloat = 0, alpha: CGFloat = 0
            XCTAssertTrue(UIColor(mark.color).getRed(&red, green: &green, blue: &blue, alpha: &alpha))
            XCTAssertEqual([red, green, blue].map { Int(($0 * 255).rounded()) }, [0, 130, 251])
        }
        XCTAssertNotNil(UIImage(named: "ProviderMuse"), "The asset catalog must ship the Muse squiggle.")
    }

    func testKimiCodeSessionsShowTheInkKWithTheKimiBlueDot() throws {
        func rgb(_ color: Color) -> [Int] {
            var red: CGFloat = 0, green: CGFloat = 0, blue: CGFloat = 0, alpha: CGFloat = 0
            XCTAssertTrue(UIColor(color).getRed(&red, green: &green, blue: &blue, alpha: &alpha))
            return [red, green, blue].map { Int(($0 * 255).rounded()) }
        }
        for profile in ["kimi", "acp:kimi"] {
            let mark = try XCTUnwrap(ProviderMark.marks[profile], "\(profile) must not fall back to the terminal glyph.")
            XCTAssertEqual(mark.asset, "ProviderKimi")
            XCTAssertEqual(rgb(mark.color), rgb(PhoneStyle.text))
            let dot = try XCTUnwrap(ProviderMark.accents[profile])
            XCTAssertEqual(dot.asset, "ProviderKimiDot")
            XCTAssertEqual(rgb(dot.color), [23, 131, 255])
        }
        XCTAssertNil(ProviderMark.accents["muse"], "Single-colour marks have no accent layer.")
        XCTAssertNotNil(UIImage(named: "ProviderKimi"), "The asset catalog must ship the Kimi K.")
        XCTAssertNotNil(UIImage(named: "ProviderKimiDot"), "The asset catalog must ship the Kimi dot.")
    }
}

import LocalAuthentication
final class KeychainProtectionTests: XCTestCase {
    func testStoredTrustRefusesAReadWithoutUserPresence() async throws {
        let origin = URL(string: "https://fixture-\(UUID().uuidString.lowercased()).example.ts.net/")!
        let credential = try DeviceCredential.create()
        defer { try? DeviceKeychain.forget(origin: origin) }
        try DeviceKeychain.save(credential, origin: origin)
        let context = LAContext(); context.interactionNotAllowed = true
        do {
            _ = try await DeviceKeychain.read(origin: origin, context: context)
            XCTFail("Protected trust was readable without user presence")
        } catch PhoneError.locked { /* expected: no silent credential access */ }
    }
    // A restored or transferred iPhone keeps the app's trust flags but not this
    // device-only item. That must not read as the Mac revoking this phone.
    func testAMissingIdentityIsNotAMacRefusal() async throws {
        let origin = URL(string: "https://fixture-\(UUID().uuidString.lowercased()).example.ts.net/")!
        do {
            _ = try await DeviceKeychain.lookup(origin: origin, context: LAContext())
            XCTFail("An absent identity was returned")
        } catch PhoneError.identityMissing { /* expected: pairing may replace it */ }
    }
}

final class DirectRouteTests: XCTestCase {
    private actor Probes {
        var order: [String] = []
        func record(_ url: URL) { order.append(url.absoluteString) }
    }
    private func connection() throws -> SavedMacConnection {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("electron/src/fixtures/remote-connection-v5.json"))) as? [String: Any])
        let connection = SavedMacConnection(try PairingInvitation.parse(XCTUnwrap(fixture["qr"] as? String), now: 1789142400))
        XCTAssertNotNil(connection.relayOrigin)
        return connection
    }
    func testATailnetAddressIsNoLongerARoute() async throws {
        let transport = DirectTransport(connection: try connection(), session: .shared, nearbyOrigins: ["http://100.101.102.103:4321"],
            discover: { _ in nil }, probe: { _ in true })
        let selected = try await transport.selectEndpoint()
        XCTAssertEqual(selected.scheme, "https")
        let route = await transport.routeName
        XCTAssertEqual(route, "Connected through relay")
    }
    func testANearbyRouteWinsEvenWhenTheRelayWouldAnswerFirst() async throws {
        let probes = Probes()
        let transport = DirectTransport(connection: try connection(), session: .shared, nearbyOrigins: ["http://192.168.1.20:4321"],
            discover: { _ in nil },
            probe: { url in
                await probes.record(url)
                if url.scheme == "https" { return true }
                try? await Task.sleep(for: .milliseconds(300))
                return true
            })
        let selected = try await transport.selectEndpoint()
        XCTAssertEqual(selected.absoluteString, "http://192.168.1.20:4321")
        let order = await probes.order
        XCTAssertFalse(order.contains { $0.hasPrefix("https://") }, "A phone on the Mac's network sends the relay nothing.")
    }
    func testNearbyRoutesAndBonjourAreProbedTogether() async throws {
        let transport = DirectTransport(connection: try connection(), session: .shared,
            nearbyOrigins: ["http://192.168.1.20:4321", "http://10.0.0.5:4321"],
            // Discovery ignores cancellation, like NetServiceBrowser's deadline.
            discover: { _ in try? await Task.sleep(for: .seconds(2)); return nil },
            probe: { url in
                if url.host == "192.168.1.20" { try? await Task.sleep(for: .seconds(3)); return false }
                try? await Task.sleep(for: .milliseconds(50))
                return url.host == "10.0.0.5"
            })
        let started = ContinuousClock.now
        let selected = try await transport.selectEndpoint()
        XCTAssertEqual(selected.host, "10.0.0.5")
        XCTAssertLessThan(ContinuousClock.now - started, .seconds(1), "A slow route or discovery must not delay a nearby success.")
    }
    func testTheRelayIsTriedOnlyAfterEveryNearbyRouteFails() async throws {
        let probes = Probes()
        let transport = DirectTransport(connection: try connection(), session: .shared,
            nearbyOrigins: ["http://192.168.1.20:4321", "http://10.0.0.5:4321"],
            discover: { _ in URL(string: "http://studio.local:4321") },
            probe: { url in
                await probes.record(url)
                return url.scheme == "https"
            })
        let selected = try await transport.selectEndpoint()
        XCTAssertEqual(selected.scheme, "https")
        let order = await probes.order
        XCTAssertEqual(order.count, 4)
        XCTAssertEqual(order.last, selected.absoluteString)
    }
    func testASeededRouteIsUsedWithoutDiscoveryOrProbes() async throws {
        let probes = Probes()
        let seeded = try XCTUnwrap(URL(string: "http://10.0.0.5:4321"))
        let transport = DirectTransport(connection: try connection(), session: .shared,
            nearbyOrigins: ["http://192.168.1.20:4321"],
            discover: { _ in await probes.record(URL(string: "http://discovery.invalid")!); return nil },
            probe: { url in await probes.record(url); return true },
            endpoint: seeded)
        let current = await transport.currentEndpoint
        XCTAssertEqual(current, seeded)
        let selected = try await transport.selectEndpoint()
        XCTAssertEqual(selected, seeded)
        let order = await probes.order
        XCTAssertTrue(order.isEmpty, "A proven route needs no discovery before the revoke.")
        let client = MacClient(origin: try connection().virtualOrigin, connection: try connection(), endpoint: seeded)
        let threaded = await client.direct?.currentEndpoint
        XCTAssertEqual(threaded, seeded)
        client.cancel()
        let unseeded = MacClient(origin: try connection().virtualOrigin, connection: try connection())
        let none = await unseeded.direct?.currentEndpoint
        XCTAssertNil(none)
        unseeded.cancel()
    }
    /// A transport to a stand-in Mac whose key the stub holds, so revokes really seal and open.
    private func stubMac(losing lost: Set<String> = []) throws -> (SavedMacConnection, URLSession) {
        let key = P256.KeyAgreement.PrivateKey()
        let publicKey = key.publicKey.x963Representation
        let macId = ConnectionCrypto.hex(Data(SHA256.hash(data: publicKey)))
        let connection = SavedMacConnection(PairingInvitation(version: 5, kind: "interfacePairing", macId: macId,
            macName: "Stub Mac", profile: "development", publicKey: ConnectionCrypto.hex(publicKey),
            invitationId: String(repeating: "a", count: 64), issuedAt: 1, expiresAt: 2, relayOrigin: nil))
        XCTAssertTrue(connection.valid)
        StubMacProtocol.reset(mac: key, macId: macId, lost: lost)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [StubMacProtocol.self]
        return (connection, URLSession(configuration: config, delegate: RefuseRedirects(), delegateQueue: nil))
    }
    func testTheRevokeGoesOverTheSeededRouteWithoutDiscovery() async throws {
        let (connection, session) = try stubMac(); defer { session.invalidateAndCancel() }
        let probes = Probes()
        let transport = DirectTransport(connection: connection, session: session, nearbyOrigins: [],
            discover: { _ in await probes.record(URL(string: "http://discovery.invalid")!); return nil },
            probe: { url in await probes.record(url); return true },
            endpoint: URL(string: "http://10.0.0.5:4321"))
        let (_, response) = try await transport.requestOnce(path: "/api/v3/lock", method: "POST", body: Data("{}".utf8), cookie: nil)
        XCTAssertEqual(response.statusCode, 200)
        XCTAssertEqual(StubMacProtocol.sent, ["10.0.0.5 /api/v3/lock"])
        let order = await probes.order
        XCTAssertTrue(order.isEmpty, "The seeded fast path neither discovers nor probes.")
    }
    func testARefusedSeededRouteFallsBackToDiscoveryAndRevokesOnce() async throws {
        let (connection, session) = try stubMac(); defer { session.invalidateAndCancel() }
        let probes = Probes()
        let found = try XCTUnwrap(URL(string: "http://10.0.0.9:4321"))
        // Loopback port 1 refuses for real: no stub answers 127.0.0.1.
        let transport = DirectTransport(connection: connection, session: session, nearbyOrigins: [],
            discover: { _ in await probes.record(URL(string: "http://discovery.invalid")!); return found },
            probe: { url in await probes.record(url); return url == found },
            endpoint: URL(string: "http://127.0.0.1:1"))
        let (_, response) = try await transport.requestOnce(path: "/api/v3/lock", method: "POST", body: Data("{}".utf8), cookie: nil)
        XCTAssertEqual(response.statusCode, 200)
        XCTAssertEqual(StubMacProtocol.sent, ["10.0.0.9 /api/v3/lock"], "Exactly one revoke, over the discovered route.")
        let order = await probes.order
        XCTAssertEqual(order, ["http://discovery.invalid", found.absoluteString])
        let current = await transport.currentEndpoint
        XCTAssertEqual(current, found)
    }
    func testARevokeThatMayHaveArrivedIsNeverResent() async throws {
        let (connection, session) = try stubMac(losing: ["10.0.0.5"]); defer { session.invalidateAndCancel() }
        let probes = Probes()
        let transport = DirectTransport(connection: connection, session: session, nearbyOrigins: [],
            discover: { _ in await probes.record(URL(string: "http://discovery.invalid")!); return URL(string: "http://10.0.0.9:4321") },
            probe: { url in await probes.record(url); return true },
            endpoint: URL(string: "http://10.0.0.5:4321"))
        do {
            _ = try await transport.requestOnce(path: "/api/v3/lock", method: "POST", body: Data("{}".utf8), cookie: nil)
            XCTFail("A connection lost after the request left must fail.")
        } catch {
            XCTAssertEqual((error as? URLError)?.code, .networkConnectionLost)
        }
        XCTAssertEqual(StubMacProtocol.sent, ["10.0.0.5 /api/v3/lock"], "The revoke reached the Mac once and is not resent.")
        let order = await probes.order
        XCTAssertTrue(order.isEmpty, "An ambiguous failure never falls back to discovery.")
        let current = await transport.currentEndpoint
        XCTAssertNil(current)
    }
    func testOnlyFailuresBeforeTheRequestLeftCountAsNeverSent() {
        for code in [URLError.Code.cannotConnectToHost, .cannotFindHost, .notConnectedToInternet, .secureConnectionFailed] {
            XCTAssertTrue(DirectTransport.neverSent(URLError(code)), "\(code)")
        }
        for code in [URLError.Code.timedOut, .networkConnectionLost, .cancelled, .badServerResponse, .unknown] {
            XCTAssertFalse(DirectTransport.neverSent(URLError(code)), "\(code)")
        }
        XCTAssertFalse(DirectTransport.neverSent(PhoneError.unavailable))
        XCTAssertFalse(DirectTransport.neverSent(CancellationError()))
    }
    func testDecodedLengthBoundsThePassedThroughBody() {
        XCTAssertEqual(DirectTransport.decodedLength(""), 0)
        XCTAssertEqual(DirectTransport.decodedLength("YQ=="), 1)
        XCTAssertEqual(DirectTransport.decodedLength("YWI="), 2)
        XCTAssertEqual(DirectTransport.decodedLength("YWJj"), 3)
        XCTAssertNil(DirectTransport.decodedLength("YWJ"))
        let limit = Data(repeating: 7, count: 2 * 1024 * 1024).base64EncodedString()
        XCTAssertEqual(DirectTransport.decodedLength(limit), 2 * 1024 * 1024)
    }
}

/// A stand-in Mac: it opens each sealed request with the Mac key, logs its host and path,
/// and answers 200, or fails a chosen host the way a connection lost after sending does.
final class StubMacProtocol: URLProtocol {
    private static let lock = NSLock()
    private static var mac: P256.KeyAgreement.PrivateKey?
    private static var macId = ""
    private static var lost: Set<String> = []
    private static var log: [String] = []
    static var sent: [String] { lock.withLock { log } }
    static func reset(mac: P256.KeyAgreement.PrivateKey, macId: String, lost: Set<String>) {
        lock.withLock { self.mac = mac; self.macId = macId; self.lost = lost; log = [] }
    }
    override class func canInit(with request: URLRequest) -> Bool { request.url?.host != "127.0.0.1" }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        do {
            let (response, data) = try answer()
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }
    override func stopLoading() {}
    private func answer() throws -> (HTTPURLResponse, Data) {
        let (mac, macId, lost) = Self.lock.withLock { (Self.mac, Self.macId, Self.lost) }
        guard let mac, let url = request.url, let host = url.host else { throw URLError(.badURL) }
        let packet = try JSONDecoder().decode(EncryptedPacket.self, from: Self.body(of: request))
        let plaintext = try ConnectionCrypto.open(packet, key: mac, macId: macId, requestId: packet.requestId,
                                                  expectedSender: packet.senderKey, direction: "request")
        let path = (try JSONSerialization.jsonObject(with: plaintext) as? [String: Any])?["path"] as? String ?? ""
        Self.lock.withLock { Self.log.append("\(host) \(path)") }
        if lost.contains(host) { throw URLError(.networkConnectionLost) }
        let reply = TunnelResponse(version: 5, status: 200, body: Data("{}".utf8).base64EncodedString(),
                                   contentType: "application/json", setCookie: nil)
        let sealed = try ConnectionCrypto.seal(JSONEncoder().encode(reply), key: mac, recipient: packet.senderKey,
                                               macId: macId, requestId: packet.requestId, direction: "response")
        let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "application/json"])!
        return (response, try JSONEncoder().encode(sealed))
    }
    private static func body(of request: URLRequest) -> Data {
        if let body = request.httpBody { return body }
        guard let stream = request.httpBodyStream else { return Data() }
        stream.open(); defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while true {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count <= 0 { break }
            data.append(buffer, count: count)
        }
        return data
    }
}

final class ChunkedBodyProtocol: URLProtocol {
    static var chunks: [Data] = []
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        for chunk in Self.chunks { client?.urlProtocol(self, didLoad: chunk) }
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

final class BoundedBodyTests: XCTestCase {
    func testWholeChunksAreKeptUpToTheCapAndRefusedPastIt() async throws {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [ChunkedBodyProtocol.self]
        let session = URLSession(configuration: config, delegate: RefuseRedirects(), delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        ChunkedBodyProtocol.chunks = [Data(repeating: 1, count: 3000), Data(repeating: 2, count: 3000)]
        let request = URLRequest(url: URL(string: "https://fixture.invalid/body")!)
        let (data, response) = try await BoundedBody.read(session, request, cap: 6000)
        XCTAssertEqual(data, Data(repeating: 1, count: 3000) + Data(repeating: 2, count: 3000))
        XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
        do {
            _ = try await BoundedBody.read(session, request, cap: 5999)
            XCTFail("A body past its cap must be refused.")
        } catch PhoneError.unavailable { }
    }
}
