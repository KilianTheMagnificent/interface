import SwiftUI

@main
struct InterfacePhoneApp: App {
    var body: some Scene {
        WindowGroup {
            #if DEBUG && targetEnvironment(simulator)
            if ProcessInfo.processInfo.arguments.contains("--design-preview") {
                PhoneDesignPreview().preferredColorScheme(.dark)
            } else { PhoneRootView().preferredColorScheme(.dark) }
            #else
            PhoneRootView().preferredColorScheme(.dark)
            #endif
        }
    }
}

/// The pinned Mac's own pages. Typed addresses (the retired Tailscale
/// route, 2026-10-14) are no longer parsed; a Mac is paired by QR code.
enum MacAddress {
    static func contains(_ url: URL, origin: URL) -> Bool {
        url.scheme == origin.scheme && url.host?.lowercased() == origin.host?.lowercased()
            && (url.port ?? (url.scheme == "https" ? 443 : 80)) == (origin.port ?? (origin.scheme == "https" ? 443 : 80))
            && url.user == nil && url.password == nil
    }
}

// Shared with the phone reader: warm charcoal, quiet borders, the emerald Fold accent.
enum PhoneStyle {
    static let background = Color(red: 19/255, green: 22/255, blue: 21/255)
    static let surface = Color(red: 28/255, green: 32/255, blue: 30/255)
    static let elevated = Color(red: 36/255, green: 42/255, blue: 39/255)
    static let text = Color(red: 239/255, green: 242/255, blue: 237/255)
    static let secondary = Color(red: 158/255, green: 170/255, blue: 162/255)
    static let accent = Color(red: 165/255, green: 219/255, blue: 182/255)
    static let border = Color.white.opacity(0.08)
}

struct PhoneRootView: View {
    @StateObject private var model = PhoneModel()
    @Environment(\.scenePhase) private var phase
    @State private var forgetConfirmation = false
    @State private var scanning = false
    @State private var openSessions = 0
    var body: some View {
        ZStack {
            NavigationStack {
                Group {
                    if let origin = model.origin, let cookie = model.cookie {
                        PhoneLibraryView(workspaces: model.workspaces, online: model.online, error: model.error ?? model.inboxError, routeDescription: model.routeDescription,
                                         threads: model.threads, inbox: model.inbox) {
                            await model.refresh()
                        }
                        .navigationDestination(for: PhoneTerminal.self) { terminal in
                            SessionDetail(origin: origin, cookie: cookie, terminal: terminal, client: model.sessionClient)
                                .onAppear { openSessions += 1 }
                                .onDisappear { openSessions = max(0, openSessions - 1) }
                        }
                        .navigationDestination(for: PhoneThread.self) { thread in
                            ThreadDetailView(thread: thread, cookie: cookie, client: model.sessionClient)
                        }
                        .navigationDestination(for: PhoneInboxItem.self) { item in
                            if let thread = item.thread {
                                ThreadDetailView(thread: thread, cookie: cookie, client: model.sessionClient, canSend: item.canSend, taskDetail: item.detail)
                            }
                        }
                    } else { connectionScreen }
                }
                .background(PhoneStyle.background)
                .navigationBarTitleDisplayMode(.inline)
                .toolbarBackground(PhoneStyle.background, for: .navigationBar)
                .toolbarBackground(.visible, for: .navigationBar)
                .toolbar {
                    ToolbarItem(placement: .principal) { PhoneWordmark().fixedSize() }
                    if model.origin != nil {
                        ToolbarItem(placement: .topBarTrailing) {
                            Menu {
                                if model.cookie != nil { Button("Lock Interface", systemImage: "lock", action: model.lock) }
                                Button("Change Mac", systemImage: "network", action: model.changeMac)
                                if model.stored { Button("Forget this connection", systemImage: "trash", role: .destructive) { forgetConfirmation = true } }
                            } label: {
                                Image(systemName: "ellipsis").font(.system(size: 19, weight: .medium)).frame(width: 44, height: 44)
                            }.accessibilityLabel("Connection options")
                        }
                    }
                }
            }
            .id(model.cookie != nil) // Lock destroys the entire navigation stack and every session web view.
            .tint(PhoneStyle.text)
            if phase != .active {
                PhoneStyle.background.ignoresSafeArea()
                    .overlay { Image(systemName: "lock.fill").font(.largeTitle).foregroundStyle(PhoneStyle.secondary) }
                    .accessibilityLabel("Interface locked")
            }
        }
        .confirmationDialog("Forget this Mac on this iPhone?", isPresented: $forgetConfirmation, titleVisibility: .visible) {
            Button("Forget connection", role: .destructive, action: model.forget)
        } message: { Text("You’ll need Mac approval to connect again. You can also revoke this iPhone in Interface Settings on your Mac.") }
        .onChange(of: phase, initial: true) { _, next in model.sceneChanged(next) }
        .task(id: model.cookie != nil) {
            var idle: Duration = .zero
            while model.cookie != nil && !Task.isCancelled {
                try? await Task.sleep(for: .seconds(3))
                guard !Task.isCancelled else { break }
                idle += .seconds(3)
                // An open session view polls the roster itself. A slower check here
                // still notices a Mac-side revocation and locks the whole stack.
                if openSessions > 0 && idle < .seconds(30) { continue }
                idle = .zero
                await model.refresh()
            }
        }
    }

