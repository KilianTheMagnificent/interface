// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "interface-voice",
    platforms: [.macOS("26.0")],
    products: [
        .executable(name: "interface-voice", targets: ["interface-voice"])
    ],
    targets: [
        .executableTarget(
            name: "interface-voice",
            exclude: ["Info.plist"],
            swiftSettings: [.swiftLanguageMode(.v6)],
            // TCC needs a usage description; a bare CLI tool carries one in a
            // __TEXT,__info_plist section. Path is relative to the package dir,
            // so build with `swift build` from apps/interface-voice.
            linkerSettings: [.unsafeFlags([
                "-Xlinker", "-sectcreate",
                "-Xlinker", "__TEXT",
                "-Xlinker", "__info_plist",
                "-Xlinker", "Sources/interface-voice/Info.plist",
            ])]
        ),
        .testTarget(name: "VoiceBufferTests", dependencies: ["interface-voice"])
    ]
)
