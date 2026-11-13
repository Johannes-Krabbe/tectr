import CoreText
import SwiftUI
import Testing
import UIKit
@testable import TectrDesign

@Suite struct TectrDesignTests {
    @Test func hexColorsRead() {
        #expect(channels(of: UIColor(Color(hex: "#112233")!)) == [0x11, 0x22, 0x33, 0xff])
        #expect(channels(of: UIColor(Color(hex: "#44556680")!)) == [0x44, 0x55, 0x66, 0x80])
        #expect(Color(hex: "112233") == nil)
        #expect(Color(hex: "#12345") == nil)
    }

    @Test func aThemeNeedsEveryColor() {
        var colors = Dictionary(uniqueKeysWithValues: ColorRole.allCases.map { ("\($0)", "#000000") })
        #expect(Palette(colors: colors) == nil, "keys are token names, not Swift names")

        colors = ["surface.base": "#09080c"]
        #expect(Theme(id: "partial", name: "Partial", scheme: .dark, colors: colors) == nil)
    }

    @Test func colorsComeFromTheThemeInEffect() {
        var environment = EnvironmentValues()
        environment.palette = .tectrLight
        #expect(ThemeColor.surfaceBase.resolve(in: environment) == Palette.tectrLight.surfaceBase)
        environment.palette = .tectrDark
        #expect(ThemeColor.surfaceBase.resolve(in: environment) == Palette.tectrDark.surfaceBase)
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
