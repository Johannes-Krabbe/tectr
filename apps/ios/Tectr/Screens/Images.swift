import PhotosUI
import SwiftUI
import TectrDesign

/// An image picked here, ready to go to a device: scaled down and re-encoded only when it has
/// to be, as the desktop does.
struct PreparedImage {
    let data: Data
    let mime: String
    let width: Int
    let height: Int

    /// Bigger images are scaled down to this on their long side. Models scale them down further
    /// anyway, so this only saves time and memory.
    private static let maxEdge: CGFloat = 2000
    /// The daemon takes 5 MB, but model APIs count the base64, which is a third bigger.
    private static let maxBytes = 3_750_000

    init?(_ data: Data) {
        guard let image = UIImage(data: data) else { return nil }
        let size = CGSize(width: image.size.width * image.scale, height: image.size.height * image.scale)
        let scale = min(1, Self.maxEdge / max(size.width, size.height))
        let mime = Self.mime(of: data)
        if scale == 1, let mime, data.count <= Self.maxBytes {
            self.init(data: data, mime: mime, width: Int(size.width), height: Int(size.height))
            return
        }
        let target = CGSize(width: max(1, (size.width * scale).rounded()), height: max(1, (size.height * scale).rounded()))
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let scaled = UIGraphicsImageRenderer(size: target, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: target))
        }
        if let png = scaled.pngData(), png.count <= Self.maxBytes {
            self.init(data: png, mime: "image/png", width: Int(target.width), height: Int(target.height))
        } else if let jpeg = scaled.jpegData(compressionQuality: 0.85), jpeg.count <= Self.maxBytes {
            self.init(data: jpeg, mime: "image/jpeg", width: Int(target.width), height: Int(target.height))
        } else {
            return nil
        }
    }

    private init(data: Data, mime: String, width: Int, height: Int) {
        self.data = data
        self.mime = mime
        self.width = width
        self.height = height
    }

    /// The types agents take, by their first bytes. Anything else is re-encoded.
    private static func mime(of data: Data) -> String? {
        let bytes = [UInt8](data.prefix(12))
        if bytes.starts(with: [0x89, 0x50, 0x4E, 0x47]) { return "image/png" }
        if bytes.starts(with: [0xFF, 0xD8, 0xFF]) { return "image/jpeg" }
        if bytes.starts(with: [0x47, 0x49, 0x46]) { return "image/gif" }
        if bytes.count >= 12, bytes[0...3] == [0x52, 0x49, 0x46, 0x46], bytes[8...11] == [0x57, 0x45, 0x42, 0x50] {
            return "image/webp"
        }
        return nil
    }
}

/// The images a message refers to and the text that refers to them. The text is the source of
/// truth, as on the desktop: deleting an image's token drops the image.
struct Attachments: Equatable {
    var images: [PromptImage] = []

    /// The images the text still names.
    func named(in text: String) -> [PromptImage] {
        images.filter { text.contains($0.token) }
    }

    /// Numbers count up through a session, so `sent` is the highest it has sent so far.
    func nextNumber(sent: UInt32) -> UInt32 {
        max(sent, images.map(\.number).max() ?? 0) + 1
    }

    /// Adds an image and its token at the end of the text.
    mutating func add(_ image: PromptImage, to text: inout String) {
        images.append(image)
        let gap = text.isEmpty || text.hasSuffix(" ") || text.hasSuffix("\n") ? "" : " "
        text += "\(gap)\(image.token) "
    }

    mutating func remove(_ image: PromptImage, from text: inout String) {
        images.removeAll { $0.id == image.id }
        text = text.replacingOccurrences(of: "\(image.token) ", with: "").replacingOccurrences(of: image.token, with: "")
    }
}

/// What adding an image shows: the panel of ways to add one, or the picker chosen from it.
enum Attaching {
    case panel, library, camera
}

/// The + of a message box. It opens the attach panel, and closes the keyboard, so a picker
/// chosen from the panel never opens while the keyboard is moving. SwiftUI closed pickers
/// opened then straight away.
struct AttachButton: View {
    @Binding var attaching: Attaching?
    /// Called on the way in, to take the text field's focus.
    let unfocus: () -> Void

    var body: some View {
        Button("Add an image", systemImage: "plus") {
            unfocus()
            attaching = attaching == .panel ? nil : .panel
        }
        .labelStyle(.iconOnly)
        .font(.title3)
        .frame(width: Tokens.Size.control, height: Tokens.Size.control)
        .contentShape(.circle)
        .foregroundStyle(.textPrimary)
        .buttonStyle(.borderless)
        .anchorPreference(key: AttachAnchor.self, value: .bounds) { $0 }
    }
}

/// Where the + is, which the panel opens from.
private struct AttachAnchor: PreferenceKey {
    static let defaultValue: Anchor<CGRect>? = nil

    static func reduce(value: inout Anchor<CGRect>?, nextValue: () -> Anchor<CGRect>?) {
        value = value ?? nextValue()
    }
}

extension View {
    /// The attach panel, opening above the +, and the pickers it opens. Apply it to the whole
    /// screen, after its bottom bar: from inside the bar, the pickers were taken away again as
    /// the box changed shape. `add` gets each image, prepared.
    func attachPanel(_ attaching: Binding<Attaching?>, add: @escaping (PreparedImage) -> Void) -> some View {
        modifier(AttachPanel(attaching: attaching, add: add))
    }
}

