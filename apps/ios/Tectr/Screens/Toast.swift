import SwiftUI
import TectrDesign

/// A result that needs nothing from you, for a few seconds. Failures stay where they happened instead.
@MainActor @Observable
final class Toasts {
    private(set) var message: String?
    private var hide: Task<Void, Never>?

    func show(_ message: String) {
        self.message = message
        hide?.cancel()
        hide = Task {
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            self.message = nil
        }
    }
}

extension View {
    /// Sheets cover the root, so each one shows toasts too.
    func toasts() -> some View {
        modifier(ToastOverlay())
    }
}

private struct ToastOverlay: ViewModifier {
    @Environment(Toasts.self) private var toasts

    func body(content: Content) -> some View {
        content.overlay(alignment: .top) {
            // The animation sits inside the overlay. Outside it, anything else that changed
            // with the toast, anywhere on the screen, would animate too.
            ZStack {
                if let message = toasts.message {
                    Text(message)
                        .font(Tokens.Text.label)
                        .lineLimit(1)
                        .padding(.horizontal, Tokens.Space.lg)
                        .padding(.vertical, Tokens.Space.sm)
                        .glassEffect(.regular, in: .capsule)
                        .transition(.move(edge: .top).combined(with: .opacity))
                        .allowsHitTesting(false)
                }
            }
            .animation(.smooth(duration: Tokens.Motion.normal), value: toasts.message)
        }
    }
}