    private var connectionScreen: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Image("InterfaceLogo").resizable().frame(width: 64, height: 64)
                    .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
                    .padding(.top, 48).padding(.bottom, 28).accessibilityHidden(true)
                Text(model.stored ? "Pick up where\nyou left off." : "A little closer\nto your work.")
                    .font(.system(.largeTitle, weight: .semibold)).tracking(-1.2).fixedSize(horizontal: false, vertical: true)
                Text("Your Mac’s sessions.\nA quieter space to follow along.")
                    .font(.body).foregroundStyle(PhoneStyle.secondary).lineSpacing(5).padding(.top, 16).padding(.bottom, 36)
                if model.connection == nil {
                    primaryButton("Scan Mac QR code", symbol: "qrcode.viewfinder") { scanning = true }
                    Text("Pair once in Interface Settings on your Mac. No VPN or address to type.")
                        .font(.footnote).foregroundStyle(PhoneStyle.secondary).padding(.top, 12).padding(.bottom, 28)
                }
                if let origin = model.origin {
                    HStack(spacing: 12) {
                        Image(systemName: "desktopcomputer").font(.title3).foregroundStyle(PhoneStyle.accent)
                        VStack(alignment: .leading, spacing: 5) {
                            Text(model.connection?.macName ?? "Your Mac").font(.subheadline.weight(.medium))
                            Text(model.connection.map { $0.profile == "production" ? "Production sessions" : "Development sessions" } ?? origin.host ?? "Private connection")
                                .font(.caption).foregroundStyle(PhoneStyle.secondary).lineLimit(2)
                        }
                        Spacer(minLength: 0)
                        Image(systemName: "lock.shield").foregroundStyle(PhoneStyle.secondary)
                    }.padding(18).background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 18))
                    .padding(.bottom, 20)
                    if let verification = model.waiting {
                        VStack(alignment: .leading, spacing: 16) {
                            Text("One quick check on your Mac").font(.headline)
                            Text("In Interface Settings, open iPhone remote access and approve the matching number.")
                                .font(.subheadline).foregroundStyle(PhoneStyle.secondary).lineSpacing(4)
                            Text(verification).font(.system(.largeTitle, design: .monospaced)).tracking(3)
                                .foregroundStyle(PhoneStyle.accent).minimumScaleFactor(0.65).lineLimit(1)
                                .accessibilityLabel("Verification number \(verification)")
                            HStack { ProgressView().tint(PhoneStyle.accent); Text("Waiting for approval").font(.footnote).foregroundStyle(PhoneStyle.secondary); Spacer() }
                            Button("Cancel", action: model.lock).frame(minHeight: 44)
                        }.padding(20).background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 20))
                    } else {
                        primaryButton(model.needsMacApproval ? "Request Mac approval" : model.stored ? "Unlock with Face ID" : "Connect this iPhone", symbol: model.needsMacApproval ? "desktopcomputer" : model.stored ? "faceid" : "arrow.right") {
                            if model.connection != nil && model.needsMacApproval { scanning = true }
                            else { model.connect(requestApproval: model.needsMacApproval) }
                        }.disabled(model.busy)
                        if model.busy { ProgressView("Unlocking…").font(.footnote).padding(.top, 18) }
                        Text(model.stored ? "Face ID or your passcode keeps your sessions private." : "Approve once on your Mac. Unlock with Face ID next time.")
                            .font(.footnote).foregroundStyle(PhoneStyle.secondary).lineSpacing(4).padding(.top, 16)
                        if model.error != nil && model.stored && !model.busy && !model.needsMacApproval {
                            Button("Request Mac approval again") {
                                if model.connection != nil { scanning = true } else { model.connect(requestApproval: true) }
                            }.frame(minHeight: 44).padding(.top, 8)
                        }
                    }
                }
                if let error = model.error {
                    Label(error, systemImage: "exclamationmark.circle").font(.footnote).foregroundStyle(PhoneStyle.accent)
                        .padding(16).frame(maxWidth: .infinity, alignment: .leading)
                        .background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 14)).padding(.top, 20)
                        .accessibilityIdentifier("connection-error")
                }
                Label("Encrypted from your iPhone to your Mac", systemImage: "lock.fill")
                    .font(.caption).foregroundStyle(PhoneStyle.secondary).padding(.top, 36)
            }.foregroundStyle(PhoneStyle.text).padding(.horizontal, 26).padding(.bottom, 32).frame(maxWidth: 600)
                .frame(maxWidth: .infinity)
        }.scrollDismissesKeyboard(.interactively)
            .sheet(isPresented: $scanning) { PairingScannerSheet(pair: model.pair) }
    }
    private func primaryButton(_ title: String, symbol: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 12) { Image(systemName: symbol); Text(title) }
                .font(.headline).padding(18).frame(maxWidth: .infinity)
        }.buttonStyle(PhonePressStyle()).foregroundStyle(PhoneStyle.background)
            .background(PhoneStyle.accent, in: RoundedRectangle(cornerRadius: 16))
    }
}

