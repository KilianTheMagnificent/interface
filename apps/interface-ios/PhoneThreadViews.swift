import SwiftUI

// The Mac's Threads on the phone (review D6). Order, status and what can be
// answered all come from the Mac's Rust host; these views only draw them and
// send one deliberate tap at a time.

extension PhoneStyle {
    /// The desktop's waiting colour: a session or thread that needs you.
    static let attention = Color(red: 232/255, green: 178/255, blue: 92/255)
}

/// One ordered owner inbox, combining Threads and Agent tasks. Rust supplies
/// both membership and status; session-less queued work has no action link.
struct PhoneInboxListView: View {
    let inbox: PhoneInbox
    let search: String
    private var shown: [PhoneInboxItem] {
        search.isEmpty ? inbox.items : inbox.items.filter { "\($0.title) \($0.agentName ?? "") \($0.detail)".localizedCaseInsensitiveContains(search) }
    }
    var body: some View {
        LazyVStack(alignment: .leading, spacing: 14) {
            if let unavailable = inbox.unavailable { Text(unavailable).font(.footnote).foregroundStyle(PhoneStyle.attention) }
            ForEach(shown) { item in
                if item.thread != nil {
                    NavigationLink(value: item) { row(item) }.buttonStyle(PhonePressStyle()).accessibilityIdentifier("inbox-\(item.id)")
                } else { row(item) }
            }
            if shown.isEmpty { Text(search.isEmpty ? "No work in your inbox yet." : "No matching work.").font(.subheadline).foregroundStyle(PhoneStyle.secondary).padding(.vertical, 32) }
        }
    }
    private func row(_ item: PhoneInboxItem) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(item.agentName.map { "Agent · \($0)" } ?? "Thread").font(.caption.weight(.semibold)).foregroundStyle(PhoneStyle.secondary)
                Spacer()
                if item.status == "needsYou" { NeedsYouLabel() }
                else if item.status == "working" { Text("Working").font(.caption.weight(.semibold)).foregroundStyle(PhoneStyle.accent) }
                else if item.status == "failed" { Text("Needs review").font(.caption).foregroundStyle(PhoneStyle.attention) }
                if item.thread != nil { Image(systemName: "chevron.right").font(.caption).foregroundStyle(PhoneStyle.secondary) }
            }
            Text(item.title).font(.body.weight(.medium)).foregroundStyle(PhoneStyle.text).multilineTextAlignment(.leading)
            if !item.detail.isEmpty { Text(item.detail).font(.caption).foregroundStyle(PhoneStyle.secondary).lineLimit(3).multilineTextAlignment(.leading) }
        }.padding(18).frame(maxWidth: .infinity, alignment: .leading)
            .background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 20))
            .overlay(RoundedRectangle(cornerRadius: 20).stroke(PhoneStyle.border))
    }
}

/// "Needs you", in the desktop's waiting colour.
struct NeedsYouLabel: View {
    var body: some View {
        Label("Needs you", systemImage: "exclamationmark.circle.fill")
            .labelStyle(.titleAndIcon).font(.caption.weight(.semibold))
            .foregroundStyle(PhoneStyle.attention)
    }
}

