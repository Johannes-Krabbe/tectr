import SwiftTerm
import SwiftUI
import TectrCore
import TectrDesign

/// A terminal tab: the daemon's shell, drawn by SwiftTerm as the desktop draws it with
/// xterm.js. SwiftTerm's own bar above the keyboard has the keys iOS lacks.
struct TerminalTabView: View {
    @Environment(Store.self) private var store
    @Environment(\.palette) private var palette
    @AppStorage(SettingKey.terminalSize) private var size = 12.0
    @AppStorage(SettingKey.terminalCursor) private var cursor = CursorShape.block
    @AppStorage(SettingKey.terminalBlink) private var blink = true
    let terminal: Terminal
    let topicID: Topic.ID
    @State private var typing = false

    var body: some View {
        Group {
            if let link = store.terminalLink(topicID) {
                TerminalScreen(
                    link: link, terminalID: terminal.id, palette: palette, fontSize: size,
                    cursor: cursor, blink: blink, typing: $typing, report: store.report
                )
            } else {
                ContentUnavailableView("Offline", systemImage: "wifi.slash")
            }
        }
        .tectrBackground()
        .safeAreaInset(edge: .bottom) {
            if let note {
                Text(note)
                    .font(Tokens.Text.meta)
                    .foregroundStyle(.textTertiary)
                    .padding(Tokens.Space.sm)
            }
        }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(typing ? "Hide keyboard" : "Show keyboard", systemImage: typing ? "keyboard.chevron.compact.down" : "keyboard") {
                    typing.toggle()
                }
            }
        }
    }

    /// Why typing does something different, for terminals that aren't running.
    private var note: String? {
        switch terminal.status {
        case .running: nil
        case .stopped: "Stopped. Typing starts a shell where the last one was"
        case .exited: "The shell failed. Typing starts another"
        }
    }
}

private struct TerminalScreen: UIViewRepresentable {
    let link: TerminalLink
    let terminalID: Terminal.ID
    let palette: Palette
    let fontSize: Double
    let cursor: CursorShape
    let blink: Bool
    @Binding var typing: Bool
    let report: (String) -> Void

    func makeUIView(context: Context) -> TerminalView {
        let view = TerminalView(frame: .zero)
        view.terminalDelegate = context.coordinator
        view.optionAsMetaKey = true
        context.coordinator.view = view
        return view
    }

    func updateUIView(_ view: TerminalView, context: Context) {
        let coordinator = context.coordinator
        coordinator.report = report
        if view.font.pointSize != fontSize { view.font = .code(size: fontSize) }
        if coordinator.palette != palette {
            coordinator.palette = palette
            paint(view)
        }
        view.getTerminal().setCursorStyle(cursorStyle)
        // Only the button moves the keyboard. Tapping the terminal brings it up too, and a
        // redraw for something else mustn't take it away.
        if coordinator.typing != typing {
            coordinator.typing = typing
            _ = typing ? view.becomeFirstResponder() : view.resignFirstResponder()
        }
        coordinator.attachIfSized()
    }

    static func dismantleUIView(_ view: TerminalView, coordinator: Coordinator) {
        coordinator.detach()
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(link: link, terminalID: terminalID, colors: TerminalScreen.colors(palette))
    }

    private var cursorStyle: CursorStyle {
        switch (cursor, blink) {
        case (.block, true): .blinkBlock
        case (.block, false): .steadyBlock
        case (.bar, true): .blinkBar
        case (.bar, false): .steadyBar
        case (.underline, true): .blinkUnderline
        case (.underline, false): .steadyUnderline
        }
    }

    private func paint(_ view: TerminalView) {
        view.nativeBackgroundColor = UIColor(palette.surfaceBase)
        view.nativeForegroundColor = UIColor(palette.textPrimary)
        view.caretColor = UIColor(palette.textPrimary)
        view.selectedTextBackgroundColor = UIColor(palette.accentText).withAlphaComponent(0.3)
        view.installColors(Self.ansi(palette).map { color in
            let (red, green, blue) = color.components
            return SwiftTerm.Color(red: UInt16(red * 65535), green: UInt16(green * 65535), blue: UInt16(blue * 65535))
        })
    }

    private static func ansi(_ palette: Palette) -> [UIColor] {
        [
            palette.terminalBlack, palette.terminalRed, palette.terminalGreen, palette.terminalYellow,
            palette.terminalBlue, palette.terminalMagenta, palette.terminalCyan, palette.terminalWhite,
            palette.terminalBrightBlack, palette.terminalBrightRed, palette.terminalBrightGreen, palette.terminalBrightYellow,
            palette.terminalBrightBlue, palette.terminalBrightMagenta, palette.terminalBrightCyan, palette.terminalBrightWhite,
        ].map { UIColor($0) }
    }

    /// The theme's colors, for programs that ask the terminal what they are.
    static func colors(_ palette: Palette) -> Wire.TerminalColors {
        Wire.TerminalColors(
            foreground: UIColor(palette.textPrimary).hex,
            background: UIColor(palette.surfaceBase).hex,
            cursor: UIColor(palette.textPrimary).hex,
            palette: ansi(palette).map(\.hex)
        )
    }

