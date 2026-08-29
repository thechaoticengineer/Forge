use serde::{Deserialize, Serialize};

use crate::state::{AgentKind, EngineSnapshot};

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
    CreateDraftRun {
        repository: String,
        goal: String,
    },
    GeneratePlan {
        run_id: String,
        agent: AgentKind,
    },
    UpdatePlanTask {
        run_id: String,
        plan_id: String,
        task_id: String,
        title: String,
        description: String,
        acceptance_criteria: Vec<String>,
    },
    MovePlanTask {
        run_id: String,
        plan_id: String,
        task_id: String,
        direction: MoveDirection,
    },
    ApprovePlan {
        run_id: String,
        plan_id: String,
    },
    RejectPlan {
        run_id: String,
        plan_id: String,
        reason: Option<String>,
    },
    GetSnapshot,
    Ping,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveDirection {
    Up,
    Down,
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

    #[test]
    fn parses_create_draft_request() {
        let message: ClientMessage = serde_json::from_str(
            r#"{"version":1,"request_id":"request-2","method":"create_draft_run","repository":"/tmp/project","goal":"Add a test"}"#,
        )
        .expect("request should parse");

        assert_eq!(
            message.request,
            ClientRequest::CreateDraftRun {
                repository: "/tmp/project".to_owned(),
                goal: "Add a test".to_owned(),
            }
        );
    }

    #[test]
    fn parses_plan_workflow_requests() {
        let generate: ClientMessage = serde_json::from_str(
            r#"{"version":1,"request_id":"request-3","method":"generate_plan","run_id":"run-1","agent":"claude"}"#,
        )
        .expect("generate request should parse");
        assert_eq!(
            generate.request,
            ClientRequest::GeneratePlan {
                run_id: "run-1".to_owned(),
                agent: AgentKind::Claude,
            }
        );

        let move_task: ClientMessage = serde_json::from_str(
            r#"{"version":1,"request_id":"request-4","method":"move_plan_task","run_id":"run-1","plan_id":"plan-1","task_id":"task-2","direction":"up"}"#,
        )
        .expect("move request should parse");
        assert_eq!(
            move_task.request,
            ClientRequest::MovePlanTask {
                run_id: "run-1".to_owned(),
                plan_id: "plan-1".to_owned(),
                task_id: "task-2".to_owned(),
                direction: MoveDirection::Up,
            }
        );
    }
}