struct PhoneThreadRow: View {
    let thread: PhoneThread
    @Environment(\.dynamicTypeSize) private var typeSize
    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            ProviderMark(profileId: thread.profileId)
                .frame(width: 36, height: 36).background(PhoneStyle.elevated, in: RoundedRectangle(cornerRadius: 11))
                .overlay(alignment: .topTrailing) {
                    if thread.needsYou {
                        Circle().fill(PhoneStyle.attention).frame(width: 9, height: 9)
                            .overlay(Circle().stroke(PhoneStyle.surface, lineWidth: 2)).offset(x: 3, y: -3)
                    }
                }
            VStack(alignment: .leading, spacing: 9) {
                Text(thread.title).font(.system(.body, weight: .medium)).tracking(-0.25)
                    .lineLimit(typeSize.isAccessibilitySize ? nil : 2).multilineTextAlignment(.leading)
                    .foregroundStyle(PhoneStyle.text)
                HStack(spacing: 6) {
                    if thread.needsYou { NeedsYouLabel() }
                    else if thread.working { Text("Working").font(.caption.weight(.semibold)).foregroundStyle(PhoneStyle.accent) }
                    else if thread.status == "failed" { Text("Stopped with an error").font(.caption).foregroundStyle(PhoneStyle.secondary) }
                    if let project = thread.projectName {
                        if thread.needsYou || thread.working || thread.status == "failed" { Text("·").font(.caption).foregroundStyle(PhoneStyle.secondary) }
                        Text(project).font(.caption).foregroundStyle(PhoneStyle.secondary).lineLimit(1)
                    } else if !thread.needsYou && !thread.working && thread.status != "failed" {
                        Text("Chat").font(.caption).foregroundStyle(PhoneStyle.secondary)
                    }
                }
            }.frame(maxWidth: .infinity, alignment: .leading)
            if thread.unread {
                Circle().fill(PhoneStyle.accent).frame(width: 7, height: 7).padding(.top, 14).accessibilityLabel("Unread")
            }
            Image(systemName: "chevron.right").font(.system(size: 10, weight: .semibold))
                .foregroundStyle(PhoneStyle.secondary).padding(.top, 12)
        }.padding(18).frame(minHeight: 94).contentShape(Rectangle())
            .accessibilityElement(children: .combine)
    }
}

/// The Threads list: the Mac's order kept, drawn as its three groups.
struct PhoneThreadListView: View {
    let threads: [PhoneThread]
    let search: String
    private var shown: [PhoneThread] {
        search.isEmpty ? threads : threads.filter {
            "\($0.title) \($0.projectName ?? "") \($0.engine)".localizedCaseInsensitiveContains(search)
        }
    }
    private var groups: [(title: String, threads: [PhoneThread])] {
        let needsYou = shown.filter(\.needsYou), working = shown.filter(\.working)
        let rest = shown.filter { !$0.needsYou && !$0.working }
        return [("Needs you", needsYou), ("Working", working), ("Threads", rest)].filter { !$0.1.isEmpty }
    }
    var body: some View {
        LazyVStack(alignment: .leading, spacing: 28) {
            ForEach(groups, id: \.title) { group in
                VStack(alignment: .leading, spacing: 12) {
                    HStack(spacing: 8) {
                        Image(systemName: group.title == "Needs you" ? "exclamationmark.circle" : group.title == "Working" ? "circle.dotted" : "bubble.left.and.bubble.right")
                            .font(.system(size: 12, weight: .medium))
                        Text(group.title).font(.subheadline.weight(.semibold))
                        Spacer(minLength: 8)
                        Text("\(group.threads.count)").font(.caption.monospacedDigit())
                    }.foregroundStyle(group.title == "Needs you" ? PhoneStyle.attention : PhoneStyle.secondary).padding(.horizontal, 4)
                    VStack(spacing: 0) {
                        ForEach(Array(group.threads.enumerated()), id: \.element.id) { index, thread in
                            if index > 0 { Rectangle().fill(PhoneStyle.border).frame(height: 1).padding(.leading, 66) }
                            NavigationLink(value: thread) { PhoneThreadRow(thread: thread) }
                                .buttonStyle(PhonePressStyle()).accessibilityIdentifier("thread-\(thread.key)")
                        }
                    }.background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 20))
                        .overlay(RoundedRectangle(cornerRadius: 20).stroke(PhoneStyle.border))
                }
            }
            if groups.isEmpty {
                VStack(spacing: 14) {
                    Image(systemName: search.isEmpty ? "bubble.left.and.bubble.right" : "magnifyingglass").font(.system(size: 28)).foregroundStyle(PhoneStyle.accent)
                    Text(threads.isEmpty ? "No threads yet" : "No matching threads").font(.title3.weight(.semibold))
                    Text(threads.isEmpty ? "Start a thread in Interface on your Mac.\nIt will be here when you need it." : "Try another search.")
                        .font(.subheadline).foregroundStyle(PhoneStyle.secondary).multilineTextAlignment(.center).lineSpacing(4)
                }.padding(.vertical, 56).frame(maxWidth: .infinity)
            }
        }
    }
}

