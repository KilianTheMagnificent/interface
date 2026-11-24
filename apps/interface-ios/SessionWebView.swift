import SwiftUI
import WebKit

// Opaque input markers survive the ephemeral web view; credentials and input text never do.
enum InputMarkers {
    private static func file() throws -> URL {
        let folder = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        return folder.appendingPathComponent("input-markers.v1.json")
    }
    private static func read() throws -> [String: String] {
        let url = try file()
        guard FileManager.default.fileExists(atPath: url.path) else { return [:] }
        let data = try Data(contentsOf: url)
        guard data.count <= 256 * 1024 else { throw PhoneError.storage }
        return try JSONDecoder().decode([String: String].self, from: data)
    }
    static func pending(_ key: String) -> Bool { (try? read()[key] != nil) ?? true }
    /// The marker's opaque value; an unreadable store is a failure, never "none".
    static func value(_ key: String) -> Result<String?, Error> { Result { try read()[key] } }
    static func set(_ key: String, command: String?) throws {
        var values = try read()
        if command != nil && values[key] == nil && values.count >= 512 { throw PhoneError.storage }
        values[key] = command
        try JSONEncoder().encode(values).write(to: file(), options: [.atomic, .completeFileProtection])
    }
}

struct SessionWebView: UIViewRepresentable {
    let origin: URL
    let cookie: HTTPCookie
    let terminal: PhoneTerminal
    let reload: Int
    @Binding var error: String?
    @Binding var loading: Bool
    var client: MacClient? = nil
    func makeCoordinator() -> Coordinator { Coordinator(origin: origin, terminal: terminal, cookie: cookie, client: client, error: $error, loading: $loading) }
    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        if client?.direct != nil {
            config.setURLSchemeHandler(NativeSessionAssets(), forURLScheme: "interface-session")
            config.userContentController.addScriptMessageHandler(context.coordinator, contentWorld: .page, name: "interfaceRequest")
            config.userContentController.addUserScript(WKUserScript(source: "window.__interfaceConnectionVersion = 5;", injectionTime: .atDocumentStart, forMainFrameOnly: true))
        }
        let key = context.coordinator.markerKey
        config.userContentController.addScriptMessageHandler(context.coordinator, contentWorld: .page, name: "interfaceInput")
        config.userContentController.addUserScript(WKUserScript(source: "window.__interfaceNativeInput = {version:1,pending:\(InputMarkers.pending(key) ? "true" : "false")};", injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let web = WKWebView(frame: .zero, configuration: config)
        web.navigationDelegate = context.coordinator; web.uiDelegate = context.coordinator
        web.isOpaque = false; web.backgroundColor = UIColor(PhoneStyle.background)
        web.scrollView.backgroundColor = web.backgroundColor
        web.scrollView.contentInsetAdjustmentBehavior = .never
        return web
    }
    func updateUIView(_ web: WKWebView, context: Context) {
        guard context.coordinator.loaded != reload else { return }
        context.coordinator.loaded = reload
        let coordinator = context.coordinator
        Task { @MainActor [weak web, weak coordinator] in
            guard let web, let coordinator else { return }
            if client?.direct == nil { await web.configuration.websiteDataStore.httpCookieStore.setCookie(cookie) }
            guard !coordinator.stopped else { return }
            var parts = URLComponents(url: coordinator.viewOrigin, resolvingAgainstBaseURL: false)!
            parts.queryItems = [URLQueryItem(name: "native", value: "1"), URLQueryItem(name: "terminal", value: terminal.id), URLQueryItem(name: "generation", value: String(terminal.generation))]
            web.load(URLRequest(url: parts.url!, cachePolicy: .reloadIgnoringLocalCacheData))
        }
    }
    static func dismantleUIView(_ web: WKWebView, coordinator: Coordinator) {
        coordinator.stopped = true
        web.stopLoading(); web.navigationDelegate = nil; web.uiDelegate = nil
        web.configuration.userContentController.removeScriptMessageHandler(forName: "interfaceInput", contentWorld: .page)
        web.configuration.userContentController.removeScriptMessageHandler(forName: "interfaceRequest", contentWorld: .page)
        // nonPersistent data store, including its session cookie, dies with this view.
    }
    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandlerWithReply {
        let origin: URL
        let viewOrigin: URL
        let terminal: PhoneTerminal
        let cookie: HTTPCookie
        let client: MacClient?
        var requests = 0
        var navigationDeadline: Task<Void, Never>?
        let inputKey: String
        let markerKey: String
        var loaded: Int?
        var stopped = false
        @Binding var error: String?
        @Binding var loading: Bool
        init(origin: URL, terminal: PhoneTerminal, cookie: HTTPCookie, client: MacClient?, error: Binding<String?>, loading: Binding<Bool>) {
            self.origin = origin
            self.terminal = terminal; self.cookie = cookie; self.client = client
            self.viewOrigin = client?.direct != nil ? URL(string: "interface-session://\(origin.host!)/")! : origin
            inputKey = "interface.remote.pending.\(terminal.id).\(terminal.generation)"
            markerKey = "\(origin.absoluteString)|\(inputKey)"
            self._error = error; self._loading = loading
        }
        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage,
                                   replyHandler: @escaping (Any?, String?) -> Void) {
            let source = message.frameInfo.securityOrigin
            guard !stopped, message.frameInfo.isMainFrame, source.protocol == viewOrigin.scheme,
                  source.host == viewOrigin.host, (source.port == 0 ? (source.protocol == "https" ? 443 : 80) : source.port) == (viewOrigin.port ?? (viewOrigin.scheme == "https" ? 443 : 80)) else { replyHandler(nil, "This frame cannot access Interface."); return }
            if message.name == "interfaceRequest" { remoteRequest(message.body, replyHandler: replyHandler); return }
            guard message.name == "interfaceInput",
                  let body = message.body as? [String: String], body["key"] == inputKey,
                  let operation = body["operation"], ["reserve", "clear"].contains(operation),
                  Set(body.keys).isSubset(of: ["operation", "key", "command"]) else { replyHandler(nil, "Invalid input marker request."); return }
            do {
                let command = body["command"]
                if operation == "reserve" { guard let command, UUID(uuidString: command) != nil else { throw PhoneError.storage } }
                try InputMarkers.set(markerKey, command: operation == "reserve" ? command : nil)
                replyHandler(true, nil)
            } catch { replyHandler(nil, "The input marker could not be saved. Check the terminal before continuing.") }
        }
        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            guard let url = action.request.url, MacAddress.contains(url, origin: viewOrigin) else {
                error = "Open external links separately in Safari."; decisionHandler(.cancel); return
            }
            decisionHandler(.allow)
        }
        func webView(_ webView: WKWebView, decidePolicyFor response: WKNavigationResponse, decisionHandler: @escaping (WKNavigationResponsePolicy) -> Void) {
            if response.isForMainFrame, let http = response.response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                error = PhoneError.unavailable.localizedDescription; loading = false; decisionHandler(.cancel); return
            }
            decisionHandler(.allow)
        }
        func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
            loading = true; error = nil; navigationDeadline?.cancel()
            navigationDeadline = Task { @MainActor [weak self, weak webView] in
                try? await Task.sleep(for: .seconds(12))
                guard !Task.isCancelled, let self, !stopped, loading else { return }
                webView?.stopLoading(); loading = false; error = "The session view took too long to open. Check the connection and tap Reconnect."
            }
        }
        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { navigationDeadline?.cancel(); loading = false }
        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError cause: Error) { failed(cause) }
        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError cause: Error) { failed(cause) }
        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) { loading = false; error = "The session view was interrupted. Reconnect and check your last input before sending again." }
        private func failed(_ cause: Error) { if (cause as NSError).code != NSURLErrorCancelled { loading = false; error = PhoneError.unavailable.localizedDescription } }
        func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration, for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? { error = "Open external links separately in Safari."; return nil }
        private func remoteRequest(_ payload: Any, replyHandler: @escaping (Any?, String?) -> Void) {
            guard requests < 8, let direct = client?.direct, let body = payload as? [String: String],
                  Set(body.keys).isSubset(of: ["path", "method", "body", "control", "command"]),
                  let path = body["path"], path.utf8.count <= 256, let method = body["method"],
                  NativeSessionRequest.allowed(path: path, method: method, terminal: terminal),
                  let encoded = body["body"], encoded.utf8.count <= 24 * 1024,
                  let bytes = Data(base64Encoded: encoded), bytes.count <= 16384,
                  [body["control"], body["command"]].allSatisfy({ $0 == nil || UUID(uuidString: $0!) != nil }) else {
                replyHandler(nil, "This session request was refused."); return
            }
            requests += 1
            Task { @MainActor in
                defer { requests -= 1 }
                do {
                    // The body stays base64 from the tunnel to JavaScript: its decoded
                    // size was bounded by the transport, and JavaScript rejects bad base64.
                    let (encoded, response) = try await direct.requestEncoded(path: path, method: method, body: bytes, cookie: cookie, control: body["control"], command: body["command"])
                    guard !stopped else { replyHandler(nil, "Interface is locked."); return }
                    replyHandler(["version":5, "status":response.statusCode, "body":encoded, "contentType": response.value(forHTTPHeaderField: "Content-Type") ?? "application/octet-stream"], nil)
                } catch { replyHandler(nil, "Connection interrupted. Check your Mac before sending again.") }
            }
        }
    }
}