struct PhoneWordmark: View {
    var body: some View {
        HStack(spacing: 9) {
            Image("InterfaceLogo").resizable().frame(width: 25, height: 25).clipShape(RoundedRectangle(cornerRadius: 7)).accessibilityHidden(true)
            Text("Interface").font(.system(size: 17, weight: .semibold)).tracking(-0.4)
        }.foregroundStyle(PhoneStyle.text).accessibilityElement(children: .combine)
    }
}
struct PhonePressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.opacity(configuration.isPressed ? 0.62 : 1)
            .contentShape(Rectangle())
    }
}

/// Sessions grouped as the Mac sidebar groups them. Roster v3 names each session's
/// project; an older Mac leaves the folder Rust authorized for it. Projects with a
/// session that needs you come first, then by name with "Other" (chats outside any
/// repository) last; inside one, sessions that need you come first (roster v4,
/// decided on the Mac), then live sessions, then the most recently changed.
struct PhoneProject: Identifiable, Equatable {
    let id: String
    let name: String
    let terminals: [PhoneTerminal]

    static func group(_ workspaces: [PhoneWorkspace]) -> [PhoneProject] {
        var buckets: [PhoneProjectKey: [PhoneTerminal]] = [:]
        var seen = Set<String>()
        for workspace in workspaces {
            for terminal in workspace.terminals where seen.insert(terminal.id).inserted {
                buckets[terminal.project ?? .folder(terminal.root ?? workspace.root), default: []].append(terminal)
            }
        }
        return buckets.map { key, terminals in
            let terminals = terminals.sorted(by: comesBefore)
            switch key {
            case .other: return PhoneProject(id: otherId, name: "Other", terminals: terminals)
            case .folder(let root):
                let path = normalized(root)
                return PhoneProject(id: path, name: name(of: path), terminals: terminals)
            }
        }
        // Two folder spellings of one path merge after normalizing.
        .reduce(into: [PhoneProject]()) { merged, project in
            if let index = merged.firstIndex(where: { $0.id == project.id }) {
                merged[index] = PhoneProject(id: project.id, name: project.name,
                                             terminals: (merged[index].terminals + project.terminals).sorted(by: comesBefore))
            } else { merged.append(project) }
        }
        .sorted {
            if $0.needsYou != $1.needsYou { return $0.needsYou }
            if ($0.id == otherId) != ($1.id == otherId) { return $1.id == otherId }
            let order = $0.name.localizedStandardCompare($1.name)
            return order == .orderedSame ? $0.id < $1.id : order == .orderedAscending
        }
    }
    /// Matches the sidebar's key for its "Other" group, which no real path can equal.
    static let otherId = "\u{0}other"
    var needsYou: Bool { terminals.contains(where: \.needsYou) }
    private static func normalized(_ root: String) -> String {
        root.count > 1 && root.hasSuffix("/") ? String(root.dropLast()) : root
    }
    private static func name(of root: String) -> String {
        root.split(separator: "/").last.map(String.init) ?? root
    }
    private static func rank(_ state: String) -> Int {
        ["starting": 0, "running": 1, "failed": 2][state] ?? 3
    }
    private static func comesBefore(_ left: PhoneTerminal, _ right: PhoneTerminal) -> Bool {
        if left.needsYou != right.needsYou { return left.needsYou }
        if rank(left.state) != rank(right.state) { return rank(left.state) < rank(right.state) }
        // Rust writes one canonical UTC format, so the strings order by time.
        let leftTime = left.updatedAt ?? "", rightTime = right.updatedAt ?? ""
        if leftTime != rightTime { return leftTime > rightTime }
        return left.displayTitle.localizedStandardCompare(right.displayTitle) == .orderedAscending
    }
}

