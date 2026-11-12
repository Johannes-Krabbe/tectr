import Foundation
import TectrCore
import TectrDesign
import Testing
@testable import Tectr

/// Session events as the daemon sends them, one JSON object each, a second apart.
private func events(_ kinds: [String]) throws -> [Wire.SessionEvent] {
    let events = kinds.enumerated().map { index, kind in
        #"{"session":{"machine_id":"m","session_id":"s"},"seq":\#(index + 1),"at_ms":\#(index * 1000),"kind":\#(kind)}"#
    }
    return try JSONDecoder().decode([Wire.SessionEvent].self, from: Data("[\(events.joined(separator: ","))]".utf8))
}

private func fold(_ kinds: [String], status: SessionStatus = .done) throws -> [TranscriptItem] {
    var fold = TranscriptFold()
    fold.append(try events(kinds))
    return fold.items(status: status)
}

@Test func foldingAsEventsComeMatchesFoldingThemAllAtOnce() throws {
    let kinds = [
        #"{"event":"user_prompt","text":"Fix it"}"#,
        #"{"event":"agent_thought","text":"Look"}"#,
        #"{"event":"agent_thought","text":"ing"}"#,
        #"{"event":"tool_call","id":"t1","tool":{"title":"cargo test","kind":"execute","status":"in_progress"}}"#,
        #"{"event":"permission_requested","approval":{"id":"r","session_id":"s","tool_call":{"title":"rm"},"options":[{"id":"y","name":"Allow","kind":"allow_once"}]}}"#,
        #"{"event":"permission_resolved","approval_id":"r","outcome":{"outcome":"selected","option_id":"y"}}"#,
        #"{"event":"agent_message","text":"Half"}"#,
        #"{"event":"tool_call_update","id":"t1","tool":{"status":"failed"}}"#,
        #"{"event":"agent_message","text":" way"}"#,
        #"{"event":"turn_ended","stop_reason":"end_turn","last_message":"msg-1"}"#,
        #"{"event":"user_prompt","text":"Again"}"#,
        #"{"event":"tool_call","id":"t2","tool":{"title":"Edit lib.rs","kind":"edit","status":"in_progress"}}"#,
        #"{"event":"tool_call_update","id":"t2","tool":{"status":"completed","content":[{"type":"diff","path":"lib.rs","old_text":"a","new_text":"b"}]}}"#,
        #"{"event":"agent_message","text":"Done"}"#,
    ]
    let all = try events(kinds)
    var incremental = TranscriptFold()
    for (index, event) in all.enumerated() {
        incremental.append([event])
        for status in [SessionStatus.working, .waiting, .done] {
            var whole = TranscriptFold()
            whole.append(Array(all[...index]))
            let shown = incremental.items(status: status)
            let expected = whole.items(status: status)
            #expect(shown == expected, "after event \(index + 1), \(status)")
        }
    }
}

@Test func followingStopsWhenThePersonScrollsAndResumesAtTheEnd() {
    var follow = Follow()
    #expect(follow.isOn)

    // A drag takes over straight away, even before it has left the end.
    follow.dragStarted()
    #expect(!follow.isOn)
    var catchUp = follow.cameToRest(atEnd: false)
    #expect(!catchUp && !follow.isOn)

    // Content arriving while it's away doesn't bring it back.
    catchUp = follow.cameToRest(atEnd: false)
    #expect(!catchUp && !follow.isOn)

    // Coming to rest at the end by hand follows again, with nothing to make up.
    follow.dragStarted()
    catchUp = follow.cameToRest(atEnd: true)
    #expect(!catchUp && follow.isOn)

    // A jump that lands short is finished off.
    follow.resume()
    catchUp = follow.cameToRest(atEnd: false)
    #expect(catchUp)

    // Unfolding something above the end stops following, at the end it doesn't.
    follow.unfolded(last: true)
    #expect(follow.isOn)
    follow.unfolded(last: false)
    #expect(!follow.isOn)
}

@Test func streamedChunksAreOneMessageAndWorkBetweenIsOneActivity() throws {
    let items = try fold([
        #"{"event":"user_prompt","text":"Fix it"}"#,
        #"{"event":"agent_thought","text":"Look"}"#,
        #"{"event":"agent_thought","text":"ing"}"#,
        #"{"event":"tool_call","id":"t1","tool":{"title":"cargo test","kind":"execute","status":"in_progress"}}"#,
        #"{"event":"tool_call_update","id":"t1","tool":{"status":"failed"}}"#,
        #"{"event":"agent_message","text":"Done"}"#,
        #"{"event":"agent_message","text":" now"}"#,
    ])
    #expect(items.count == 3)
    guard case .activity(let activity) = items[1], case .message(_, let blocks, _) = items[2] else {
        Issue.record("expected an activity, then a message")
        return
    }
    #expect(activity.state == .finished("Worked for 3s, 2 steps", failed: 1))
    #expect(activity.steps.map(\.title) == ["Looking", "cargo test"])
    #expect(blocks == [.paragraph("Done now")])
}

