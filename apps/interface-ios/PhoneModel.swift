import SwiftUI
import LocalAuthentication

// Face ID can make the scene inactive and active again. That is not a new
// request to unlock. Only a completed session may re-arm automatic unlock when
// the app goes into the background; failed/cancelled attempts require a tap.
struct UnlockLifecycle {
    private var automaticUnlockPending = true
    mutating func suppressAutomaticUnlock() { automaticUnlockPending = false }
    mutating func enteredBackground(hadSession: Bool) { automaticUnlockPending = hadSession }
    mutating func takeAutomaticUnlock(stored: Bool, busy: Bool, unlocked: Bool) -> Bool {
        guard automaticUnlockPending else { return false }
        automaticUnlockPending = false
        return stored && !busy && !unlocked
    }
}

@MainActor
final class PhoneModel: ObservableObject {
    @Published var origin: URL?
    @Published var cookie: HTTPCookie?
    @Published var workspaces: [PhoneWorkspace] = []
    /// The Mac's Threads, needs you first (phone Threads v1); nil from a Mac without them.
    @Published var threads: [PhoneThread]?
    @Published var inbox: PhoneInbox?
    @Published var inboxError: String?
    @Published var error: String?
    @Published var busy = false
    @Published var waiting: String?
    @Published var stored = false
    @Published var online = false
    @Published var needsMacApproval = false
    @Published var connection: SavedMacConnection?
    @Published var routeDescription: String?
    private var invitationId: String?
    private var unlockLifecycle = UnlockLifecycle()
    private var attempt = UUID()
    private var task: Task<Void, Never>?
    private var expiry: Task<Void, Never>?
    private var authentication: LAContext?
    private var client: MacClient?
    var sessionClient: MacClient? { client }