/// The Mac's two places for agents on the phone: Code panes and Threads.
enum PhoneSurface: Hashable { case code, threads }

// Presentation only. Session identity, each session's folder, what needs you and
// the Threads order come from Rust.
struct PhoneLibraryView: View {
    let workspaces: [PhoneWorkspace]
    let online: Bool
    let error: String?
    var routeDescription: String? = nil
    /// The Mac's Threads; nil from a Mac without phone Threads.
    var threads: [PhoneThread]? = nil
    var inbox: PhoneInbox? = nil
    var initialSurface: PhoneSurface = .code
    let refresh: () async -> Void
    @State private var search = ""
    @State private var selectedProject: String?
    @State private var chosenSurface: PhoneSurface?
    private var surface: PhoneSurface { threads == nil && inbox == nil ? .code : chosenSurface ?? initialSurface }
    private var sessionsNeedingYou: Int { projects.reduce(0) { $0 + $1.terminals.filter(\.needsYou).count } }
    private var threadsNeedingYou: Int { inbox?.items.filter { $0.status == "needsYou" }.count ?? threads?.filter(\.needsYou).count ?? 0 }
    private var sessionCount: Int { workspaces.reduce(0) { $0 + $1.terminals.count } }
    private var projects: [PhoneProject] { PhoneProject.group(workspaces) }
    private var groups: [PhoneProject] {
        // Workspace names stay searchable: the Mac still shows them.
        let workspaceNames = Dictionary(workspaces.flatMap { workspace in workspace.terminals.map { ($0.id, workspace.name) } },
                                        uniquingKeysWith: { first, _ in first })
        return projects.compactMap { project in
            guard selectedProject == nil || selectedProject == project.id else { return nil }
            let rows = project.terminals.filter {
                search.isEmpty || "\(project.name) \(workspaceNames[$0.id] ?? "") \($0.title) \($0.cwd) \($0.provider)".localizedCaseInsensitiveContains(search)
            }
            return rows.isEmpty ? nil : PhoneProject(id: project.id, name: project.name, terminals: rows)
        }
    }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Circle().fill(online ? PhoneStyle.accent : PhoneStyle.secondary).frame(width: 6, height: 6)
                    Text(online ? (routeDescription?.uppercased() ?? "CONNECTED TO YOUR MAC") : "MAC UNAVAILABLE")
                        .font(.system(size: 10, weight: .semibold)).tracking(1.3)
                }.foregroundStyle(PhoneStyle.secondary).padding(.top, 24)
                HStack(alignment: .firstTextBaseline) {
                    Text("Your sessions").font(.system(.largeTitle, weight: .semibold)).tracking(-1.2)
                    Spacer(minLength: 8)
                    Text("\(surface == .code ? sessionCount : inbox?.items.count ?? threads?.count ?? 0)").font(.title3.monospacedDigit()).foregroundStyle(PhoneStyle.secondary)
                }.padding(.top, 12).padding(.bottom, 8)
                let waiting = surface == .code ? sessionsNeedingYou : threadsNeedingYou
                Group {
                    if waiting > 0 {
                        Label(waiting == 1 ? "1 needs you" : "\(waiting) need you", systemImage: "exclamationmark.circle.fill")
                            .font(.subheadline.weight(.semibold)).foregroundStyle(PhoneStyle.attention)
                    } else {
                        Text("Keep the conversation going.").font(.subheadline).foregroundStyle(PhoneStyle.secondary)
                    }
                }.padding(.bottom, threads == nil ? 24 : 18)
                if threads != nil || inbox != nil {
                    Picker("Show", selection: Binding(get: { surface }, set: { chosenSurface = $0; search = "" })) {
                        Text(sessionsNeedingYou > 0 ? "Code · \(sessionsNeedingYou)" : "Code").tag(PhoneSurface.code)
                        Text(threadsNeedingYou > 0 ? "Inbox · \(threadsNeedingYou)" : "Inbox").tag(PhoneSurface.threads)
                    }.pickerStyle(.segmented).padding(.bottom, 18).accessibilityIdentifier("surface-picker")
                }
                HStack(spacing: 10) {
                    Image(systemName: "magnifyingglass").foregroundStyle(PhoneStyle.secondary)
                    TextField(surface == .code ? "Find a session" : "Find a thread", text: $search).font(.body).autocorrectionDisabled().submitLabel(.search)
                        .accessibilityIdentifier("session-search")
                    if !search.isEmpty {
                        Button { search = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(PhoneStyle.secondary).frame(width: 44, height: 44) }
                            .accessibilityLabel("Clear search")
                    }
                }.padding(.leading, 16).padding(.trailing, search.isEmpty ? 16 : 2).frame(minHeight: 52)
                    .background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 16))
                    .overlay(RoundedRectangle(cornerRadius: 16).stroke(PhoneStyle.border))
                if surface == .threads {
                    Color.clear.frame(height: 24)
                    if let error {
                        Label(error, systemImage: "wifi.exclamationmark").font(.footnote).foregroundStyle(PhoneStyle.secondary)
                            .padding(.bottom, 20)
                    }
                    if let inbox {
                        PhoneInboxListView(inbox: inbox, search: search)
                    } else {
                        Text("This Mac supports Threads only. Update Interface on the Mac to include Agent tasks.").font(.caption).foregroundStyle(PhoneStyle.secondary)
                        PhoneThreadListView(threads: threads ?? [], search: search)
                    }
                } else {
                    if projects.count > 1 {
                        ScrollView(.horizontal) {
                            HStack(spacing: 8) {
                                projectChip("All projects", id: nil)
                                ForEach(projects) { project in
                                    projectChip(project.name, id: project.id)
                                }
                            }.padding(.vertical, 16)
                        }.scrollIndicators(.hidden)
                    } else { Color.clear.frame(height: 24) }
                    if let error {
                        Label(error, systemImage: "wifi.exclamationmark").font(.footnote).foregroundStyle(PhoneStyle.secondary)
                            .padding(.bottom, 20)
                    }
                    LazyVStack(alignment: .leading, spacing: 28) {
                        ForEach(groups) { project in
                            VStack(alignment: .leading, spacing: 12) {
                                HStack(spacing: 8) {
                                    Image(systemName: "folder").font(.system(size: 12, weight: .medium))
                                    Text(project.name).font(.subheadline.weight(.semibold)).lineLimit(2)
                                    Spacer(minLength: 8)
                                    Text("\(project.terminals.count)").font(.caption.monospacedDigit())
                                }.foregroundStyle(PhoneStyle.secondary).padding(.horizontal, 4)
                                VStack(spacing: 0) {
                                    ForEach(Array(project.terminals.enumerated()), id: \.element.id) { index, terminal in
                                        if index > 0 { Rectangle().fill(PhoneStyle.border).frame(height: 1).padding(.leading, 66) }
                                        NavigationLink(value: terminal) { PhoneSessionRow(terminal: terminal) }
                                            .buttonStyle(PhonePressStyle()).accessibilityIdentifier("session-\(terminal.id)")
                                    }
                                }.background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 20))
                                    .overlay(RoundedRectangle(cornerRadius: 20).stroke(PhoneStyle.border))
                            }
                        }
                    }
                    if groups.isEmpty {
                        VStack(spacing: 14) {
                            Image(systemName: search.isEmpty ? "bubble.left.and.bubble.right" : "magnifyingglass").font(.system(size: 28)).foregroundStyle(PhoneStyle.accent)
                            Text(sessionCount == 0 ? "Ready when you are" : "No matching sessions").font(.title3.weight(.semibold))
                            Text(sessionCount == 0 ? "Open a session in Interface on your Mac.\nIt will be here when you need it." : "Try another search or project.")
                                .font(.subheadline).foregroundStyle(PhoneStyle.secondary).multilineTextAlignment(.center).lineSpacing(4)
                            if selectedProject != nil || !search.isEmpty {
                                Button("Show all sessions") { search = ""; selectedProject = nil }.frame(minHeight: 44).tint(PhoneStyle.accent)
                            }
                        }.padding(.vertical, 56).frame(maxWidth: .infinity)
                    }
                }
                if sessionCount > 0 || !(threads ?? []).isEmpty || !(inbox?.items ?? []).isEmpty {
                    Label(surface == .code ? "Sessions keep running on your Mac" : "Threads keep running on your Mac", systemImage: "desktopcomputer")
                        .font(.caption).foregroundStyle(PhoneStyle.secondary).frame(maxWidth: .infinity).padding(.top, 28)
                }
            }.padding(.horizontal, 22).padding(.bottom, 24).frame(maxWidth: 640).frame(maxWidth: .infinity)
        }.background(PhoneStyle.background).foregroundStyle(PhoneStyle.text)
            .scrollDismissesKeyboard(.interactively).refreshable { await refresh() }
            .onChange(of: projects.map(\.id)) { _, ids in
                if let selectedProject, !ids.contains(selectedProject) { self.selectedProject = nil }
            }
    }
    private func projectChip(_ title: String, id: String?) -> some View {
        Button { selectedProject = id } label: {
            Text(title).font(.subheadline.weight(selectedProject == id ? .semibold : .regular))
                .lineLimit(1).padding(.horizontal, 15).padding(.vertical, 10).frame(minHeight: 44)
                .foregroundStyle(selectedProject == id ? PhoneStyle.background : PhoneStyle.secondary)
                .background(selectedProject == id ? PhoneStyle.accent : PhoneStyle.surface, in: Capsule())
        }.buttonStyle(PhonePressStyle()).accessibilityAddTraits(selectedProject == id ? .isSelected : [])
    }
}

