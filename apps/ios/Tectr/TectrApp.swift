import SwiftUI
import TectrCore
import TectrDesign

@main
struct TectrApp: App {
    @State private var store: Store
    @State private var toasts = Toasts()
    @Environment(\.scenePhase) private var phase
    @AppStorage(SettingKey.mode) private var mode = AppearanceMode.system
    @AppStorage(SettingKey.darkTheme) private var darkTheme = Theme.tectrDark.id
    @AppStorage(SettingKey.lightTheme) private var lightTheme = Theme.tectrLight.id
    @AppStorage(SettingKey.motionStatusIcons) private var animatesStatusIcons = true
    @AppStorage(SettingKey.motionTranscript) private var animatesTranscript = true
    /// Why the engine couldn't start, which leaves the app with nothing to show.
    private let failure: String?

    init() {
        // The key and the paired devices are this phone's alone, so they stay out of backups.
        var folder = URL.applicationSupportDirectory.appending(path: "tectr", directoryHint: .isDirectory)
        do {
            let engine = try Engine(folder: folder, clientName: "tectr-ios")
            var values = URLResourceValues()
            values.isExcludedFromBackup = true
            try? folder.setResourceValues(values)
            _store = State(initialValue: Store(engine: engine))
            failure = nil
        } catch {
            _store = State(initialValue: Store(
                devices: [], workspaces: [], requests: [], archived: [], conversations: [],
                candidates: [:], folders: [:], transcripts: [:]
            ))
            failure = error.localizedDescription
        }
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(store)
                .environment(toasts)
                .environment(\.animatesStatusIcons, animatesStatusIcons)
                .environment(\.animatesTranscript, animatesTranscript)
                .tectrTheme(
                    mode: mode,
                    dark: store.themes.first { $0.id == darkTheme } ?? .tectrDark,
                    light: store.themes.first { $0.id == lightTheme } ?? .tectrLight
                )
                .onAppear {
                    store.report = { [toasts] in toasts.show($0) }
                    if let failure { toasts.show("The engine didn't start: \(failure)") }
                }
        }
        .onChange(of: phase) { _, phase in
            // iOS drops connections in the background, so coming back tries every device now
            // rather than when its next attempt is due.
            if phase == .active { store.reconnect() }
        }
    }
}