private struct AttachPanel: ViewModifier {
    @Environment(Toasts.self) private var toasts
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Binding var attaching: Attaching?
    let add: (PreparedImage) -> Void
    @State private var picked: [PhotosPickerItem] = []

    func body(content: Content) -> some View {
        content
            .overlayPreferenceValue(AttachAnchor.self) { anchor in
                GeometryReader { proxy in
                    ZStack(alignment: .bottomLeading) {
                        if attaching == .panel {
                            // Its corner meets the box's: the + sits that far in from the box's
                            // edge, and the panel sits a gap above the box. Without the +, just
                            // above the bar.
                            let button = anchor.map { proxy[$0] }
                            // A tap anywhere else closes it, like a menu.
                            Color.clear
                                .contentShape(.rect)
                                .onTapGesture { attaching = nil }
                            panel
                                .padding(.leading, button.map { $0.minX - Self.buttonInset } ?? Tokens.Space.md)
                                .padding(.bottom, button.map { proxy.size.height - $0.minY + Self.buttonInset + Tokens.Space.sm } ?? Tokens.Space.sm)
                                .transition(.scale(scale: 0.85, anchor: .bottomLeading).combined(with: .opacity))
                        }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
                    .animation(reduceMotion ? .easeOut(duration: Tokens.Motion.fast) : .snappy(duration: Tokens.Motion.normal), value: attaching == .panel)
                }
            }
            .photosPicker(isPresented: showing(.library), selection: $picked, matching: .images)
            .fullScreenCover(isPresented: showing(.camera)) {
                CameraPicker { prepare($0, toasts: toasts, add: add) }
                    .ignoresSafeArea()
            }
            .onChange(of: picked) {
                let items = picked
                picked = []
                Task {
                    for item in items {
                        prepare(try? await item.loadTransferable(type: Data.self), toasts: toasts, add: add)
                    }
                }
            }
    }

    /// How far the + sits inside the box's edge.
    private static let buttonInset = Tokens.Space.xs
    private static let circle: CGFloat = 44
    /// Around the rows. The corners are that plus half a circle, so they curve evenly around
    /// the circles in the corners.
    private static let padding = Tokens.Space.lg + Tokens.Space.xs

    private var panel: some View {
        VStack(alignment: .leading, spacing: Tokens.Space.lg) {
            if UIImagePickerController.isSourceTypeAvailable(.camera) {
                row("Camera", systemImage: "camera") { attaching = .camera }
            }
            row("Photos", systemImage: "photo.on.rectangle") { attaching = .library }
            row("Paste", systemImage: "doc.on.clipboard", enabled: UIPasteboard.general.hasImages) {
                attaching = nil
                prepare(UIPasteboard.general.image?.pngData(), toasts: toasts, add: add)
            }
        }
        .padding(Self.padding)
        .frame(width: 260, alignment: .leading)
        .glassEffect(.regular, in: .rect(cornerRadius: Self.padding + Self.circle / 2, style: .continuous))
    }

    private func row(_ title: String, systemImage: String, enabled: Bool = true, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: Tokens.Space.lg) {
                Image(systemName: systemImage)
                    .font(.body.weight(.medium))
                    .frame(width: Self.circle, height: Self.circle)
                    .background(.textPrimary.opacity(0.08), in: .circle)
                Text(title).font(.title3)
                Spacer(minLength: 0)
            }
            .foregroundStyle(enabled ? .textPrimary : .textTertiary)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }

    private func showing(_ kind: Attaching) -> Binding<Bool> {
        Binding { attaching == kind } set: { if !$0 && attaching == kind { attaching = nil } }
    }
}

@MainActor
private func prepare(_ data: Data?, toasts: Toasts, add: (PreparedImage) -> Void) {
    guard let data, let prepared = PreparedImage(data) else {
        toasts.show("That image couldn't be read, or is too big to send")
        return
    }
    add(prepared)
}

/// The system camera, for a photo to send.
private struct CameraPicker: UIViewControllerRepresentable {
    let take: (Data?) -> Void
    @Environment(\.dismiss) private var dismiss

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ picker: UIImagePickerController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    final class Coordinator: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        let parent: CameraPicker

        init(_ parent: CameraPicker) {
            self.parent = parent
        }

        func imagePickerController(_ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
            let image = info[.originalImage] as? UIImage
            parent.take(image?.jpegData(compressionQuality: 0.9))
            parent.dismiss()
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
            parent.dismiss()
        }
    }
}

/// Picked images above a message box, each with a way to remove it.
struct AttachmentStrip: View {
    let images: [PromptImage]
    /// Where images picked on another client are fetched from.
    let topicID: Topic.ID?
    let remove: (PromptImage) -> Void
    @ScaledMetric(relativeTo: .callout) private var size: CGFloat = 44

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: Tokens.Space.sm) {
                ForEach(images, id: \.id) { image in
                    RemoteImage(image: image, topicID: topicID, thumbnail: true)
                        .frame(width: size, height: size)
                        .clipShape(.rect(cornerRadius: Tokens.Radius.sm))
                        .overlay(alignment: .topTrailing) {
                            Button("Remove", systemImage: "xmark.circle.fill") { remove(image) }
                                .labelStyle(.iconOnly)
                                .symbolRenderingMode(.hierarchical)
                                .foregroundStyle(.textSecondary)
                                .offset(x: 6, y: -6)
                        }
                }
            }
            .padding(.top, 6)
        }
    }
}
