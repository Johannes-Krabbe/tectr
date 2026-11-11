import SwiftUI
import TectrCore
import TectrDesign

/// The message box, in glass at the bottom of an agent tab. At rest it's a one-line pill with
/// the image menu, the text and Stop while the agent works. Focus, text or images open it into
/// a card: the text above a row with the image menu, the agent's settings and Send. The text
/// field keeps its place in both, so it keeps focus while the box changes shape.
struct Composer: View {
    @Environment(Store.self) private var store
    @AppStorage(SettingKey.thumbnailsInBox) private var thumbnails = true
    let session: Session
    let topicID: Topic.ID

    @State private var text: String
    @State private var attachments = Attachments()
    @State private var sent = 0
    /// The attach panel and pickers, which the chat screen shows, and what they picked, for
    /// this box to add.
    @Binding var attaching: Attaching?
    @Binding var picked: [PreparedImage]
    @FocusState private var focused: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// Between the buttons and the edge.
    private static let inset = Tokens.Space.xs
    /// The pill's corners, which are round on a 44pt button row. The card keeps them, so
    /// opening only changes its size.
    private static let radius = (Tokens.Size.control + 2 * inset) / 2

    init(session: Session, topicID: Topic.ID, attaching: Binding<Attaching?>, picked: Binding<[PreparedImage]>) {
        self.session = session
        self.topicID = topicID
        _attaching = attaching
        _picked = picked
        // The draft is there from the first frame. Filling it in on appear grows the box after
        // the push, which moves the transcript above it.
        _text = State(initialValue: session.draft)
        _attachments = State(initialValue: Attachments(images: session.draftImages))
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Self.radius, style: .continuous)
        VStack(spacing: Tokens.Space.sm) {
            if !suggestions.isEmpty {
                slashCommands
            }
            // No spacing: the buttons' 44pt hit areas already leave room around their glyphs.
            VStack(alignment: .leading, spacing: 0) {
                if isOpen, thumbnails, !images.isEmpty {
                    AttachmentStrip(images: images, topicID: topicID) { attachments.remove($0, from: &text) }
                        .padding(.horizontal, Tokens.Space.sm)
                }
                // One layout for the pill and the card, so opening moves the same views rather
                // than making new ones. A new image button would close the menu it had open.
                ComposerLayout(open: isOpen) {
                    imageMenu
                        .layoutValue(key: ComposerPart.self, value: .leading)
                    TextField("Message the agent", text: $text, axis: .vertical)
                        .font(Tokens.Text.reading)
                        .lineLimit(1...(isOpen ? 8 : 1))
                        .focused($focused)
                        .padding(.horizontal, isOpen ? Tokens.Space.sm : Tokens.Space.xs)
                        .padding(.top, isOpen ? Tokens.Space.sm : 0)
                        // The button row's height in the pill. Open, it would pad out the
                        // space under the text.
                        .frame(maxWidth: .infinity, minHeight: isOpen ? nil : Tokens.Size.control, alignment: .leading)
                        // The space around the text focuses it too. Behind the field, so the
                        // field keeps its own taps, and not over the whole box, where it also
                        // caught taps on the buttons and opened the box under an open menu.
                        .background {
                            Color.clear.contentShape(.rect).onTapGesture { focused = true }
                        }
                        .layoutValue(key: ComposerPart.self, value: .text)
                    if isOpen {
                        SessionSettingsMenu(session: session, provider: provider)
                            .transition(.opacity)
                            .layoutValue(key: ComposerPart.self, value: .settings)
                    }
                    sendButton
                        .layoutValue(key: ComposerPart.self, value: .trailing)
                }
            }
            .padding(Self.inset)
            .glassEffect(.regular.interactive(), in: shape)
        }
        // At rest it sits as far from the sides as from the bottom of the screen, and open it
        // widens to the keyboard's edges.
        .padding(.horizontal, isOpen ? Tokens.Space.md : max(Self.homeIndicator, Tokens.Space.md) + Tokens.Space.sm)
        .padding(.bottom, isOpen ? Tokens.Space.md : Tokens.Space.sm)
        .frame(maxWidth: 760)
        // Typing again closes the panel, as tapping anywhere else does.
        .onChange(of: focused) {
            if focused && attaching == .panel { attaching = nil }
        }
        .onChange(of: picked.count) {
            guard !picked.isEmpty else { return }
            picked.forEach(add)
            picked = []
        }
        .animation(reduceMotion ? .easeOut(duration: Tokens.Motion.fast) : .snappy(duration: Tokens.Motion.normal), value: isOpen)
        .animation(reduceMotion ? nil : .snappy(duration: Tokens.Motion.fast), value: canSend)
        .onChange(of: text) {
            // Clearing after a send isn't a draft, and sending already cleared the device's.
            guard !text.isEmpty || !session.draft.isEmpty else { return }
            store.setDraft(text, images: images, for: session.id)
        }
        .sensoryFeedback(.impact(weight: .light), trigger: sent)
    }

    private var provider: Provider? {
        store.provider(session.agent, on: store.device(of: topicID))
    }

    /// The images the text still names.
    private var images: [PromptImage] { attachments.named(in: text) }

    /// The strip the home indicator keeps clear, which sets the pill's margins. It's the
    /// window's, which unlike SwiftUI's safe area here never includes the keyboard, and it's
    /// known before the first frame, so the pill doesn't move after the push.
    private static var homeIndicator: CGFloat {
        UIApplication.shared.connectedScenes.lazy
            .compactMap { ($0 as? UIWindowScene)?.keyWindow }
            .first?.safeAreaInsets.bottom ?? 0
    }

    private var isOpen: Bool {
        focused || !text.isEmpty || !images.isEmpty
    }

    private var imageMenu: some View {
        AttachButton(attaching: $attaching) { focused = false }
    }

    private var canSend: Bool {
        !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !images.isEmpty
    }

    private func add(_ image: PreparedImage) {
        let sentNumbers = (store.items(for: session.id) ?? []).flatMap { item -> [UInt32] in
            if case .prompt(_, _, let images) = item { images.map(\.number) } else { [] }
        }
        let number = attachments.nextNumber(sent: sentNumbers.max() ?? 0)
        let picked = store.pick(image.data, mime: image.mime, width: image.width, height: image.height, number: number)
        attachments.add(picked, to: &text)
    }

    /// Only there when it does something: Stop while the agent works, Send once there's
    /// something to send.
    @ViewBuilder
    private var sendButton: some View {
        if session.isRunning || canSend {
            sendOrStop
                .transition(.scale(scale: 0.8).combined(with: .opacity))
        }
    }

    private var sendOrStop: some View {
        Group {
            if session.isRunning && !canSend {
                Button("Stop", systemImage: "stop.fill") { store.cancel(session.id) }
                    .keyboardShortcut(".", modifiers: .command)
            } else {
                Button("Send", systemImage: "arrow.up") {
                    if let refused = Engine.refusal(of: text) {
                        store.report(refused)
                        return
                    }
                    store.send(text, images: images, to: session.id)
                    text = ""
                    attachments = Attachments()
                    sent += 1
                }
                .keyboardShortcut(.return, modifiers: .command)
            }
        }
        .labelStyle(.iconOnly)
        .fontWeight(.semibold)
        .buttonStyle(.glassProminent)
        .buttonBorderShape(.circle)
        .tint(ThemeColor.accentSolid)
        .contentTransition(.symbolEffect(.replace))
    }

    /// Typing `/` lists the agent's commands, narrowing as you type.
    private var suggestions: [SlashCommand] {
        guard text.hasPrefix("/"), !text.contains(" ") else { return [] }
        let typed = text.dropFirst()
        return session.commands.filter { typed.isEmpty || $0.name.localizedStandardContains(typed) }
    }

    /// About six rows, then it scrolls. Uncapped, an agent with many commands pushed the box
    /// off the screen.
    private static let commandsHeight: CGFloat = 220

    private var slashCommands: some View {
        let rows = VStack(alignment: .leading, spacing: 0) {
            ForEach(suggestions) { command in
                Button { text = "/\(command.name) " } label: {
                    HStack(alignment: .firstTextBaseline, spacing: Tokens.Space.sm) {
                        Text("/\(command.name)").font(Tokens.Text.codeBody)
                        Text(command.detail)
                            .font(Tokens.Text.meta)
                            .foregroundStyle(.textTertiary)
                            .lineLimit(1)
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, Tokens.Space.lg)
                    .padding(.vertical, Tokens.Space.sm)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.vertical, Tokens.Space.sm)
        return CappedHeight(limit: Self.commandsHeight) {
            ScrollView { rows }
                .scrollBounceBehavior(.basedOnSize)
        }
        .glassEffect(.regular, in: .rect(cornerRadius: Self.radius, style: .continuous))
    }
}

/// As tall as its content, up to `limit`. A plain `maxHeight` frame lets a scroll view take
/// the whole limit, which left a short list floating in a tall box.
private struct CappedHeight: Layout {
    let limit: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        guard let content = subviews.first else { return .zero }
        let ideal = content.sizeThatFits(ProposedViewSize(width: proposal.width, height: nil))
        return CGSize(width: proposal.width ?? ideal.width, height: min(ideal.height, limit))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        subviews.first?.place(at: bounds.origin, proposal: ProposedViewSize(bounds.size))
    }
}

