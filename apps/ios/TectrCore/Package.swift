// swift-tools-version: 6.2
import PackageDescription

// The engine, built by `mise run ios:core`, with the protocol's types for reading what it sends.
let package = Package(
    name: "TectrCore",
    platforms: [.iOS(.v26)],
    products: [
        .library(name: "TectrCore", targets: ["TectrCore"]),
    ],
    targets: [
        .binaryTarget(name: "TectrFFI", path: "TectrFFI.xcframework"),
        .target(
            name: "TectrCore",
            dependencies: ["TectrFFI"],
            // The generated bindings predate Swift 6's checks.
            swiftSettings: [.swiftLanguageMode(.v5)],
            // What the engine's network code calls into, which a static library can't bring along.
            linkerSettings: [.linkedFramework("SystemConfiguration")]
        ),
    ]
)
