import SwiftUI
import TectrDesign

// Every screen on the mock store, for judging the look in Xcode without running the app.

private extension View {
    func previewed() -> some View {
        environment(Store.preview())
            .environment(Toasts())
            .tectrTheme()
    }
}

#Preview("Home") {
    RootView().previewed()
}

#Preview("Topic waiting") {
    NavigationStack { TopicView(topicID: "reconnect") }.previewed()
}

#Preview("Topic failed") {
    NavigationStack { TopicView(topicID: "markdown") }.previewed()
}

#Preview("Terminal") {
    NavigationStack { TerminalTabView(terminal: Store.preview().topic("reconnect")!.tabs.compactMap {
        if case .terminal(let terminal) = $0 { terminal } else { nil }
    }[0], topicID: "reconnect") }.previewed()
}

#Preview("New topic") {
    NavigationStack { NewTopicView(workspaceID: "tectr-laptop") { _ in } }.previewed()
}

#Preview("Settings") {
    SettingsSheet().previewed()
}

#Preview("Light, large text") {
    RootView()
        .previewed()
        .preferredColorScheme(.light)
        .dynamicTypeSize(.xxxLarge)
}
