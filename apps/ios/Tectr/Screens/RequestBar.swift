import SwiftUI
import TectrDesign

/// Something the agent needs from you, in the message box's place at the bottom of its tab.
struct RequestBar: View {
    @Environment(Store.self) private var store
    let request: Request

    var body: some View {
        RequestCard(
            kind: request.kind,
            position: store.position(of: request),
            command: request.command,
            reason: request.reason,
            options: request.options
        ) { option in
            store.answer(request, with: option)
        }
        .padding(Tokens.Space.lg)
        .glassEffect(.regular, in: .rect(cornerRadius: Tokens.Radius.lg))
        .padding(.horizontal, Tokens.Space.md)
        .padding(.bottom, Tokens.Space.sm)
        .frame(maxWidth: 760)
        .id(request.id)
        .transition(.move(edge: .bottom).combined(with: .opacity))
    }
}