/// The desktop's provider marks (electron `ProviderLogo`) in their brand colours,
/// so a session reads the same on the phone as on the Mac. A shell or an unknown
/// profile keeps a plain terminal glyph, as it does on the desktop.
struct ProviderMark: View {
    let profileId: String?

    static let marks: [String: (asset: String, color: Color)] = {
        let claude = ("ProviderClaude", Color(red: 184/255, green: 79/255, blue: 49/255))
        let codex = ("ProviderCodex", Color(red: 18/255, green: 128/255, blue: 90/255))
        let grok = ("ProviderGrok", PhoneStyle.text)
        let qwen = ("ProviderQwen", Color(red: 97/255, green: 92/255, blue: 237/255))
        let antigravity = ("ProviderAntigravity", Color(red: 66/255, green: 133/255, blue: 244/255))
        let opencode = ("ProviderOpenCode", PhoneStyle.text)
        let omp = ("ProviderOMP", PhoneStyle.text)
        // The desktop's dark-theme Muse blue (#0082fb); the phone is always dark.
        let muse = ("ProviderMuse", Color(red: 0, green: 130/255, blue: 251/255))
        // Kimi's K is ink like Grok's; its blue dot is drawn over it (`accents`).
        let kimi = ("ProviderKimi", PhoneStyle.text)
        return ["claude": claude, "codex": codex, "grok": grok, "acp:grok": grok, "qwen": qwen,
                "acp:qwen": qwen, "gemini": antigravity, "acp:gemini": antigravity,
                "opencode": opencode, "acp:opencode": opencode, "omp": omp, "acp:omp": omp,
                "muse": muse, "acp:muse": muse, "kimi": kimi, "acp:kimi": kimi]
    }()

