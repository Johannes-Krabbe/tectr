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

public extension Font {
    /// Geist Mono, scaling with the text style like the code roles do.
    static func code(size: CGFloat, relativeTo style: Font.TextStyle) -> Font {
        _ = fontsRegistered
        return .custom(Tokens.codeFontName, size: size, relativeTo: style)
    }
}

public extension UIFont {
    /// Geist Mono at a fixed size, for views drawn by UIKit such as the terminal.
    static func code(size: CGFloat) -> UIFont {
        _ = fontsRegistered
        return UIFont(name: Tokens.codeFontName, size: size) ?? .monospacedSystemFont(ofSize: size, weight: .regular)
    }
}
