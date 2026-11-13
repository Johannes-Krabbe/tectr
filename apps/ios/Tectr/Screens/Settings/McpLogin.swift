import SwiftUI
import TectrDesign
import WebKit

struct LoginPage: Identifiable {
    let url: URL
    var id: String { url.absoluteString }
}

/// Logs a device in to an MCP server. The server sends the browser back to an address on the
/// device's own loopback, which a phone can't reach, so this catches that redirect and hands
/// what it carries to the device instead.
struct McpLoginView: View {
    @Environment(Store.self) private var store
    @Environment(Toasts.self) private var toasts
    @Environment(\.dismiss) private var dismiss
    let url: URL
    let deviceID: Device.ID

    var body: some View {
        NavigationStack {
            LoginWebView(url: url) { state, code in
                Task {
                    if await store.completeLogin(state: state, code: code, on: deviceID) { toasts.show("Logged in") }
                    dismiss()
                }
            }
            .ignoresSafeArea(edges: .bottom)
            .navigationTitle(url.host() ?? "Log in")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(role: .cancel) { dismiss() } }
            }
        }
    }
}

private struct LoginWebView: UIViewRepresentable {
    let url: URL
    let finish: (_ state: String, _ code: String) -> Void

    func makeUIView(context: Context) -> WKWebView {
        // Nothing from a login is kept on the phone: the device holds the tokens.
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = context.coordinator
        view.load(URLRequest(url: url))
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(finish: finish) }

    final class Coordinator: NSObject, WKNavigationDelegate {
        let finish: (String, String) -> Void

        init(finish: @escaping (String, String) -> Void) {
            self.finish = finish
        }

        func webView(
            _ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
            decisionHandler: @escaping @MainActor (WKNavigationActionPolicy) -> Void
        ) {
            guard let url = action.request.url, ["127.0.0.1", "localhost", "[::1]"].contains(url.host() ?? ""),
                  let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
                  let state = items.first(where: { $0.name == "state" })?.value,
                  let code = items.first(where: { $0.name == "code" })?.value
            else {
                decisionHandler(.allow)
                return
            }
            decisionHandler(.cancel)
            finish(state, code)
        }
    }
}