/// Where a view goes in `ComposerLayout`.
private enum ComposerPart: LayoutValueKey {
    case leading, text, settings, trailing

    static let defaultValue = ComposerPart.text
}

/// The composer's parts. Closed, one row: the leading button, the text, the trailing button.
/// Open, the text on top, and below it the leading button, the settings and the trailing button.
private struct ComposerLayout: Layout {
    var open: Bool

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? 320
        return CGSize(width: width, height: frames(in: width, subviews).height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let placed = frames(in: bounds.width, subviews)
        for (subview, frame) in zip(subviews, placed.frames) {
            subview.place(
                at: CGPoint(x: bounds.minX + frame.minX, y: bounds.minY + frame.minY),
                proposal: ProposedViewSize(frame.size)
            )
        }
    }

    /// Each subview's frame, in subview order, and the height they take.
    private func frames(in width: CGFloat, _ subviews: Subviews) -> (frames: [CGRect], height: CGFloat) {
        let sizes = subviews.map { $0[ComposerPart.self] == .text ? CGSize.zero : $0.sizeThatFits(.unspecified) }
        let fixed = { (part: ComposerPart) in
            zip(subviews, sizes).filter { $0.0[ComposerPart.self] == part }.map(\.1.width).reduce(0, +)
        }
        let leading = fixed(.leading)
        let trailing = fixed(.trailing)
        let textWidth = open ? width : max(0, width - leading - trailing)
        let textHeight = subviews.first { $0[ComposerPart.self] == .text }?
            .sizeThatFits(ProposedViewSize(width: textWidth, height: nil)).height ?? 0
        let row = zip(subviews, sizes).filter { $0.0[ComposerPart.self] != .text }.map(\.1.height).max() ?? 0
        let rowTop = open ? textHeight : 0
        let rowHeight = open ? row : max(row, textHeight)
        let frames = zip(subviews, sizes).map { subview, size -> CGRect in
            let y = rowTop + (rowHeight - size.height) / 2
            switch subview[ComposerPart.self] {
            case .text:
                return open
                    ? CGRect(x: 0, y: 0, width: width, height: textHeight)
                    : CGRect(x: leading, y: (rowHeight - textHeight) / 2, width: textWidth, height: textHeight)
            case .leading:
                return CGRect(x: 0, y: y, width: size.width, height: size.height)
            case .settings:
                return CGRect(x: leading, y: y, width: size.width, height: size.height)
            case .trailing:
                return CGRect(x: width - size.width, y: y, width: size.width, height: size.height)
            }
        }
        return (frames, rowTop + rowHeight)
    }
}

