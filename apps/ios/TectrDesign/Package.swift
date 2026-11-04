// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "TectrDesign",
    platforms: [.iOS(.v26)],
    products: [
        .library(name: "TectrDesign", targets: ["TectrDesign"]),
    ],
    targets: [
        .target(
            name: "TectrDesign",
            resources: [
                .process("Resources/Icons.xcassets"),
                .copy("Resources/Fonts"),
            ]
        ),
        .testTarget(name: "TectrDesignTests", dependencies: ["TectrDesign"]),
    ]
)