/// What the detail page can do now, decided from the Mac's answer and this
/// phone's own uncertainty markers.
struct ThreadDetailState: Equatable {
    var detail: PhoneThreadDetail?
    var error: String?
    var notice: String?
    var busy = false
    /// An answer to this request may already have reached the agent.
    var answerUncertain = false
    /// A follow-up may already have reached the agent.
    var sendUncertain = false
}

/// The page itself, without a Mac: the live view and the design preview share it.
struct ThreadDetailContent: View {
    let thread: PhoneThread
    let state: ThreadDetailState
    @Binding var draft: String
    var answer: (Bool) -> Void = { _ in }
    var send: () -> Void = {}
    var checked: () -> Void = {}
    var canSend = true
    var taskDetail = ""
    private var current: PhoneThread { state.detail?.thread ?? thread }
    private var atRest: Bool { canSend && (current.status == "idle" || current.status == "failed") }
    var body: some View {
        ScrollViewReader { scroll in
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    VStack(alignment: .leading, spacing: 8) {
                        HStack(spacing: 6) {
                            if current.needsYou { NeedsYouLabel(); Text("·").font(.caption).foregroundStyle(PhoneStyle.secondary) }
                            else if current.working {
                                Text("Working").font(.caption.weight(.semibold)).foregroundStyle(PhoneStyle.accent)
                                Text("·").font(.caption).foregroundStyle(PhoneStyle.secondary)
                            }
                            Text(current.projectName ?? "Chat").font(.caption).foregroundStyle(PhoneStyle.secondary)
                        }
                        Text(current.title).font(.title2.weight(.semibold)).tracking(-0.6).foregroundStyle(PhoneStyle.text)
                        if !taskDetail.isEmpty { Text(taskDetail).font(.subheadline).foregroundStyle(PhoneStyle.secondary).textSelection(.enabled) }
                        if !canSend { Text("Task result. Start new work from the agent on your Mac.").font(.caption).foregroundStyle(PhoneStyle.secondary) }
                    }.padding(.top, 8)
                    if let older = state.detail?.olderTurns, older > 0 {
                        Label("\(older) earlier \(older == 1 ? "turn is" : "turns are") on your Mac", systemImage: "clock.arrow.circlepath")
                            .font(.caption).foregroundStyle(PhoneStyle.secondary)
                    }
                    if let detail = state.detail {
                        ForEach(Array(detail.turns.enumerated()), id: \.offset) { index, turn in
                            ThreadTurnView(turn: turn).id(index)
                        }
                        if let unavailable = detail.unavailable {
                            Label(unavailable, systemImage: "desktopcomputer").font(.footnote).foregroundStyle(PhoneStyle.secondary)
                        } else if detail.turns.isEmpty {
                            Text("No turns yet.").font(.footnote).foregroundStyle(PhoneStyle.secondary)
                        }
                    } else if state.error == nil {
                        HStack(spacing: 10) { ProgressView().tint(PhoneStyle.accent); Text("Reading the thread").font(.subheadline).foregroundStyle(PhoneStyle.secondary) }
                            .frame(maxWidth: .infinity).padding(.vertical, 40)
                    }
                    if let error = state.error {
                        Label(error, systemImage: "wifi.exclamationmark").font(.footnote).foregroundStyle(PhoneStyle.secondary)
                    }
                    Color.clear.frame(height: 1).id("end")
                }.padding(.horizontal, 22).padding(.bottom, 20).frame(maxWidth: 640).frame(maxWidth: .infinity)
            }
            .onChange(of: state.detail?.turns.count) { _, _ in withAnimation { scroll.scrollTo("end", anchor: .bottom) } }
            .onAppear { scroll.scrollTo("end", anchor: .bottom) }
        }
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 10) {
                if let notice = state.notice {
                    Text(notice).font(.footnote).foregroundStyle(PhoneStyle.secondary).frame(maxWidth: .infinity, alignment: .leading)
                }
                if let approval = state.detail?.approval {
                    ThreadApprovalCard(approval: approval, busy: state.busy, uncertain: state.answerUncertain, answer: answer)
                } else if current.working && state.detail != nil {
                    HStack(spacing: 8) {
                        ProgressView().tint(PhoneStyle.accent)
                        Text("Agent is working on your Mac…").font(.footnote).foregroundStyle(PhoneStyle.secondary)
                    }.padding(.vertical, 8).frame(maxWidth: .infinity, alignment: .leading)
                } else if atRest && state.detail != nil {
                    if state.sendUncertain {
                        VStack(alignment: .leading, spacing: 10) {
                            Text("Your last message may have reached the agent. Check this thread on your Mac before sending another.")
                                .font(.footnote).foregroundStyle(PhoneStyle.secondary)
                            Button("I checked on my Mac", action: checked).font(.footnote.weight(.semibold)).frame(minHeight: 44).tint(PhoneStyle.accent)
                        }.padding(16).frame(maxWidth: .infinity, alignment: .leading)
                            .background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 18))
                    } else {
                        HStack(alignment: .bottom, spacing: 10) {
                            TextField("Follow up", text: $draft, axis: .vertical).lineLimit(1...5).font(.body)
                                .padding(.horizontal, 14).padding(.vertical, 11)
                                .background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 20))
                                .overlay(RoundedRectangle(cornerRadius: 20).stroke(PhoneStyle.border))
                                .accessibilityIdentifier("thread-follow-up")
                            Button(action: send) {
                                Image(systemName: "arrow.up").font(.system(size: 16, weight: .bold)).frame(width: 44, height: 44)
                                    .foregroundStyle(PhoneStyle.background)
                                    .background(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || state.busy ? PhoneStyle.secondary : PhoneStyle.accent, in: Circle())
                            }.disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || state.busy)
                                .accessibilityLabel("Send")
                        }
                    }
                }
            }.padding(.horizontal, 16).padding(.top, 8).padding(.bottom, 8).background(PhoneStyle.background)
        }
        .background(PhoneStyle.background).foregroundStyle(PhoneStyle.text)
    }
}

