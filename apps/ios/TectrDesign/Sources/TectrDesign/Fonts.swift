import CoreText
import SwiftUI

// Info.plist font lists only reach the app's main bundle, so package fonts register in code.
// Doing it here means nothing has to remember to call it, including SwiftUI previews.
private let fontsRegistered: Void = {
    let urls = Bundle.module.urls(forResourcesWithExtension: "ttf", subdirectory: "Fonts") ?? []
    for url in urls {
        CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
    }
}()

extension Font {
    static func code(size: CGFloat, relativeTo style: Font.TextStyle) -> Font {
        _ = fontsRegistered
        return .custom(Tokens.codeFontName, size: size, relativeTo: style)
    }
}
