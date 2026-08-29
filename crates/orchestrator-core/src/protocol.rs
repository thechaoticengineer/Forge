use serde::{Deserialize, Serialize};

use crate::state::EngineSnapshot;

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientMessage {
    pub version: u16,
    pub request_id: String,
    #[serde(flatten)]
    pub request: ClientRequest,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum ClientRequest {
    GetSnapshot,
    Ping,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot {
        version: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        snapshot: EngineSnapshot,
    },
    Pong {
        version: u16,
        request_id: String,
    },
    Error {
        version: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        code: String,
        message: String,
    },
}

impl ServerMessage {
    #[must_use]
    pub fn snapshot(snapshot: EngineSnapshot, request_id: Option<String>) -> Self {
        Self::Snapshot {
            version: PROTOCOL_VERSION,
            request_id,
            snapshot,
        }
    }

    #[must_use]
    pub fn error(
        request_id: Option<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::Error {
            version: PROTOCOL_VERSION,
            request_id,
            code: code.into(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_snapshot_as_versioned_json_envelope() {
        let message = ServerMessage::snapshot(EngineSnapshot::default(), None);
        let json = serde_json::to_value(message).expect("snapshot should serialize");

        assert_eq!(json["version"], PROTOCOL_VERSION);
        assert_eq!(json["type"], "snapshot");
        assert_eq!(json["snapshot"]["status"], "idle");
        assert!(json.get("request_id").is_none());
    }

    #[test]
    fn parses_snapshot_request() {
        let message: ClientMessage = serde_json::from_str(
            r#"{"version":1,"request_id":"request-1","method":"get_snapshot"}"#,
        )
        .expect("request should parse");

        assert_eq!(message.version, PROTOCOL_VERSION);
        assert_eq!(message.request_id, "request-1");
        assert_eq!(message.request, ClientRequest::GetSnapshot);
    }
}
