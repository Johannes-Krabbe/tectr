import SwiftUI

/// A small picture of a theme: its surfaces, text, accent and status colors.
public struct ThemeSwatch: View {
    let palette: Palette

    public init(_ palette: Palette) {
        self.palette = palette
    }

    public var body: some View {
        HStack(spacing: 0) {
            palette.surfaceBase.frame(width: 14)
            VStack(alignment: .leading, spacing: 4) {
                Capsule().fill(palette.textPrimary).frame(width: 26, height: 3)
                Capsule().fill(palette.textTertiary).frame(width: 18, height: 3)
                HStack(spacing: 3) {
                    Circle().fill(palette.statusWaiting)
                    Circle().fill(palette.statusDone)
                    Circle().fill(palette.statusFailed)
                    Circle().fill(palette.accentSolid)
                }
                .frame(height: 5)
            }
            .padding(.horizontal, 6)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
            .background(palette.surfacePanel)
        }
        .frame(width: 64, height: 40)
        .clipShape(.rect(cornerRadius: Tokens.Radius.xs))
        .overlay {
            RoundedRectangle(cornerRadius: Tokens.Radius.xs).stroke(palette.lineStrong, lineWidth: 1)
        }
        .accessibilityHidden(true)
    }
}
