import SwiftUI
import TectrDesign

struct SettingsSheet: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section("Devices") {
                    ForEach(store.devices) { device in
                        NavigationLink {
                            DeviceView(deviceID: device.id)
                        } label: {
                            HStack(spacing: Tokens.Space.md) {
                                OnlineDot(online: device.online)
                                Text(device.name)
                                Spacer()
                                if let lastSeen = device.lastSeen, !device.online {
                                    Text(lastSeen).font(.subheadline).foregroundStyle(.secondary)
                                }
                            }
                        }
                    }
                    NavigationLink {
                        AddDeviceView()
                    } label: {
                        Label("Add a device", systemImage: "plus")
                    }
                }
                Section("This app") {
                    NavigationLink { AppearanceSettings() } label: { Label("Appearance", systemImage: "paintpalette") }
                    NavigationLink { ConversationSettings() } label: { Label("Conversation", systemImage: "text.bubble") }
                    NavigationLink { TerminalSettings() } label: { Label("Terminal", systemImage: "apple.terminal") }
                    NavigationLink { NotificationSettings() } label: { Label("Notifications", systemImage: "bell.badge") }
                }
                Section {
                    LabeledContent("Version", value: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "")
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button(role: .close) { dismiss() } }
            }
        }
    }
}

struct AppearanceSettings: View {
    @Environment(Store.self) private var store
    @AppStorage(SettingKey.mode) private var mode = AppearanceMode.system
    @AppStorage(SettingKey.darkTheme) private var darkTheme = Theme.tectrDark.id
    @AppStorage(SettingKey.lightTheme) private var lightTheme = Theme.tectrLight.id
    @AppStorage(SettingKey.motionStatusIcons) private var statusIcons = true
    @AppStorage(SettingKey.motionTranscript) private var transcript = true

    var body: some View {
        Form {
            Section {
                Picker("Mode", selection: $mode) {
                    ForEach(AppearanceMode.allCases) { Text($0.label).tag($0) }
                }
            }
            Section("Theme") {
                themeLink("Dark theme", scheme: .dark, selection: $darkTheme)
                themeLink("Light theme", scheme: .light, selection: $lightTheme)
            }
            Section("Motion") {
                Toggle("Status icons", isOn: $statusIcons)
                Toggle("Transcript", isOn: $transcript)
            }
        }
        .navigationTitle("Appearance")
    }

    private func themeLink(_ label: String, scheme: ColorScheme, selection: Binding<String>) -> some View {
        let theme = store.themes.first { $0.id == selection.wrappedValue }
        return NavigationLink {
            ThemePicker(title: label, scheme: scheme, selection: selection)
        } label: {
            LabeledContent(label, value: theme?.name ?? "")
        }
    }
}

/// Picking a theme applies it straight away, so the list itself is the preview.
private struct ThemePicker: View {
    @Environment(Store.self) private var store
    let title: String
    let scheme: ColorScheme
    @Binding var selection: String

    var body: some View {
        List {
            Section {
                ForEach(store.themes.filter { $0.scheme == scheme }) { theme in
                    Button { selection = theme.id } label: {
                        HStack(spacing: Tokens.Space.md) {
                            ThemeSwatch(theme.palette)
                            Text(theme.name).foregroundStyle(.primary)
                            Spacer()
                            if theme.id == selection {
                                Image(systemName: "checkmark").foregroundStyle(.tint).fontWeight(.semibold)
                            }
                        }
                    }
                    .accessibilityAddTraits(theme.id == selection ? .isSelected : [])
                }
            }
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
    }
}

struct ConversationSettings: View {
    @AppStorage(SettingKey.steps) private var steps = StepsDisplay.folded
    @AppStorage(SettingKey.thumbnailsInBox) private var inBox = true
    @AppStorage(SettingKey.thumbnailsInTranscript) private var inTranscript = true

    var body: some View {
        Form {
            Section {
                Picker("Tool calls and thinking", selection: $steps) {
                    ForEach(StepsDisplay.allCases) { Text($0.label).tag($0) }
                }
            }
            Section("Images") {
                Toggle("Thumbnails in the message box", isOn: $inBox)
                Toggle("Thumbnails above your messages", isOn: $inTranscript)
            }
        }
        .navigationTitle("Conversation")
    }
}

struct TerminalSettings: View {
    @AppStorage(SettingKey.terminalSize) private var size = 12.0
    @AppStorage(SettingKey.terminalCursor) private var cursor = CursorShape.block
    @AppStorage(SettingKey.terminalBlink) private var blink = true
    @AppStorage(SettingKey.terminalScrollback) private var scrollback = 10_000

    var body: some View {
        Form {
            Section("Display") {
                Stepper(value: $size, in: 9...20) {
                    LabeledContent("Text size", value: "\(Int(size)) pt")
                }
                Picker("Cursor", selection: $cursor) {
                    ForEach(CursorShape.allCases) { Text($0.label).tag($0) }
                }
                Toggle("Blinking cursor", isOn: $blink)
            }
            Section("Behavior") {
                Picker("History", selection: $scrollback) {
                    ForEach([1_000, 10_000, 100_000], id: \.self) { Text("\($0.formatted()) lines").tag($0) }
                }
            }
        }
        .navigationTitle("Terminal")
    }
}

struct NotificationSettings: View {
    @AppStorage(SettingKey.notifyRequests) private var requests = true
    @AppStorage(SettingKey.notifyDone) private var done = true
    @AppStorage(SettingKey.notifyFailed) private var failed = true

    var body: some View {
        Form {
            Section {
                Toggle("Requests", isOn: $requests)
                Toggle("Finished turns", isOn: $done)
                Toggle("Failures", isOn: $failed)
            }
        }
        .navigationTitle("Notifications")
    }
}
