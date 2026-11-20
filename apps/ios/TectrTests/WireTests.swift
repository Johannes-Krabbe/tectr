import Foundation
import TectrCore
import Testing

// Lines from crates/protocol/tests/fixtures, so the app reads and writes what the daemon does.

private func json(_ command: Command) throws -> [String: JSONValue] {
    let object = try JSONDecoder().decode(JSONValue.self, from: JSONEncoder().encode(command))
    guard case .object(let fields) = object else { throw CocoaError(.coderReadCorrupt) }
    return fields
}

@Test func workspaceCommandsMatchTheProtocol() throws {
    #expect(try json(.closeWorkspace("w1", force: true)) == [
        "command": .string("close_workspace"), "workspace_id": .string("w1"), "force": .bool(true),
    ])
    #expect(try json(.openWorkspace("w1")) == ["command": .string("open_workspace"), "workspace_id": .string("w1")])
    #expect(try json(.listClosedWorkspaces) == ["command": .string("list_closed_workspaces")])
    #expect(try json(.removeWorkspace("w1", force: true)) == [
        "command": .string("remove_workspace"), "workspace_id": .string("w1"), "force": .bool(true),
    ])
}

@Test func closedWorkspacesAreRead() throws {
    let line = #"{"output":"closed_workspaces","workspaces":[{"id":"w1","name":"repo","path":"/repo","closed_at_ms":1700000000000,"topics":3}]}"#
    let output = try JSONDecoder().decode(Wire.CommandOutput.self, from: Data(line.utf8))
    guard case .closedWorkspaces(let workspaces) = output else {
        Issue.record("expected closed workspaces, got \(output)")
        return
    }
    #expect(workspaces.count == 1)
    #expect(workspaces[0].id == "w1")
    #expect(workspaces[0].path == "/repo")
    #expect(workspaces[0].closedAtMs == 1_700_000_000_000)
    #expect(workspaces[0].topics == 3)
}
