//! Checked-in wire examples. If one of these fails, the change would break an
//! already shipped client or daemon: add a new protocol version instead of
//! editing the fixture.

use protocol::{ClientMessage, ServerMessage};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

fn check<T: DeserializeOwned + Serialize>(fixture: &str) {
    for line in fixture.lines().filter(|line| !line.trim().is_empty()) {
        let expected: Value = serde_json::from_str(line).unwrap();
        let parsed: T = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("failed to parse {line}: {error}"));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), expected);
    }
}

#[test]
fn client_messages_keep_their_wire_format() {
    check::<ClientMessage>(include_str!("fixtures/client_messages.jsonl"));
}

#[test]
fn server_messages_keep_their_wire_format() {
    check::<ServerMessage>(include_str!("fixtures/server_messages.jsonl"));
}