    @MainActor
    final class Coordinator: NSObject, @preconcurrency TerminalViewDelegate {
        let link: TerminalLink
        let terminalID: Terminal.ID
        let colors: Wire.TerminalColors
        weak var view: TerminalView?
        var palette: Palette?
        var report: (String) -> Void = { _ in }
        var typing = false
        private var attached = false
        private let frames = FrameQueue()

        init(link: TerminalLink, terminalID: Terminal.ID, colors: Wire.TerminalColors) {
            self.link = link
            self.terminalID = terminalID
            self.colors = colors
        }

        /// Attaches once the view knows how many columns and rows fit, which the device sizes
        /// the shell to.
        func attachIfSized() {
            guard !attached, let view, view.bounds.width > 0 else { return }
            let terminal = view.getTerminal()
            guard terminal.cols > 1, terminal.rows > 1 else { return }
            attached = true
            let frames = frames
            let link = link
            let terminalID = terminalID
            frames.drain = { [weak self] batch in self?.draw(batch) }
            link.engine.attachTerminal(
                link.machine, terminalID, cols: UInt16(clamping: terminal.cols), rows: UInt16(clamping: terminal.rows),
                colors: colors
            ) { kind, payload in
                frames.push(kind, payload)
            }
        }

        func detach() {
            guard attached else { return }
            attached = false
            link.engine.detachTerminal(link.machine, terminalID)
        }

        private func draw(_ batch: [(UInt8, Data)]) {
            guard let view else { return }
            var drawn = 0
            for (kind, payload) in batch {
                switch kind {
                case 1:
                    view.feed(byteArray: [UInt8](payload)[...])
                case 2 where payload.count >= 4:
                    // Start over: the size it was drawn at, then what recreates the screen.
                    let cols = Int(payload[payload.startIndex]) | Int(payload[payload.startIndex + 1]) << 8
                    let rows = Int(payload[payload.startIndex + 2]) | Int(payload[payload.startIndex + 3]) << 8
                    let terminal = view.getTerminal()
                    terminal.resetToInitialState()
                    if cols != terminal.cols || rows != terminal.rows { terminal.resize(cols: cols, rows: rows) }
                    view.feed(byteArray: [UInt8](payload.dropFirst(4))[...])
                default:
                    continue
                }
                drawn += payload.count
            }
            if drawn > 0 { link.engine.terminalAck(link.machine, terminalID, bytes: UInt32(clamping: drawn)) }
        }

        func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
            if attached {
                link.engine.resizeTerminal(link.machine, terminalID, cols: UInt16(clamping: newCols), rows: UInt16(clamping: newRows))
            } else {
                attachIfSized()
            }
        }

        func send(source: TerminalView, data: ArraySlice<UInt8>) {
            do {
                try link.engine.terminalInput(link.machine, terminalID, Data(data))
            } catch {
                report(error.localizedDescription)
            }
        }

        func setTerminalTitle(source: TerminalView, title: String) {}
        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
        func scrolled(source: TerminalView, position: Double) {}
        func bell(source: TerminalView) {}
        func iTermContent(source: TerminalView, content: ArraySlice<UInt8>) {}
        func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}

        func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {
            // Programs write these links, so only web addresses open.
            guard let url = URL(string: link), ["http", "https"].contains(url.scheme) else { return }
            UIApplication.shared.open(url)
        }

        func clipboardCopy(source: TerminalView, content: Data) {
            UIPasteboard.general.string = String(decoding: content, as: UTF8.self)
        }
    }
}

/// Frames come in on the engine's threads. They wait here and are drawn together on the main
/// thread, so a flood of output is a few redraws instead of thousands.
private final class FrameQueue: @unchecked Sendable {
    private let lock = NSLock()
    private var pending: [(UInt8, Data)] = []
    private var scheduled = false
    var drain: @MainActor ([(UInt8, Data)]) -> Void = { _ in }

    func push(_ kind: UInt8, _ payload: Data) {
        let schedule = lock.withLock { () -> Bool in
            pending.append((kind, payload))
            defer { scheduled = true }
            return !scheduled
        }
        guard schedule else { return }
        DispatchQueue.main.async { [self] in
            let batch = lock.withLock { () -> [(UInt8, Data)] in
                scheduled = false
                defer { pending = [] }
                return pending
            }
            MainActor.assumeIsolated { drain(batch) }
        }
    }
}

private extension UIColor {
    var components: (Double, Double, Double) {
        var red: CGFloat = 0
        var green: CGFloat = 0
        var blue: CGFloat = 0
        getRed(&red, green: &green, blue: &blue, alpha: nil)
        return (Double(red), Double(green), Double(blue))
    }

    /// `#rrggbb`, which is what the daemon takes.
    var hex: String {
        let (red, green, blue) = components
        let byte = { (value: Double) in Int((min(max(value, 0), 1) * 255).rounded()) }
        return String(format: "#%02x%02x%02x", byte(red), byte(green), byte(blue))
    }
}
