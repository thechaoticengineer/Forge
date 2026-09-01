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
    ListRepositories,
    CloneRepository {
        name_with_owner: String,
    },
    CompleteRepositoryPath {
        path: String,
    },
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
    CreateTaskWorktree {
        run_id: String,
        plan_id: String,
        task_id: String,
    },
    RunTaskImplementation {
        run_id: String,
        plan_id: String,
        task_id: String,
        worktree_id: String,
        agent: AgentKind,
    },
    CancelTaskImplementation {
        run_id: String,
        attempt_id: String,
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
    RepositoryCatalog {
        version: u16,
        request_id: String,
        catalog: RepositoryCatalog,
    },
    RepositoryCloned {
        version: u16,
        request_id: String,
        name_with_owner: String,
        path: String,
    },
    PathCompletion {
        version: u16,
        request_id: String,
        replacement: String,
        candidates: Vec<String>,
    },
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryCatalog {
    pub project_roots: Vec<String>,
    pub local: Vec<LocalRepository>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_error: Option<String>,
    pub github: Vec<GithubRepository>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalRepository {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_with_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub dirty: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GithubRepository {
    pub name: String,
    pub name_with_owner: String,
    pub url: String,
    pub archived: bool,
    pub fork: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pushed_at: Option<String>,
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
    fn defaults_implementation_fields_from_an_older_version_one_snapshot() {
        let run = serde_json::json!({
            "id": "run-1",
            "goal": "Implement safely",
            "repository": "/tmp/project",
            "base_revision": "0123456789012345678901234567890123456789",
            "branch": "main",
            "worktree_dirty": false,
            "run_status": "waiting_for_user",
            "plan": null,
            "worktrees": [],
            "last_error": null
        });
        let snapshot: EngineSnapshot = serde_json::from_value(serde_json::json!({
            "sequence": 1,
            "status": "idle",
            "active_run": run,
            "requires_attention": false
        }))
        .expect("an older version-one snapshot should remain readable");

        let run = snapshot.active_run.expect("run should decode");
        assert!(run.implementation_attempts.is_empty());
        assert!(run.implementation_activity.is_empty());
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
    fn parses_repository_path_completion_request() {
        let message: ClientMessage = serde_json::from_str(
            r#"{"version":1,"request_id":"request-path","method":"complete_repository_path","path":"/home/dev/Pro"}"#,
        )
        .expect("path completion request should parse");

        assert_eq!(
            message.request,
            ClientRequest::CompleteRepositoryPath {
                path: "/home/dev/Pro".to_owned(),
            }
        );
    }

    #[test]
    fn parses_repository_catalog_and_clone_requests() {
        let list: ClientMessage = serde_json::from_str(
            r#"{"version":1,"request_id":"request-list","method":"list_repositories"}"#,
        )
        .expect("catalog request should parse");
        assert_eq!(list.request, ClientRequest::ListRepositories);

        let clone: ClientMessage = serde_json::from_str(
            r#"{"version":1,"request_id":"request-clone","method":"clone_repository","name_with_owner":"owner/project"}"#,
        )
        .expect("clone request should parse");
        assert_eq!(
            clone.request,
            ClientRequest::CloneRepository {
                name_with_owner: "owner/project".to_owned()
            }
        );
    }

    #[test]
    fn serializes_repository_catalog_response() {
        let message = ServerMessage::RepositoryCatalog {
            version: PROTOCOL_VERSION,
            request_id: "request-repositories".to_owned(),
            catalog: RepositoryCatalog {
                project_roots: vec!["/home/dev/Projects".to_owned()],
                local: vec![LocalRepository {
                    name: "Forge".to_owned(),
                    path: "/home/dev/Projects/Forge".to_owned(),
                    name_with_owner: Some("developer/Forge".to_owned()),
                    branch: Some("main".to_owned()),
                    dirty: false,
                }],
                local_error: None,
                github: vec![GithubRepository {
                    name: "remote-only".to_owned(),
                    name_with_owner: "developer/remote-only".to_owned(),
                    url: "https://github.com/developer/remote-only".to_owned(),
                    archived: false,
                    fork: false,
                    pushed_at: None,
                }],
                github_error: None,
            },
        };
        let json = serde_json::to_value(message).expect("catalog should serialize");

        assert_eq!(json["type"], "repository_catalog");
        assert_eq!(json["catalog"]["local"][0]["name"], "Forge");
        assert!(json["catalog"].get("local_error").is_none());
        assert_eq!(
            json["catalog"]["github"][0]["name_with_owner"],
            "developer/remote-only"
        );
        assert!(json["catalog"].get("github_error").is_none());
    }

    #[test]
    fn serializes_repository_clone_response() {
        let message = ServerMessage::RepositoryCloned {
            version: PROTOCOL_VERSION,
            request_id: "request-clone".to_owned(),
            name_with_owner: "owner/project".to_owned(),
            path: "/home/dev/Projects/project".to_owned(),
        };

        let json = serde_json::to_value(message).expect("clone response should serialize");

        assert_eq!(json["type"], "repository_cloned");
        assert_eq!(json["name_with_owner"], "owner/project");
        assert_eq!(json["path"], "/home/dev/Projects/project");
    }

    #[test]
    fn serializes_repository_path_completion_response() {
        let message = ServerMessage::PathCompletion {
            version: PROTOCOL_VERSION,
            request_id: "request-path".to_owned(),
            replacement: "/home/dev/Projects/".to_owned(),
            candidates: vec!["/home/dev/Projects/".to_owned()],
        };
        let json = serde_json::to_value(message).expect("completion should serialize");

        assert_eq!(json["type"], "path_completion");
        assert_eq!(json["replacement"], "/home/dev/Projects/");
        assert_eq!(json["candidates"][0], "/home/dev/Projects/");
    }

    #[test]
    fn parses_create_task_worktree_request() {
        let message: ClientMessage = serde_json::from_str(
            r#"{"version":1,"request_id":"request-worktree","method":"create_task_worktree","run_id":"run-1","plan_id":"plan-1","task_id":"task-1"}"#,
        )
        .expect("worktree request should parse");

        assert_eq!(
            message.request,
            ClientRequest::CreateTaskWorktree {
                run_id: "run-1".to_owned(),
                plan_id: "plan-1".to_owned(),
                task_id: "task-1".to_owned(),
            }
        );
    }

    #[test]
    fn parses_run_task_implementation_request() {
        let message: ClientMessage = serde_json::from_str(
            r#"{"version":1,"request_id":"request-implement","method":"run_task_implementation","run_id":"run-1","plan_id":"plan-1","task_id":"task-1","worktree_id":"worktree-1","agent":"claude"}"#,
        )
        .expect("implementation request should parse");

        assert_eq!(
            message.request,
            ClientRequest::RunTaskImplementation {
                run_id: "run-1".to_owned(),
                plan_id: "plan-1".to_owned(),
                task_id: "task-1".to_owned(),
                worktree_id: "worktree-1".to_owned(),
                agent: AgentKind::Claude,
            }
        );
    }

    #[test]
    fn parses_cancel_task_implementation_request() {
        let message: ClientMessage = serde_json::from_str(
            r#"{"version":1,"request_id":"request-cancel","method":"cancel_task_implementation","run_id":"run-1","attempt_id":"attempt-1"}"#,
        )
        .expect("cancellation request should parse");

        assert_eq!(
            message.request,
            ClientRequest::CancelTaskImplementation {
                run_id: "run-1".to_owned(),
                attempt_id: "attempt-1".to_owned(),
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
