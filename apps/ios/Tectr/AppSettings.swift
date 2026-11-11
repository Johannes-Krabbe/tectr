import SwiftUI

/// This app's own settings, kept on the phone. Names match `dsk.toml` where the desktop has the same setting.
enum SettingKey {
    static let mode = "appearance.mode"
    static let darkTheme = "appearance.dark_theme"
    static let lightTheme = "appearance.light_theme"
    static let motionStatusIcons = "appearance.motion.status_icons"
    static let motionTranscript = "appearance.motion.transcript"
    static let steps = "conversation.steps"
    static let thumbnailsInBox = "conversation.images.message_box"
    static let thumbnailsInTranscript = "conversation.images.transcript"
    static let terminalSize = "terminal.size"
    static let terminalCursor = "terminal.cursor"
    static let terminalBlink = "terminal.cursor_blink"
    static let terminalScrollback = "terminal.scrollback"
    static let notifyRequests = "notifications.requests"
    static let notifyDone = "notifications.done"
    static let notifyFailed = "notifications.failed"
}

/// `conversation.steps`.
enum StepsDisplay: String, CaseIterable, Identifiable {
    case folded, shown

    var id: Self { self }

    var label: String {
        switch self {
        case .folded: "Folded into one line"
        case .shown: "All shown"
        }
    }
}

/// `terminal.cursor`.
enum CursorShape: String, CaseIterable, Identifiable {
    case block, bar, underline

    var id: Self { self }

    var label: String {
        switch self {
        case .block: "Block"
        case .bar: "Bar"
        case .underline: "Underline"
        }
    }
}
