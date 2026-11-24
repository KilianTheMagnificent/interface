import Foundation

/// Bonjour results are untrusted routing hints. DirectTransport authenticates the
/// pinned Mac with an encrypted probe before using any discovered endpoint.
@MainActor
final class NearbyMacDiscovery: NSObject, @preconcurrency NetServiceBrowserDelegate, @preconcurrency NetServiceDelegate {
    private let browser = NetServiceBrowser()
    private var services: [NetService] = []
    private var completion: CheckedContinuation<URL?, Never>?
    private let macId: String
    private var deadline: Task<Void, Never>?
    private init(macId: String) { self.macId = macId; super.init(); browser.delegate = self }
    static func find(macId: String) async -> URL? {
        let discovery = NearbyMacDiscovery(macId: macId)
        return await withCheckedContinuation { continuation in
            discovery.completion = continuation
            discovery.browser.searchForServices(ofType: "_interface._tcp.", inDomain: "local.")
            discovery.deadline = Task { @MainActor in
                try? await Task.sleep(for: .seconds(2))
                discovery.finish(nil)
            }
        }
    }
    private func finish(_ result: URL?) {
        guard let completion else { return }
        self.completion = nil; browser.stop(); browser.delegate = nil
        for service in services { service.stop(); service.delegate = nil }
        services = []; deadline?.cancel(); deadline = nil
        completion.resume(returning: result)
    }
    func netServiceBrowser(_ browser: NetServiceBrowser, didFind service: NetService, moreComing: Bool) {
        guard services.count < 8, service.name == "Interface-\(macId.prefix(12))" else { return }
        services.append(service); service.delegate = self; service.resolve(withTimeout: 1.5)
    }
    func netServiceDidResolveAddress(_ sender: NetService) {
        guard let data = sender.txtRecordData(), let hostname = sender.hostName,
              hostname.lowercased().hasSuffix(".local."), (1...65535).contains(sender.port) else { return }
        let properties = NetService.dictionary(fromTXTRecord: data)
        guard properties["macId"] == Data(macId.utf8), properties["version"] == Data("5".utf8) else { return }
        var parts = URLComponents(); parts.scheme = "http"; parts.host = String(hostname.dropLast()); parts.port = sender.port
        finish(parts.url)
    }
    func netServiceBrowser(_ browser: NetServiceBrowser, didNotSearch errorDict: [String : NSNumber]) { finish(nil) }
}