    /// A second-colour layer for a two-colour mark: Kimi's dot in Kimi blue (#1783ff).
    static let accents: [String: (asset: String, color: Color)] = {
        let kimiDot = ("ProviderKimiDot", Color(red: 23/255, green: 131/255, blue: 255/255))
        return ["kimi": kimiDot, "acp:kimi": kimiDot]
    }()

    var body: some View {
        if let profileId, let mark = Self.marks[profileId] {
            ZStack {
                Image(mark.asset).renderingMode(.template).resizable().scaledToFit()
                    .foregroundStyle(mark.color)
                if let accent = Self.accents[profileId] {
                    Image(accent.asset).renderingMode(.template).resizable().scaledToFit()
                        .foregroundStyle(accent.color)
                }
            }
            .frame(width: 18, height: 18)
        } else {
            Image(systemName: "terminal").font(.system(size: 17, weight: .medium))
                .foregroundStyle(PhoneStyle.secondary)
        }
    }
}

struct PhoneSessionRow: View {
    let terminal: PhoneTerminal
    @Environment(\.dynamicTypeSize) private var typeSize
    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            ProviderMark(profileId: terminal.launch?.profileId)
                .opacity(terminal.conversationSupported ? 1 : 0.55)
                .frame(width: 36, height: 36).background(PhoneStyle.elevated, in: RoundedRectangle(cornerRadius: 11))
                .overlay(alignment: .topTrailing) {
                    if terminal.needsYou {
                        Circle().fill(PhoneStyle.attention).frame(width: 9, height: 9)
                            .overlay(Circle().stroke(PhoneStyle.surface, lineWidth: 2)).offset(x: 3, y: -3)
                    }
                }
            VStack(alignment: .leading, spacing: 9) {
                Text(terminal.displayTitle).font(.system(.body, weight: .medium)).tracking(-0.25).lineLimit(typeSize.isAccessibilitySize ? nil : 2)
                    .multilineTextAlignment(.leading).foregroundStyle(PhoneStyle.text)
                HStack(spacing: 6) {
                    // Decided on the Mac (roster v4); the session's own request shows inside it.
                    if terminal.needsYou { NeedsYouLabel(); Text("·").font(.caption).foregroundStyle(PhoneStyle.secondary) }
                    Text("\(terminal.provider) · \(terminal.conversationSupported ? terminal.state == "running" ? "Open on Mac" : terminal.state.capitalized : "View on Mac")")
                        .font(.caption).foregroundStyle(PhoneStyle.secondary).multilineTextAlignment(.leading)
                }
            }.frame(maxWidth: .infinity, alignment: .leading)
            Image(systemName: "chevron.right").font(.system(size: 10, weight: .semibold))
                .foregroundStyle(PhoneStyle.secondary).padding(.top, 12)
        }.padding(18).frame(minHeight: 94).contentShape(Rectangle())
            .accessibilityElement(children: .combine)
    }
}