    init() {
        connection = SavedMacConnection.load()
        // A typed Tailscale address (`interface.macOrigin.v1`) is no longer
        // read: that route was retired on 2026-10-14. Pair by QR code instead.
        origin = connection?.virtualOrigin
        stored = origin.map { UserDefaults.standard.bool(forKey: "interface.trusted.\($0.absoluteString)") } ?? false
        needsMacApproval = stored && !(origin.map { UserDefaults.standard.bool(forKey: "interface.approved.\($0.absoluteString)") } ?? false)
    }
    func sceneChanged(_ phase: ScenePhase) {
        if phase == .background {
            let hadSession = cookie != nil
            lock()
            unlockLifecycle.enteredBackground(hadSession: hadSession)
        }
        if phase == .active && unlockLifecycle.takeAutomaticUnlock(stored: stored, busy: busy, unlocked: cookie != nil) {
            connect()
        }
    }
    func pair(_ invitation: PairingInvitation) {
        lock()
        let saved = SavedMacConnection(invitation)
        guard saved.valid else { error = PairingInvitationError.identity.localizedDescription; return }
        connection = saved; origin = saved.virtualOrigin; invitationId = invitation.invitationId
        UserDefaults.standard.set(try? JSONEncoder().encode(saved), forKey: "interface.macConnection.v5")
        NearbyRoutesV1.save(macId: saved.macId, origins: invitation.nearbyOrigins)
        stored = UserDefaults.standard.bool(forKey: "interface.trusted.\(saved.virtualOrigin.absoluteString)")
        needsMacApproval = !UserDefaults.standard.bool(forKey: "interface.approved.\(saved.virtualOrigin.absoluteString)")
        error = nil
        connect(requestApproval: true)
    }
    func connect(requestApproval: Bool = false) {
        guard !busy, cookie == nil, let origin else { return }
        unlockLifecycle.suppressAutomaticUnlock()
        busy = true; error = nil
        let nonce = UUID(); attempt = nonce
        let context = LAContext(); context.localizedReason = "Unlock your Interface sessions"
        context.touchIDAuthenticationAllowableReuseDuration = 0
        authentication = context
        let client = MacClient(origin: origin, connection: connection, invitationId: invitationId); self.client = client
        task = Task {
            defer { if attempt == nonce { busy = false; authentication = nil } }
            do {
                var existing: DeviceCredential?
                var ownerVerified = false
                if stored {
                    do {
                        existing = try await DeviceKeychain.read(origin: origin, context: context)
                    } catch PhoneError.identityMissing {
                        // These flags came from another iPhone (a restore or transfer); its
                        // device-only identity did not. Only a deliberate pairing replaces it,
                        // and the new identity still needs the Mac's approval.
                        try check(nonce)
                        forgetTrustFlags(origin)
                        guard requestApproval else { throw PhoneError.identityMissing }
                        ownerVerified = true
                    }
                }
                try check(nonce)
                let credential: DeviceCredential
                if let existing {
                    credential = existing
                } else {
                    // Authenticate before creating trust, so pairing itself has the same device-owner gate.
                    if !ownerVerified {
                        guard try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: "Protect this Mac connection with Face ID or your device passcode") else { throw PhoneError.locked }
                        try check(nonce)
                    }
                    credential = try DeviceCredential.create()
                    try DeviceKeychain.save(credential, origin: origin)
                    stored = true
                    UserDefaults.standard.set(true, forKey: "interface.trusted.\(origin.absoluteString)")
                }
                try check(nonce)
                if requestApproval || !UserDefaults.standard.bool(forKey: "interface.approved.\(origin.absoluteString)") {
                    var reply = try await client.enrollment(credential, create: true)
                    try check(nonce)
                    waiting = reply.verification
                    let deadline = Date().addingTimeInterval(300)
                    while !reply.approved {
                        guard Date() < deadline else { throw PhoneError.expired }
                        try await Task.sleep(for: .seconds(2))
                        try check(nonce)
                        reply = try await client.enrollment(credential, create: false)
                    }
                    try check(nonce)
                    waiting = nil
                    UserDefaults.standard.set(true, forKey: "interface.approved.\(origin.absoluteString)")
                    needsMacApproval = false
                }
                let grant = try await client.unlock(credential)
                try check(nonce)
                cookie = grant
                expiry = Task { try? await Task.sleep(for: .seconds(14 * 60)); if !Task.isCancelled { lock() } }
                await refresh()
                await client.refreshRoutes(cookie: grant)
            } catch {
                if attempt == nonce && !Task.isCancelled {
                    waiting = nil
                    if case PhoneError.refused = error { requireMacApproval() }
                    if case PhoneError.identityMissing = error { requireMacApproval() }
                    self.error = error.localizedDescription
                }
            }
        }
    }
    private func check(_ nonce: UUID) throws { try Task.checkCancellation(); guard attempt == nonce else { throw CancellationError() } }
    func refresh() async {
        guard let client, let cookie else { return }
        let nonce = attempt
        do {
            let result = try await client.workspaces(cookie: cookie)
            // Threads are optional: a failed read keeps the last list and never
            // marks the Mac offline on its own.
            let threadList = try? await client.threads(cookie: cookie)
            do {
                let next = try await client.inbox(cookie: cookie)
                if attempt == nonce { if inbox != next { inbox = next }; inboxError = nil }
            } catch {
                if attempt == nonce { inboxError = "The inbox could not refresh. Showing the last read; check your Mac before acting." }
            }
            let route = await client.direct?.routeName
            guard attempt == nonce else { return }
            // Each assignment publishes; an unchanged roster must not re-render the stack.
            if workspaces != result { workspaces = result }
            if let threadList, threads != threadList { threads = threadList }
            if !online { online = true }
            if error != nil { error = nil }
            if routeDescription != route { routeDescription = route }
        } catch {
            guard attempt == nonce && !Task.isCancelled else { return }
            online = false
            if case PhoneError.refused = error { lock(); requireMacApproval() }
            self.error = error.localizedDescription
        }
    }
    func lock() {
        unlockLifecycle.suppressAutomaticUnlock()
        let previousCookie = cookie; let previousOrigin = origin; let previousConnection = connection
        let previousRoute = client?.direct
        attempt = UUID(); task?.cancel(); expiry?.cancel(); authentication?.invalidate(); authentication = nil
        client?.cancel(); client = nil
        cookie = nil; workspaces = []; threads = nil; inbox = nil; inboxError = nil; waiting = nil; busy = false; online = false; routeDescription = nil
        if let previousCookie, let previousOrigin {
            // A best-effort immediate revoke over the route the session already proved, so it
            // can leave before iOS suspends the app instead of waiting on discovery. If that
            // route provably never received it, discovery finds another exactly as before; a
            // revoke that may have arrived is never resent. Offline grants still expire after
            // fifteen minutes.
            Task {
                let route = await previousRoute?.currentEndpoint
                let revoke = MacClient(origin: previousOrigin, connection: previousConnection, endpoint: route)
                await revoke.revoke(cookie: previousCookie)
                revoke.cancel()
            }
        }
    }
    private func forgetTrustFlags(_ origin: URL) {
        UserDefaults.standard.removeObject(forKey: "interface.trusted.\(origin.absoluteString)")
        UserDefaults.standard.removeObject(forKey: "interface.approved.\(origin.absoluteString)")
        stored = false
    }
    private func requireMacApproval() {
        guard let origin else { return }
        UserDefaults.standard.set(false, forKey: "interface.approved.\(origin.absoluteString)")
        needsMacApproval = true
    }
    func changeMac() { lock(); origin = nil; connection = nil; invitationId = nil; error = nil
        UserDefaults.standard.removeObject(forKey: "interface.macConnection.v5")
    }
    func forget() {
        guard let origin else { return }
        lock()
        do {
            try DeviceKeychain.forget(origin: origin)
            UserDefaults.standard.removeObject(forKey: "interface.trusted.\(origin.absoluteString)")
            UserDefaults.standard.removeObject(forKey: "interface.approved.\(origin.absoluteString)")
            stored = false; needsMacApproval = false; error = nil
        } catch { self.error = error.localizedDescription }
    }
}
