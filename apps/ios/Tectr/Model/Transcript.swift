import Foundation
import TectrCore
import TectrDesign

/// Folds a session's events into what a person reads, as the desktop's `foldTranscript` does:
/// streamed chunks become one message, and everything an agent does between two things it
/// says becomes one activity.
struct TranscriptFold {
    private(set) var events: [Wire.SessionEvent] = []

    var lastSeq: UInt64 { events.last?.seq ?? 0 }

    mutating func append(_ more: [Wire.SessionEvent]) {
        events.append(contentsOf: more.filter { $0.seq > lastSeq })
    }

    func items(status: SessionStatus) -> [TranscriptItem] {
        var builder = Builder()
        for event in events { builder.add(event) }
        return builder.finish(status: status)
    }
}

private struct Builder {
    private struct OpenActivity {
        let key: UInt64
        var steps: [StepDraft] = []
        var startedMs: Int64
        var endedMs: Int64
        var waitedMs: Int64 = 0
    }

    private enum StepDraft {
        case thought(key: UInt64, text: String)
        case tool(key: UInt64, id: String, tool: Wire.ToolCallPatch)
        case note(key: UInt64, text: String)
    }

    private enum Item {
        case prompt(key: UInt64, text: String, images: [PromptImage])
        case message(key: UInt64, text: String, forkAfter: UInt64?)
        /// Its index in `activities`.
        case activity(Int)
        case note(key: UInt64, text: String, failed: Bool)
    }

    private var items: [Item] = []
    private var activities: [OpenActivity] = []
    private var open: Int?
    private var previousAt: Int64?
    private var requests: [String: (title: String, options: [String: String])] = [:]
    private var requestsOpen = 0
    private var waitingSince: Int64 = 0

    private static let stopNotes = [
        "cancelled": "Interrupted",
        "max_tokens": "Stopped at the token limit",
        "max_turn_requests": "Stopped at the turn limit",
        "refusal": "The agent refused",
    ]

    /// The activity in progress, started if there's none. It began when the agent went
    /// quiet, not when its first step showed up.
    private mutating func activity(_ seq: UInt64, _ at: Int64) -> Int {
        if let open {
            activities[open].endedMs = at
            return open
        }
        activities.append(OpenActivity(key: seq, startedMs: previousAt ?? at, endedMs: at))
        items.append(.activity(activities.count - 1))
        open = activities.count - 1
        return activities.count - 1
    }

    private mutating func touch(_ at: Int64) {
        if let open { activities[open].endedMs = at }
    }

    private mutating func close(_ at: Int64) {
        touch(at)
        open = nil
    }

    mutating func add(_ event: Wire.SessionEvent) {
        let seq = event.seq
        let at = event.atMs
        switch event.kind {
        case .userPrompt(let text, let images):
            close(at)
            items.append(.prompt(key: seq, text: text, images: images))
        case .agentMessage(let text):
            close(at)
            if case .message(let key, let previous, let fork) = items.last {
                items[items.count - 1] = .message(key: key, text: previous + text, forkAfter: fork)
            } else {
                items.append(.message(key: seq, text: text, forkAfter: nil))
            }
        case .agentThought(let text):
            let index = activity(seq, at)
            if case .thought(let key, let previous) = activities[index].steps.last {
                activities[index].steps[activities[index].steps.count - 1] = .thought(key: key, text: previous + text)
            } else {
                activities[index].steps.append(.thought(key: seq, text: text))
            }
        case .toolCall(let id, let tool), .toolCallUpdate(let id, let tool):
            if let (index, step) = findTool(id) {
                if case .tool(let key, _, let older) = activities[index].steps[step] {
                    activities[index].steps[step] = .tool(key: key, id: id, tool: tool.applied(over: older))
                }
                // Tools can finish after a message, and that doesn't reopen their activity.
                touch(at)
            } else {
                let index = activity(seq, at)
                activities[index].steps.append(.tool(key: seq, id: id, tool: tool))
            }
        case .permissionRequested(let approval):
            requests[approval.id] = (
                approval.toolCall.title ?? "Untitled tool call",
                Dictionary(approval.options.map { ($0.id, $0.name) }, uniquingKeysWith: { first, _ in first })
            )
            _ = activity(seq, at)
            if requestsOpen == 0 { waitingSince = at }
            requestsOpen += 1
        case .permissionResolved(let approvalID, let optionID):
            let request = requests[approvalID]
            let answer = optionID.map { request?.options[$0] ?? "Answered" } ?? "Cancelled"
            let index = activity(seq, at)
            requestsOpen -= 1
            if requestsOpen == 0 { activities[index].waitedMs += at - waitingSince }
            activities[index].steps.append(.note(key: seq, text: "Request: \(request?.title ?? "tool call"). \(answer)"))
        case .agentReady:
            close(at)
        case .turnEnded(let reason, let lastMessage):
            close(at)
            if lastMessage != nil, let index = items.lastIndex(where: { if case .message = $0 { true } else { false } }),
               case .message(let key, let text, _) = items[index] {
                items[index] = .message(key: key, text: text, forkAfter: seq)
            }
            if let note = Self.stopNotes[reason] {
                items.append(.note(key: seq, text: note, failed: false))
            }
        case .error(let message):
            close(at)
            items.append(.note(key: seq, text: message, failed: true))
        case .statusChanged, .unknown:
            break
        }
        previousAt = at
    }

