import Foundation
import WebKit

/// Only signed, bundled session assets are served under the private WebKit scheme.
final class NativeSessionAssets: NSObject, WKURLSchemeHandler {
    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        let files = ["/": ("remote.html", "text/html"), "/remote.js": ("remote.js", "text/javascript"),
                     "/remote.css": ("remote.css", "text/css"), "/icon.svg": ("icon.svg", "image/svg+xml"),
                     "/manifest.webmanifest": ("manifest.webmanifest", "application/manifest+json")]
        guard let url = task.request.url, let file = files[url.path.isEmpty ? "/" : url.path],
              let root = Bundle.main.resourceURL?.appendingPathComponent("RemoteSession"),
              let data = try? Data(contentsOf: root.appendingPathComponent(file.0)), data.count <= 4 * 1024 * 1024 else {
            task.didFailWithError(PhoneError.protocolVersion); return
        }
        // Vite's IIFE bundle needs no module loader or network access in this scheme.
        let bytes: Data
        if file.0 == "remote.html", let html = String(data: data, encoding: .utf8) {
            let policy = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'none'; base-uri 'none'; form-action 'none'\">"
            let prepared = html.replacingOccurrences(of: "<head>", with: "<head>" + policy)
                .replacingOccurrences(of: "type=\"module\" crossorigin", with: "defer")
            bytes = Data(prepared.utf8)
        } else { bytes = data }
        task.didReceive(URLResponse(url: url, mimeType: file.1, expectedContentLength: bytes.count, textEncodingName: "utf-8"))
        task.didReceive(bytes); task.didFinish()
    }
    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {}
}

enum NativeSessionRequest {
    /// The WebKit bridge is bound to the selected terminal and read-only roster.
    /// Auth, enrollment, filesystem, generic HTTP and desktop IPC cannot cross it.
    static func allowed(path: String, method: String, terminal: PhoneTerminal) -> Bool {
        if path == "/api/v2/workspaces" { return method == "GET" }
        let suffix = "/terminals/\(terminal.id)/\(terminal.generation)/"
        if method == "GET", path.hasPrefix("/api/v2" + suffix + "history?limit=") {
            return UInt64(path.dropFirst(("/api/v2" + suffix + "history?limit=").count)) != nil
        }
        if method == "GET", path.hasPrefix("/api/v5" + suffix + "output?cursor=") {
            return UInt64(path.dropFirst(("/api/v5" + suffix + "output?cursor=").count)) != nil
        }
        if path == "/api/v1" + suffix + "draft" { return method == "GET" || method == "POST" }
        if path == "/api/v1" + suffix + "results" { return method == "GET" }
        if path == "/api/v1" + suffix + "control" { return method == "POST" || method == "DELETE" }
        return method == "POST" && ["/api/v1" + suffix + "input", "/api/v4" + suffix + "approval", "/api/v4" + suffix + "approval/respond"].contains(path)
    }
}