private struct SessionDetail: View {
    let origin: URL
    let cookie: HTTPCookie
    let terminal: PhoneTerminal
    var client: MacClient? = nil
    @State private var error: String?
    @State private var loading = true
    @State private var reload = 0
    var body: some View {
        VStack(spacing: 0) {
            if let error {
                Label(error, systemImage: "wifi.exclamationmark").font(.footnote).foregroundStyle(PhoneStyle.secondary)
                    .padding(14).frame(maxWidth: .infinity, alignment: .leading).background(PhoneStyle.surface)
            }
            ZStack {
                SessionWebView(origin: origin, cookie: cookie, terminal: terminal, reload: reload, error: $error, loading: $loading, client: client)
                if loading {
                    PhoneStyle.background.overlay {
                        VStack(spacing: 16) { ProgressView().tint(PhoneStyle.accent); Text("Opening conversation").font(.subheadline).foregroundStyle(PhoneStyle.secondary) }
                    }
                }
            }
        }.background(PhoneStyle.background)
            .navigationTitle(terminal.displayTitle).navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(PhoneStyle.background, for: .navigationBar).toolbarBackground(.visible, for: .navigationBar)
            .toolbar { ToolbarItem(placement: .principal) {
                VStack(spacing: 4) {
                    Text(terminal.displayTitle).font(.subheadline.weight(.semibold)).lineLimit(1).foregroundStyle(PhoneStyle.text)
                    Text(terminal.provider).font(.system(size: 11, weight: .medium)).foregroundStyle(PhoneStyle.accent)
                }
            } }
            .toolbar { ToolbarItem(placement: .topBarTrailing) {
                Button("Reconnect", systemImage: "arrow.clockwise") { reload += 1 }
                    .frame(minWidth: 44, minHeight: 44)
            } }
    }
}