    private func findTool(_ id: String) -> (Int, Int)? {
        for (index, activity) in activities.enumerated().reversed() {
            if let step = activity.steps.firstIndex(where: { if case .tool(_, id, _) = $0 { true } else { false } }) {
                return (index, step)
            }
        }
        return nil
    }

    func finish(status: SessionStatus) -> [TranscriptItem] {
        let running = status == .working || status == .starting || status == .waiting
        let lastActivity = items.lastIndex { if case .activity = $0 { true } else { false } }
        var result: [TranscriptItem] = items.enumerated().map { offset, item in
            switch item {
            case .prompt(let key, let text, let images):
                return .prompt(id: "\(key)", text: text, images: images)
            case .message(let key, let text, let fork):
                return .message(id: "\(key)", blocks: MessageBlock.parse(text), forkAfter: fork)
            case .note(let key, let text, let failed):
                return .note(id: "\(key)", text: text, failed: failed)
            case .activity(let index):
                let activity = activities[index]
                // Only the last activity of a running turn is still going.
                let live = running && offset == lastActivity && offset == items.count - 1
                return .activity(Activity(
                    id: "\(activity.key)",
                    state: live ? Self.liveState(activity, status: status) : Self.summary(activity),
                    steps: activity.steps.map(Self.step)
                ))
            }
        }
        // A turn that has said nothing yet still shows that it's under way.
        if running, lastActivity != items.count - 1 || items.isEmpty {
            let state: ActivityState = status == .waiting ? .waiting : .running(status == .starting ? "Starting" : "Working")
            result.append(.activity(Activity(id: "live", state: state, steps: [])))
        }
        return result
    }

    /// What a running session is on right now. Parallel tools show the newest one still running,
    /// and a step stays until the next one replaces it.
    private static func liveState(_ activity: OpenActivity, status: SessionStatus) -> ActivityState {
        if status == .waiting { return .waiting }
        if status == .starting { return .running("Starting") }
        let steps = activity.steps.filter { if case .note = $0 { false } else { true } }
        let running = steps.filter {
            if case .tool(_, _, let tool) = $0 { tool.status != .completed && tool.status != .failed } else { true }
        }
        switch running.last ?? steps.last {
        case .tool(_, _, let tool): return .running(tool.title ?? "Tool call")
        case .thought: return .running("Thinking")
        case .note, nil: return .running("Working")
        }
    }

    private static func summary(_ activity: OpenActivity) -> ActivityState {
        let worked = formatWorked(activity.endedMs - activity.startedMs - activity.waitedMs)
        let counted = activity.steps.filter { if case .note = $0 { false } else { true } }
        let failed = counted.filter { if case .tool(_, _, let tool) = $0 { tool.status == .failed } else { false } }.count
        if !counted.isEmpty, counted.allSatisfy({ if case .thought = $0 { true } else { false } }) {
            return .finished("Thought for \(worked)", failed: failed)
        }
        let count = counted.count == 1 ? "1 step" : "\(counted.count) steps"
        return .finished("Worked for \(worked), \(count)", failed: failed)
    }

    private static func step(_ draft: StepDraft) -> Step {
        switch draft {
        case .thought(let key, let text):
            return Step(id: "\(key)", kind: "think", title: text.trimmingCharacters(in: .whitespacesAndNewlines), status: .done)
        case .note(let key, let text):
            return Step(id: "\(key)", kind: "note", title: text, status: .done)
        case .tool(let key, _, let tool):
            var diff: [DiffLine] = []
            var output: [String] = []
            for content in tool.content ?? [] {
                switch content {
                case .diff(_, let old, let new): diff += DiffLine.between(old ?? "", new)
                case .text(let text): output.append(text)
                case .unknown: break
                }
            }
            let status: ToolCallRow.Status = switch tool.status {
            case .completed: .done
            case .failed: .failed
            default: .running
            }
            return Step(
                id: "\(key)", kind: tool.kind ?? "other", title: tool.title ?? "Tool call", status: status,
                diff: diff, output: output.isEmpty ? nil : output.joined(separator: "\n")
            )
        }
    }
}