struct ThreadTurnView: View {
    let turn: PhoneThreadTurn
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if !turn.prompt.isEmpty {
                Text(turn.prompt).font(.body).foregroundStyle(PhoneStyle.text).textSelection(.enabled)
                    .padding(.horizontal, 15).padding(.vertical, 11)
                    .background(PhoneStyle.elevated, in: RoundedRectangle(cornerRadius: 18))
                    .frame(maxWidth: .infinity, alignment: .trailing).padding(.leading, 44)
            }
            if !turn.reply.isEmpty {
                Text(Self.formatted(turn.reply)).font(.body).lineSpacing(4).foregroundStyle(PhoneStyle.text).textSelection(.enabled)
            }
            if turn.tools > 0 || turn.clipped {
                HStack(spacing: 10) {
                    if turn.tools > 0 {
                        Label("\(turn.tools) tool \(turn.tools == 1 ? "call" : "calls")", systemImage: "wrench.and.screwdriver")
                    }
                    if turn.clipped { Label("Shortened for the phone", systemImage: "scissors") }
                }.font(.caption).foregroundStyle(PhoneStyle.secondary)
            }
        }
    }
    /// Inline Markdown (bold, code, links) as the Mac renders it; plain text otherwise.
    static func formatted(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
    }
}

/// The request a thread is parked on: Allow once or Deny, never a standing grant.
struct ThreadApprovalCard: View {
    let approval: PhoneThreadApproval
    let busy: Bool
    let uncertain: Bool
    let answer: (Bool) -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 6) {
                Image(systemName: "hand.raised.fill")
                Text("Needs your OK").textCase(.uppercase).tracking(1.1)
            }.font(.system(size: 11, weight: .semibold)).foregroundStyle(PhoneStyle.attention)
            Text(approval.tool).font(.headline).foregroundStyle(PhoneStyle.text)
            if !approval.summary.isEmpty {
                Text(approval.summary).font(.system(.footnote, design: .monospaced)).foregroundStyle(PhoneStyle.secondary)
                    .lineLimit(6).frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12).background(PhoneStyle.elevated, in: RoundedRectangle(cornerRadius: 12))
            }
            if !approval.answerable {
                Label("This is a question. Answer it on your Mac.", systemImage: "desktopcomputer")
                    .font(.footnote).foregroundStyle(PhoneStyle.secondary)
            } else if uncertain {
                Label("Your answer may have reached the agent. Check this thread on your Mac.", systemImage: "questionmark.circle")
                    .font(.footnote).foregroundStyle(PhoneStyle.secondary)
            } else {
                HStack(spacing: 10) {
                    Button { answer(false) } label: {
                        Text("Deny").font(.headline).frame(maxWidth: .infinity).frame(minHeight: 48)
                            .foregroundStyle(PhoneStyle.text).background(PhoneStyle.elevated, in: RoundedRectangle(cornerRadius: 14))
                    }.accessibilityIdentifier("thread-deny")
                    Button { answer(true) } label: {
                        Text("Allow once").font(.headline).frame(maxWidth: .infinity).frame(minHeight: 48)
                            .foregroundStyle(PhoneStyle.background).background(PhoneStyle.accent, in: RoundedRectangle(cornerRadius: 14))
                    }.accessibilityIdentifier("thread-allow")
                }.buttonStyle(PhonePressStyle()).disabled(busy).opacity(busy ? 0.6 : 1)
            }
        }.padding(18).frame(maxWidth: .infinity, alignment: .leading)
            .background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 20))
            .overlay(RoundedRectangle(cornerRadius: 20).stroke(PhoneStyle.attention.opacity(0.45)))
    }
}

