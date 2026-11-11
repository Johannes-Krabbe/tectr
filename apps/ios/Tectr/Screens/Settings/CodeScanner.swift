import SwiftUI
import VisionKit

/// Reads a pairing code with the camera: from a QR code, or from the text of one on a screen.
struct CodeScanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    @MainActor static var isAvailable: Bool {
        DataScannerViewController.isSupported && DataScannerViewController.isAvailable
    }

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr]), .text()],
            qualityLevel: .accurate,
            isHighlightingEnabled: true
        )
        scanner.delegate = context.coordinator
        try? scanner.startScanning()
        return scanner
    }

    func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let found: (String) -> Void
        private var done = false

        init(found: @escaping (String) -> Void) {
            self.found = found
        }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for item in items {
                let text: String? = switch item {
                case .barcode(let code): code.payloadStringValue
                case .text(let text): text.transcript
                @unknown default: nil
                }
                if let text, let code = Self.code(in: text), !done {
                    done = true
                    scanner.stopScanning()
                    found(code)
                    return
                }
            }
        }

        /// The first thing shaped like `tectr-<key>-<secret>`, which text on a screen can
        /// have around it or broken over lines.
        private static func code(in text: String) -> String? {
            let joined = text.components(separatedBy: .whitespacesAndNewlines).joined()
            guard let match = joined.firstMatch(of: /tectr-[A-Za-z0-9]+-[A-Za-z0-9]+/) else { return nil }
            return String(match.output)
        }
    }
}