/// A length of time in the app's short form, never less than a second.
func formatWorked(_ ms: Int64) -> String {
    let seconds = max(1, Int((Double(ms) / 1000).rounded()))
    if seconds < 60 { return "\(seconds)s" }
    let minutes = seconds / 60
    if minutes < 60 { return "\(minutes)m \(seconds % 60)s" }
    return "\(minutes / 60)h \(minutes % 60)m"
}

/// A time in the past, in the app's short form.
func ago(_ ms: Int64) -> String {
    let minutes = Int((Date().timeIntervalSince1970 * 1000 - Double(ms)) / 60_000)
    if minutes < 1 { return "just now" }
    if minutes < 60 { return "\(minutes)m ago" }
    let hours = minutes / 60
    return hours < 24 ? "\(hours)h ago" : "\(hours / 24)d ago"
}

extension MessageBlock {
    /// An agent's markdown as blocks: fenced code, headings, lists and paragraphs. What's
    /// inside a block stays markdown, which the view renders inline.
    static func parse(_ text: String) -> [MessageBlock] {
        var blocks: [MessageBlock] = []
        var paragraph: [String] = []
        var bullets: [String] = []
        var code: (language: String?, lines: [String])?

        func flush() {
            if !paragraph.isEmpty {
                blocks.append(.paragraph(paragraph.joined(separator: "\n")))
                paragraph = []
            }
            if !bullets.isEmpty {
                blocks.append(.bullets(bullets))
                bullets = []
            }
        }

        for line in text.components(separatedBy: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if var open = code {
                if trimmed.hasPrefix("```") {
                    blocks.append(.code(language: open.language, text: open.lines.joined(separator: "\n")))
                    code = nil
                } else {
                    open.lines.append(line)
                    code = open
                }
                continue
            }
            if trimmed.hasPrefix("```") {
                flush()
                let language = trimmed.dropFirst(3).trimmingCharacters(in: .whitespaces)
                code = (language.isEmpty ? nil : language, [])
            } else if trimmed.isEmpty {
                flush()
            } else if trimmed.hasPrefix("#") {
                flush()
                blocks.append(.heading(trimmed.drop { $0 == "#" }.trimmingCharacters(in: .whitespaces)))
            } else if let item = Self.listItem(trimmed) {
                if !paragraph.isEmpty { flush() }
                bullets.append(item)
            } else if !bullets.isEmpty, line.hasPrefix(" ") {
                // A list item carried onto the next line.
                bullets[bullets.count - 1] += " " + trimmed
            } else {
                if !bullets.isEmpty { flush() }
                paragraph.append(line)
            }
        }
        // A fence still open is code that's still streaming.
        if let code { blocks.append(.code(language: code.language, text: code.lines.joined(separator: "\n"))) }
        flush()
        return blocks
    }

    private static func listItem(_ line: String) -> String? {
        for marker in ["- ", "* ", "+ "] where line.hasPrefix(marker) {
            return String(line.dropFirst(marker.count))
        }
        let digits = line.prefix { $0.isNumber }
        if !digits.isEmpty, line.dropFirst(digits.count).hasPrefix(". ") {
            return "\(digits). " + line.dropFirst(digits.count + 2)
        }
        return nil
    }
}

extension DiffLine {
    /// The changed lines between two versions of a file, with a few unchanged lines around each.
    static func between(_ old: String, _ new: String, context: Int = 3) -> [DiffLine] {
        let before = old.isEmpty ? [] : old.components(separatedBy: "\n")
        let after = new.components(separatedBy: "\n")
        let difference = after.difference(from: before)
        var removed = Set<Int>()
        var inserted = Set<Int>()
        for change in difference {
            switch change {
            case .remove(let offset, _, _): removed.insert(offset)
            case .insert(let offset, _, _): inserted.insert(offset)
            }
        }
        var lines: [(DiffLine, changed: Bool)] = []
        var oldIndex = 0
        var newIndex = 0
        while oldIndex < before.count || newIndex < after.count {
            if oldIndex < before.count, removed.contains(oldIndex) {
                lines.append((DiffLine(.removed, before[oldIndex]), true))
                oldIndex += 1
            } else if newIndex < after.count, inserted.contains(newIndex) {
                lines.append((DiffLine(.added, after[newIndex]), true))
                newIndex += 1
            } else {
                if newIndex < after.count { lines.append((DiffLine(.context, after[newIndex]), false)) }
                oldIndex += 1
                newIndex += 1
            }
        }
        var keep = [Bool](repeating: false, count: lines.count)
        for index in lines.indices where lines[index].changed {
            for near in max(0, index - context)...min(lines.count - 1, index + context) { keep[near] = true }
        }
        return lines.indices.filter { keep[$0] }.map { lines[$0].0 }
    }
}