/// Model, effort and permission mode for one agent, from what its provider offers.
struct SessionSettingsMenu: View {
    @Environment(Store.self) private var store
    let session: Session
    let provider: Provider?

    var body: some View {
        let model = provider?.model(session.model)
        Menu {
            if let provider {
                Picker("Model", selection: binding(\.model)) {
                    ForEach(provider.models) { Text($0.name).tag(Optional($0.id)) }
                }
                .pickerStyle(.menu)
                if let efforts = model?.efforts, !efforts.isEmpty {
                    Picker("Effort", selection: binding(\.effort)) {
                        ForEach(efforts) { Text($0.name).tag(Optional($0.id)) }
                    }
                    .pickerStyle(.menu)
                }
                Picker("Permission mode", selection: modeBinding) {
                    ForEach(provider.modes) { Text($0.label).tag($0) }
                }
                .pickerStyle(.menu)
            }
        } label: {
            HStack(spacing: Tokens.Space.xs) {
                Text(model?.name ?? "Model")
                if let effort = model?.efforts.first(where: { $0.id == session.effort }) {
                    Text(effort.name).foregroundStyle(.textTertiary)
                }
            }
            .font(Tokens.Text.meta)
            .padding(.horizontal, Tokens.Space.xs)
            .frame(minHeight: Tokens.Size.control)
            .contentShape(.rect)
        }
        // Plain text, like the image menu: the box it's in is the glass.
        .buttonStyle(.borderless)
        .foregroundStyle(.textSecondary)
    }

    private func binding(_ path: WritableKeyPath<Session, String?>) -> Binding<String?> {
        Binding {
            session[keyPath: path]
        } set: { value in
            var changed = session
            changed[keyPath: path] = value
            store.setSettings(model: changed.model, effort: changed.effort, mode: changed.mode, for: session.id)
        }
    }

    private var modeBinding: Binding<PermissionMode> {
        Binding { session.mode } set: {
            store.setSettings(model: session.model, effort: session.effort, mode: $0, for: session.id)
        }
    }
}