#if DEBUG && targetEnvironment(simulator)
// A render-only gallery of invented sessions. It never creates a Mac client,
// grants access, or changes the production connection; absent from phone builds.
struct PhoneDesignPreview: View {
    // Two workspaces whose sessions span three projects, as they do on the Mac;
    // one Claude session waits on a permission dialog.
    private let workspaces = [
        PhoneWorkspace(id: "review", name: "Code review and improvements", root: "/preview/Interface", terminals: [
            PhoneTerminal(id: "design", generation: 1, title: "A better home for your sessions", cwd: "/preview/Interface", state: "running", launch: .init(profileId: "claude"), root: "/preview/Interface", updatedAt: "2026-09-26T21:40:00Z"),
            PhoneTerminal(id: "sync", generation: 1, title: "Keep conversations in sync", cwd: "/preview/Interface", state: "running", launch: .init(profileId: "muse"), root: "/preview/Interface", updatedAt: "2026-09-26T21:55:00Z"),
            PhoneTerminal(id: "plan", generation: 1, title: "Plan the next release", cwd: "/preview/Kite App", state: "running", launch: .init(profileId: "claude"), root: "/preview/Kite App", updatedAt: "2026-09-26T20:10:00Z",
                          attention: PhoneAttention(version: 1, since: "2026-09-26T21:59:00Z"))]),
        PhoneWorkspace(id: "weekend", name: "Workspace 12", root: "/preview/Kite App", terminals: [
            PhoneTerminal(id: "tests", generation: 1, title: "Review the changes and test the new onboarding flow", cwd: "/preview/Kite App", state: "exited", launch: .init(profileId: "codex"), root: "/preview/Kite App", updatedAt: "2026-09-26T21:58:00Z"),
            PhoneTerminal(id: "site", generation: 1, title: "Tidy the sample landing page", cwd: "/preview/Sample Site", state: "running", launch: .init(profileId: "claude"), root: "/preview/Sample Site", updatedAt: "2026-09-26T19:30:00Z")])
    ]
    // The Mac's Threads in its order: needs you, working, then the rest.
    private static let threads = [
        PhoneThread(key: "claude#preview-1", engine: "claude", sessionId: "preview-1", title: "Migrate the settings store", project: "/preview/Interface",
                    status: "needsYou", section: "active", unread: true, lastActivity: "2026-09-26T21:59:00Z"),
        PhoneThread(key: "codex#preview-2", engine: "codex", sessionId: "preview-2", title: "Speed up the test gate", project: "/preview/Interface",
                    status: "working", section: "pinned", unread: false, lastActivity: "2026-09-26T21:58:00Z"),
        PhoneThread(key: "claude#preview-3", engine: "claude", sessionId: "preview-3", title: "Draft the onboarding email", project: "/preview/Kite App",
                    status: "idle", section: "active", unread: true, lastActivity: "2026-09-26T20:40:00Z"),
        PhoneThread(key: "claude#preview-4", engine: "claude", sessionId: "preview-4", title: "What is a good name for a bakery?", project: nil,
                    status: "idle", section: "active", unread: false, lastActivity: "2026-09-26T18:12:00Z")
    ]
    private static let inbox = PhoneInbox(version: 2, items: [
        PhoneInboxItem(id: "agent-waiting", source: "agentThread", title: "Approve the research export", status: "needsYou", detail: "Waiting for your permission", canSend: true, agentName: "Researcher", thread: threads[0]),
        PhoneInboxItem(id: "thread-working", source: "thread", title: threads[1].title, status: "working", detail: "", canSend: true, agentName: nil, thread: threads[1]),
        PhoneInboxItem(id: "agent-queued", source: "agentTask", title: "Tomorrow's research brief", status: "working", detail: "Queued", canSend: false, agentName: "Researcher", thread: nil),
        PhoneInboxItem(id: "agent-completed", source: "agentTask", title: "Welcome email ready", status: "idle", detail: "Completed: draft saved for review", canSend: false, agentName: "Writer", thread: threads[2])
    ], unavailable: nil)
    private static let followUp = PhoneThreadDetail(version: 1, thread: threads[2], turns: [
        PhoneThreadTurn(prompt: "Draft a short welcome email for new Kite users.", reply: "Here is a draft: **Welcome to Kite.** Your workspace is ready, and your first project is one click away.", tools: 0, clipped: false)],
        olderTurns: 0, approval: nil, unavailable: nil)
    private static let detail = PhoneThreadDetail(version: 1, thread: threads[0], turns: [
        PhoneThreadTurn(prompt: "Move the settings store to the new schema.", reply: "I read the store and its tests. The new schema needs one migration step, and the old file stays readable until the first save.", tools: 4, clipped: false),
        PhoneThreadTurn(prompt: "Go ahead and write the migration.", reply: "The migration is written. I need to run the migration tests to check it.", tools: 2, clipped: false)],
        olderTurns: 3, approval: PhoneThreadApproval(id: "preview", tool: "Bash", summary: "cargo test -p interface-settings migration", answerable: true), unavailable: nil)
    private let arguments = ProcessInfo.processInfo.arguments
    @State private var path: [PhoneThread] = []
    @State private var draft = ""
    var body: some View {
        NavigationStack(path: $path) {
            PhoneLibraryView(workspaces: workspaces, online: true, error: nil, threads: Self.threads, inbox: Self.inbox,
                             initialSurface: arguments.contains("--design-threads") ? .threads : .code, refresh: {})
                .navigationBarTitleDisplayMode(.inline)
                .toolbarBackground(PhoneStyle.background, for: .navigationBar).toolbarBackground(.visible, for: .navigationBar)
                .toolbar {
                    ToolbarItem(placement: .principal) { PhoneWordmark().fixedSize() }
                    ToolbarItem(placement: .topBarTrailing) { Image(systemName: "ellipsis").frame(width: 44, height: 44).accessibilityLabel("Preview") }
                }
                .navigationDestination(for: PhoneInboxItem.self) { item in
                    if let thread = item.thread {
                        ThreadDetailContent(thread: thread, state: ThreadDetailState(detail: item.status == "needsYou" ? Self.detail : Self.followUp), draft: $draft, canSend: item.canSend, taskDetail: item.detail)
                    }
                }
                .navigationDestination(for: PhoneTerminal.self) { terminal in
                    Text("Design preview · \(terminal.displayTitle)").frame(maxWidth: .infinity, maxHeight: .infinity).background(PhoneStyle.background)
                }
                .navigationDestination(for: PhoneThread.self) { thread in
                    ThreadDetailContent(thread: thread,
                                        state: ThreadDetailState(detail: thread.key == Self.detail.thread.key ? Self.detail
                                            : thread.key == Self.followUp.thread.key ? Self.followUp
                                            : PhoneThreadDetail(version: 1, thread: thread, turns: [], olderTurns: 0, approval: nil, unavailable: nil)),
                                        draft: $draft)
                        .navigationTitle(thread.title).navigationBarTitleDisplayMode(.inline)
                        .toolbarBackground(PhoneStyle.background, for: .navigationBar).toolbarBackground(.visible, for: .navigationBar)
                }
        }.tint(PhoneStyle.text)
            .onAppear {
                if arguments.contains("--design-thread") { path = [Self.threads[0]] }
                if arguments.contains("--design-follow-up") { path = [Self.threads[2]] }
            }
    }
}
#endif

