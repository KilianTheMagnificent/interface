import SwiftUI
import AVFoundation

struct PairingScannerSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var error: String?
    @State private var scanned: PairingInvitation?
    let pair: (PairingInvitation) -> Void
    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 22) {
                Text("Bring your Mac along.").font(.largeTitle.weight(.semibold)).tracking(-1)
                Text("On your Mac, open Interface → Settings → iPhone remote access → Pair iPhone. Scan the QR code here.")
                    .foregroundStyle(PhoneStyle.secondary)
                if let scanned {
                    VStack(alignment: .leading, spacing: 14) {
                        Label(scanned.macName, systemImage: "desktopcomputer").font(.title2.weight(.semibold))
                        Text(scanned.profile == "production" ? "Production sessions" : "Development sessions")
                            .foregroundStyle(PhoneStyle.accent)
                        Text("Approve this iPhone on your Mac once. Use Face ID to unlock it next time.")
                            .foregroundStyle(PhoneStyle.secondary)
                        Button("Pair with this Mac") { dismiss(); pair(scanned) }.buttonStyle(.borderedProminent).tint(PhoneStyle.accent).foregroundStyle(PhoneStyle.background)
                        Button("Scan again") { self.scanned = nil; error = nil }
                    }.padding(22).frame(maxWidth: .infinity, alignment: .leading)
                        .background(PhoneStyle.surface, in: RoundedRectangle(cornerRadius: 20))
                } else {
                    PairingCamera { text in
                        do { scanned = try PairingInvitation.parse(text, now: UInt64(Date().timeIntervalSince1970)); error = nil }
                        catch { self.error = error.localizedDescription }
                    } failed: { error = $0 }
                    .frame(height: 300).clipShape(RoundedRectangle(cornerRadius: 22))
                    .accessibilityLabel("Camera scanning for an Interface pairing QR code")
                }
                if let error { Text(error).font(.footnote).foregroundStyle(PhoneStyle.accent).accessibilityAddTraits(.updatesFrequently) }
                Spacer()
            }.padding(26).background(PhoneStyle.background).foregroundStyle(PhoneStyle.text)
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
        }.preferredColorScheme(.dark)
    }
}

private struct PairingCamera: UIViewControllerRepresentable {
    let scanned: (String) -> Void
    let failed: (String) -> Void
    func makeUIViewController(context: Context) -> CameraController { CameraController(scanned: scanned, failed: failed) }
    func updateUIViewController(_ controller: CameraController, context: Context) {}
    static func dismantleUIViewController(_ controller: CameraController, coordinator: ()) { controller.stop() }
}
private final class CameraController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    private let capture = AVCaptureSession()
    private let queue = DispatchQueue(label: "InterfacePairingCamera")
    private var preview: AVCaptureVideoPreviewLayer?
    private let scanned: (String) -> Void
    private let failed: (String) -> Void
    private var stopped = false
    private var lastScan: String?
    init(scanned: @escaping (String) -> Void, failed: @escaping (String) -> Void) {
        self.scanned = scanned; self.failed = failed; super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { nil }
    override func viewDidLoad() {
        super.viewDidLoad(); view.backgroundColor = .black
        Task { @MainActor [weak self] in
            let authorized: Bool
            if AVCaptureDevice.authorizationStatus(for: .video) == .authorized { authorized = true }
            else { authorized = await AVCaptureDevice.requestAccess(for: .video) }
            guard let self, !stopped else { return }
            guard authorized else { failed("Allow camera access in iPhone Settings to scan your Mac's pairing code."); return }
            configure()
        }
    }
    private func configure() {
        guard let camera = AVCaptureDevice.default(for: .video), let input = try? AVCaptureDeviceInput(device: camera), capture.canAddInput(input) else {
            failed("A camera is unavailable on this device."); return
        }
        capture.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard capture.canAddOutput(output) else { failed("The camera could not scan a QR code."); return }
        capture.addOutput(output); output.setMetadataObjectsDelegate(self, queue: .main); output.metadataObjectTypes = [.qr]
        let layer = AVCaptureVideoPreviewLayer(session: capture); layer.videoGravity = .resizeAspectFill
        layer.frame = view.bounds; view.layer.addSublayer(layer); preview = layer
        queue.async { [capture] in capture.startRunning() }
    }
    override func viewDidLayoutSubviews() { super.viewDidLayoutSubviews(); preview?.frame = view.bounds }
    func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection) {
        guard !stopped, let text = (objects.first as? AVMetadataMachineReadableCodeObject)?.stringValue, text != lastScan else { return }
        lastScan = text; scanned(text)
    }
    func stop() { stopped = true; queue.async { [capture] in capture.stopRunning() } }
}