@Test func aRunningTurnShowsTheStepItIsOn() throws {
    let items = try fold([
        #"{"event":"user_prompt","text":"Go"}"#,
        #"{"event":"tool_call","id":"a","tool":{"title":"Read lib.rs","status":"completed"}}"#,
        #"{"event":"tool_call","id":"b","tool":{"title":"Edit lib.rs","status":"in_progress"}}"#,
    ], status: .working)
    guard case .activity(let activity) = items.last else {
        Issue.record("expected the turn to end in an activity")
        return
    }
    #expect(activity.state == .running("Edit lib.rs"))
}

@Test func aTurnThatHasDoneNothingYetStillShowsItIsUnderWay() throws {
    let items = try fold([#"{"event":"user_prompt","text":"Go"}"#], status: .working)
    guard case .activity(let activity) = items.last else {
        Issue.record("expected a live line")
        return
    }
    #expect(activity.state == .running("Working"))
}

@Test func anAnswerTheAgentEndedCanBeForkedAndAnInterruptionIsSaid() throws {
    let items = try fold([
        #"{"event":"user_prompt","text":"Go"}"#,
        #"{"event":"agent_message","text":"Half"}"#,
        #"{"event":"turn_ended","stop_reason":"cancelled","last_message":"msg-1"}"#,
    ])
    guard case .message(_, _, let forkAfter) = items[1], case .note(_, let text, let failed) = items[2] else {
        Issue.record("expected a message, then a note")
        return
    }
    #expect(forkAfter == 3)
    #expect(text == "Interrupted")
    #expect(!failed)
}

@Test func timeWaitingOnYouIsntTimeWorked() throws {
    let items = try fold([
        #"{"event":"user_prompt","text":"Go"}"#,
        #"{"event":"tool_call","id":"t","tool":{"title":"rm","status":"pending"}}"#,
        #"{"event":"permission_requested","approval":{"id":"r","session_id":"s","tool_call":{"title":"rm"},"options":[{"id":"y","name":"Allow","kind":"allow_once"}]}}"#,
        #"{"event":"tool_call_update","id":"t","tool":{"status":"pending"}}"#,
        #"{"event":"tool_call_update","id":"t","tool":{"status":"pending"}}"#,
        #"{"event":"permission_resolved","approval_id":"r","outcome":{"outcome":"selected","option_id":"y"}}"#,
        #"{"event":"tool_call_update","id":"t","tool":{"status":"completed"}}"#,
    ])
    guard case .activity(let activity) = items[1] else {
        Issue.record("expected an activity")
        return
    }
    // Five seconds in all, three of them waiting for an answer.
    #expect(activity.state == .finished("Worked for 3s, 1 step", failed: 0))
    #expect(activity.steps.last?.title == "Request: rm. Allow")
}

@Test func markdownBecomesBlocks() {
    let text = "# Title\nSome *text*\ncontinued\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n\n```\nstill streaming"
    #expect(MessageBlock.parse(text) == [
        .heading("Title"),
        .paragraph("Some *text*\ncontinued"),
        .bullets(["one", "two"]),
        .code(language: "rust", text: "fn main() {}"),
        .code(language: nil, text: "still streaming"),
    ])
}

@Test func aDiffKeepsOnlyTheLinesNearAChange() {
    let old = (1...20).map(String.init).joined(separator: "\n")
    let new = old.replacingOccurrences(of: "\n10\n", with: "\nten\n")
    let lines = DiffLine.between(old, new, context: 1)
    #expect(lines == [
        DiffLine(.context, "9"), DiffLine(.removed, "10"), DiffLine(.added, "ten"), DiffLine(.context, "11"),
    ])
}

@Test func anImageGoesWithItsTokenInTheText() {
    var attachments = Attachments()
    var text = "Look"
    let image = PromptImage(id: "a", number: attachments.nextNumber(sent: 2), mime: "image/png", width: 1, height: 1)
    attachments.add(image, to: &text)
    #expect(image.number == 3)
    #expect(text == "Look [Image #3] ")
    #expect(attachments.named(in: text) == [image])
    #expect(attachments.named(in: "Look") == [])
    attachments.remove(image, from: &text)
    #expect(text == "Look ")
    #expect(attachments.images.isEmpty)
}
