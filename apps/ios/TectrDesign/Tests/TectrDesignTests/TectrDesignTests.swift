import CoreText
import SwiftUI
import Testing
import UIKit
@testable import TectrDesign

@Suite struct TectrDesignTests {
    @Test func colorsFollowTheAppearance() {
        let color = UIColor(Color(light: 0x112233ff, dark: 0x44556680))
        let light = color.resolvedColor(with: UITraitCollection(userInterfaceStyle: .light))
        let dark = color.resolvedColor(with: UITraitCollection(userInterfaceStyle: .dark))

        #expect(channels(of: light) == [0x11, 0x22, 0x33, 0xff])
        #expect(channels(of: dark) == [0x44, 0x55, 0x66, 0x80])
    }

    @Test func codeFontLoads() {
        _ = Font.code(size: 13, relativeTo: .footnote)

        #expect(UIFont(name: Tokens.codeFontName, size: 13) != nil)
    }

    @Test(arguments: StatusGlyph.allCases)
    func statusGlyphLoadsAsTemplate(glyph: StatusGlyph) throws {
        let image = try #require(UIImage(named: glyph.assetName, in: .module, with: nil))

        #expect(image.renderingMode == .alwaysTemplate)
    }

    private func channels(of color: UIColor) -> [Int] {
        var (r, g, b, a) = (CGFloat.zero, CGFloat.zero, CGFloat.zero, CGFloat.zero)
        color.getRed(&r, green: &g, blue: &b, alpha: &a)
        return [r, g, b, a].map { Int(($0 * 255).rounded()) }
    }
}
