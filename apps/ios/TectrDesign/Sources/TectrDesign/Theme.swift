import SwiftUI

public extension View {
    /// App-wide defaults, applied once at the root. Appearance follows the system.
    func tectrTheme() -> some View {
        self
            .font(Tokens.Text.body)
            .foregroundStyle(.textPrimary)
            .tint(.accentText)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(.surfaceBase)
    }
}