/// A thread from the Mac: polled while it shows, one answer or follow-up at a time.
struct ThreadDetailView: View {
    let thread: PhoneThread
    let cookie: HTTPCookie
    var client: MacClient?
    var canSend = true
    var taskDetail = ""
    @State private var state = ThreadDetailState()
    @State private var draft = ""
    @State private var refresh = 0
    var body: some View {
        ThreadDetailContent(thread: thread, state: state, draft: $draft,
                            answer: { allow in Task { await answer(allow) } },
                            send: { Task { await send() } },
                            checked: {
                                if let macId = client?.connectionMacId { ThreadSendMarkers.clear(macId: macId, thread: thread.key) }
                                state.sendUncertain = false
                            }, canSend: canSend, taskDetail: taskDetail)
            .navigationTitle(thread.title).navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(PhoneStyle.background, for: .navigationBar).toolbarBackground(.visible, for: .navigationBar)
            .toolbar { ToolbarItem(placement: .principal) {
                VStack(spacing: 4) {
                    Text(thread.title).font(.subheadline.weight(.semibold)).lineLimit(1).foregroundStyle(PhoneStyle.text)
                    Text("Thread").font(.system(size: 11, weight: .medium)).foregroundStyle(PhoneStyle.accent)
                }
            } }
            .task(id: refresh) {
                while !Task.isCancelled {
                    await load()
                    try? await Task.sleep(for: .seconds(3))
                }
            }
    }
    private func load() async {
        guard let client else { state.error = "Your Mac could not be reached."; return }
        do {
            let detail = try await client.threadDetail(thread, cookie: cookie)
            guard !Task.isCancelled else { return }
            if let macId = client.connectionMacId {
                state.answerUncertain = ThreadAnswerMarkers.blocks(macId: macId, thread: thread.key, approvalId: detail.approval?.id)
                state.sendUncertain = ThreadSendMarkers.blocks(macId: macId, thread: thread.key, turns: detail.turnCount)
            }
            if state.detail != detail { state.detail = detail }
            state.error = nil
        } catch {
            guard !Task.isCancelled else { return }
            state.error = error.localizedDescription
        }
    }
    private func answer(_ allow: Bool) async {
        guard let client, let approval = state.detail?.approval, !state.busy else { return }
        state.busy = true; state.notice = nil
        let outcome = await client.answer(thread, approvalId: approval.id, allow: allow, cookie: cookie)
        state.busy = false
        switch outcome {
        case .sent: state.notice = allow ? "Allowed once." : "Denied."
        case .refused(let message): state.notice = message
        case .unknown: state.answerUncertain = true
        }
        refresh += 1
    }
    private func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard canSend, let client, let detail = state.detail, !text.isEmpty, !state.busy else { return }
        state.busy = true; state.notice = nil
        let outcome = await client.send(thread, text: text, turns: detail.turnCount, cookie: cookie)
        state.busy = false
        switch outcome {
        case .sent: draft = ""; state.notice = "Sent. The thread is working on your Mac."
        case .refused(let message): state.notice = message
        case .unknown: draft = ""; state.sendUncertain = true
        }
        refresh += 1
    }
}
