//! Project-defined deterministic verification commands and policy.
//!
//! A project declares its own checks in a committed JSON file instead of
//! letting the engine guess a toolchain or letting an agent decide what
//! counts as success. The file is read exactly as it was committed at the
//! task worktree's base revision, so an implementing agent cannot widen,
//! weaken, or delete its own quality gates from inside the worktree.
//!
//! Projects without that file keep the previous detected-toolchain behavior.

use std::{path::Path, time::Duration};

use orchestrator_git::committed_file;
use serde::Deserialize;

/// Repository-relative path of the committed verification policy.
pub const CONFIG_PATH: &str = ".orchestrator/verification.json";

const SUPPORTED_SCHEMA_VERSION: u32 = 1;
const MAX_COMMANDS: usize = 32;
const MAX_LABEL_CHARS: usize = 80;
const MAX_ARGUMENTS: usize = 64;
const DEFAULT_TIMEOUT_SECONDS: u64 = 900;
const MAX_TIMEOUT_SECONDS: u64 = 3600;

/// One deterministic check run inside a task worktree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationCommand {
    pub label: String,
    pub program: String,
    pub arguments: Vec<String>,
    /// Worktree-relative directory the command runs in.
    pub working_directory: String,
    pub timeout: Duration,
    /// Whether failing this command fails the whole verification attempt.
    pub required: bool,
}

impl VerificationCommand {
    fn detected(label: &str, program: &str, arguments: &[&str]) -> Self {
        Self {
            label: label.to_owned(),
            program: program.to_owned(),
            arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
            working_directory: ".".to_owned(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
            required: true,
        }
    }
}

/// Where a resolved set of checks came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerificationSource {
    /// Declared by the project in its committed policy file.
    Project,
    /// Inferred by the engine from well-known project markers.
    Detected,
}

impl VerificationSource {
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Project => "project-defined verification policy",
            Self::Detected => "detected project verification commands",
        }
    }
}

/// Resolves the checks for a task worktree.
///
/// Returns the project's committed policy when it exists, otherwise the
/// detected commands. An unreadable or invalid policy is an error rather than
/// a silent fall back to detection: a project that declares its gates must
/// never be verified by a weaker set of checks it did not ask for.
pub fn resolve_verification_commands(
    worktree: &Path,
    base_revision: &str,
) -> Result<(Vec<VerificationCommand>, VerificationSource), String> {
    let declared = committed_file(worktree, base_revision, CONFIG_PATH)
        .map_err(|error| format!("cannot read {CONFIG_PATH} at {base_revision}: {error}"))?;
    match declared {
        Some(contents) => parse_verification_config(&contents)
            .map(|commands| (commands, VerificationSource::Project)),
        None => Ok((
            detected_verification_commands(worktree),
            VerificationSource::Detected,
        )),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct VerificationConfigFile {
    schema_version: u32,
    commands: Vec<VerificationCommandConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct VerificationCommandConfig {
    label: String,
    program: String,
    #[serde(default)]
    arguments: Vec<String>,
    working_directory: Option<String>,
    timeout_seconds: Option<u64>,
    required: Option<bool>,
}

/// Validates one committed policy document into runnable checks.
fn parse_verification_config(contents: &str) -> Result<Vec<VerificationCommand>, String> {
    let file: VerificationConfigFile =
        serde_json::from_str(contents).map_err(|error| invalid(&error.to_string()))?;
    if file.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(invalid(&format!(
            "schemaVersion {} is not supported; this engine understands {SUPPORTED_SCHEMA_VERSION}",
            file.schema_version
        )));
    }
    if file.commands.is_empty() {
        return Err(invalid(
            "commands must declare at least one deterministic check",
        ));
    }
    if file.commands.len() > MAX_COMMANDS {
        return Err(invalid(&format!(
            "commands must declare at most {MAX_COMMANDS} checks"
        )));
    }
    let mut commands = Vec::with_capacity(file.commands.len());
    let mut labels = Vec::with_capacity(file.commands.len());
    for (index, declared) in file.commands.into_iter().enumerate() {
        let command = validate_command(declared, index)?;
        if labels.contains(&command.label) {
            return Err(invalid(&format!(
                "commands[{index}] repeats the label {:?}; labels identify evidence and must be unique",
                command.label
            )));
        }
        labels.push(command.label.clone());
        commands.push(command);
    }
    if !commands.iter().any(|command| command.required) {
        return Err(invalid(
            "at least one check must be required; an entirely advisory policy would pass every task",
        ));
    }
    Ok(commands)
}

fn validate_command(
    declared: VerificationCommandConfig,
    index: usize,
) -> Result<VerificationCommand, String> {
    let label = declared.label.trim().to_owned();
    if label.is_empty() {
        return Err(field(index, "label", "must not be empty"));
    }
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(field(
            index,
            "label",
            &format!("must be at most {MAX_LABEL_CHARS} characters"),
        ));
    }
    let program = declared.program.trim().to_owned();
    if program.is_empty() {
        return Err(field(index, "program", "must not be empty"));
    }
    if program.starts_with('-') || program.contains('\0') {
        return Err(field(
            index,
            "program",
            "must be a program name or path, not an option",
        ));
    }
    if !program.starts_with('/') && has_parent_segment(&program) {
        return Err(field(
            index,
            "program",
            "must not escape the worktree with ..",
        ));
    }
    if declared.arguments.len() > MAX_ARGUMENTS {
        return Err(field(
            index,
            "arguments",
            &format!("must contain at most {MAX_ARGUMENTS} values"),
        ));
    }
    if declared.arguments.iter().any(|value| value.contains('\0')) {
        return Err(field(index, "arguments", "must not contain NUL bytes"));
    }
    let working_directory = match declared.working_directory {
        None => ".".to_owned(),
        Some(value) => {
            let value = value.trim().to_owned();
            if value.is_empty() {
                return Err(field(index, "workingDirectory", "must not be empty"));
            }
            if value.starts_with('/') || has_parent_segment(&value) {
                return Err(field(
                    index,
                    "workingDirectory",
                    "must be a relative path inside the worktree",
                ));
            }
            value
        }
    };
    let timeout_seconds = declared.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    if timeout_seconds == 0 || timeout_seconds > MAX_TIMEOUT_SECONDS {
        return Err(field(
            index,
            "timeoutSeconds",
            &format!("must be between 1 and {MAX_TIMEOUT_SECONDS}"),
        ));
    }
    Ok(VerificationCommand {
        label,
        program,
        arguments: declared.arguments,
        working_directory,
        timeout: Duration::from_secs(timeout_seconds),
        required: declared.required.unwrap_or(true),
    })
}

fn has_parent_segment(value: &str) -> bool {
    value.split('/').any(|segment| segment == "..")
}

fn invalid(reason: &str) -> String {
    format!("{CONFIG_PATH} is invalid: {reason}")
}

fn field(index: usize, name: &str, reason: &str) -> String {
    invalid(&format!("commands[{index}].{name} {reason}"))
}

/// Infers checks for projects that have not declared their own policy.
fn detected_verification_commands(worktree: &Path) -> Vec<VerificationCommand> {
    let mut commands = Vec::new();
    if worktree.join("Cargo.toml").is_file() {
        commands.extend([
            VerificationCommand::detected(
                "Rust format",
                "cargo",
                &["fmt", "--all", "--", "--check"],
            ),
            VerificationCommand::detected("Rust tests", "cargo", &["test", "--workspace"]),
            VerificationCommand::detected(
                "Rust lint",
                "cargo",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
        ]);
    }
    if worktree.join("manifest.json").is_file() {
        commands.push(VerificationCommand::detected(
            "Omarchy plugin",
            "omarchy",
            &["plugin", "validate", "."],
        ));
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs, process::Command};

    use tempfile::TempDir;

    #[test]
    fn detects_fixed_rust_and_omarchy_verification_commands() {
        let directory = TempDir::new().expect("temporary directory should exist");
        fs::write(directory.path().join("Cargo.toml"), "[workspace]\n")
            .expect("Cargo marker should be written");
        fs::write(directory.path().join("manifest.json"), "{}")
            .expect("plugin marker should be written");

        let commands = detected_verification_commands(directory.path());

        assert_eq!(commands.len(), 4);
        assert_eq!(commands[0].arguments, ["fmt", "--all", "--", "--check"]);
        assert_eq!(commands[3].arguments, ["plugin", "validate", "."]);
        assert!(commands.iter().all(|command| command.required));
        assert!(
            commands
                .iter()
                .all(|command| command.timeout == Duration::from_secs(DEFAULT_TIMEOUT_SECONDS))
        );
    }

    #[test]
    fn parses_declared_commands_and_policy() {
        let commands = parse_verification_config(
            r#"{
              "schemaVersion": 1,
              "commands": [
                {
                  "label": "Unit tests",
                  "program": "cargo",
                  "arguments": ["test", "--workspace"],
                  "timeoutSeconds": 120
                },
                {
                  "label": "Docs",
                  "program": "./scripts/docs.sh",
                  "workingDirectory": "tools",
                  "required": false
                }
              ]
            }"#,
        )
        .expect("valid policy should parse");

        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].working_directory, ".");
        assert_eq!(commands[0].timeout, Duration::from_secs(120));
        assert!(commands[0].required);
        assert_eq!(commands[1].working_directory, "tools");
        assert_eq!(commands[1].timeout, Duration::from_mins(15));
        assert!(!commands[1].required);
    }

    #[test]
    fn rejects_unusable_policies_with_actionable_messages() {
        let cases = [
            ("{", "EOF while parsing"),
            (r#"{"schemaVersion": 2, "commands": []}"#, "schemaVersion 2"),
            (r#"{"schemaVersion": 1, "commands": []}"#, "at least one"),
            (
                r#"{"schemaVersion": 1, "commands": [{"label": "a", "program": "b", "extra": 1}]}"#,
                "unknown field",
            ),
            (
                r#"{"schemaVersion": 1, "commands": [{"label": "  ", "program": "cargo"}]}"#,
                "commands[0].label must not be empty",
            ),
            (
                r#"{"schemaVersion": 1, "commands": [{"label": "a", "program": "--version"}]}"#,
                "commands[0].program must be a program name",
            ),
            (
                r#"{"schemaVersion": 1, "commands": [{"label": "a", "program": "../tool"}]}"#,
                "commands[0].program must not escape",
            ),
            (
                r#"{"schemaVersion": 1, "commands": [{"label": "a", "program": "b", "workingDirectory": "../outside"}]}"#,
                "commands[0].workingDirectory must be a relative path",
            ),
            (
                r#"{"schemaVersion": 1, "commands": [{"label": "a", "program": "b", "timeoutSeconds": 0}]}"#,
                "commands[0].timeoutSeconds must be between",
            ),
            (
                r#"{"schemaVersion": 1, "commands": [{"label": "a", "program": "b"}, {"label": "a", "program": "c"}]}"#,
                "repeats the label",
            ),
            (
                r#"{"schemaVersion": 1, "commands": [{"label": "a", "program": "b", "required": false}]}"#,
                "at least one check must be required",
            ),
        ];

        for (contents, expected) in cases {
            let error = parse_verification_config(contents)
                .expect_err("unusable policy should be rejected");
            assert!(
                error.contains(CONFIG_PATH) && error.contains(expected),
                "{error} should mention {CONFIG_PATH} and {expected}"
            );
        }
    }

    #[test]
    fn rejects_more_commands_than_the_supported_maximum() {
        let declared = (0..=MAX_COMMANDS)
            .map(|index| format!(r#"{{"label": "check {index}", "program": "true"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let error = parse_verification_config(&format!(
            r#"{{"schemaVersion": 1, "commands": [{declared}]}}"#
        ))
        .expect_err("an oversized policy should be rejected");

        assert!(error.contains("at most 32 checks"), "{error}");
    }

    #[test]
    fn prefers_the_committed_policy_over_worktree_edits() {
        let repository = repository_with_policy(
            r#"{"schemaVersion": 1, "commands": [{"label": "Committed", "program": "true"}]}"#,
        );
        let head = head_revision(repository.path());
        fs::write(
            repository.path().join(CONFIG_PATH),
            r#"{"schemaVersion": 1, "commands": [{"label": "Weakened", "program": "true"}]}"#,
        )
        .expect("worktree edit should be written");

        let (commands, source) = resolve_verification_commands(repository.path(), &head)
            .expect("committed policy should resolve");

        assert_eq!(source, VerificationSource::Project);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].label, "Committed");
    }

    #[test]
    fn falls_back_to_detection_without_a_committed_policy() {
        let repository = initialized_repository();
        fs::write(repository.path().join("Cargo.toml"), "[workspace]\n")
            .expect("Cargo marker should be written");
        let head = head_revision(repository.path());

        let (commands, source) = resolve_verification_commands(repository.path(), &head)
            .expect("detection should resolve");

        assert_eq!(source, VerificationSource::Detected);
        assert_eq!(commands.len(), 3);
    }

    #[test]
    fn refuses_an_invalid_policy_instead_of_falling_back_to_detection() {
        let repository = repository_with_policy(r#"{"schemaVersion": 4, "commands": []}"#);
        fs::write(repository.path().join("Cargo.toml"), "[workspace]\n")
            .expect("Cargo marker should be written");
        let head = head_revision(repository.path());

        let error = resolve_verification_commands(repository.path(), &head)
            .expect_err("an invalid policy should not silently detect commands");

        assert!(error.contains("schemaVersion 4"), "{error}");
    }

    fn repository_with_policy(policy: &str) -> TempDir {
        let repository = initialized_repository();
        let config = repository.path().join(CONFIG_PATH);
        fs::create_dir_all(config.parent().expect("policy should have a parent"))
            .expect("policy directory should be created");
        fs::write(&config, policy).expect("policy should be written");
        git(repository.path(), &["add", CONFIG_PATH]);
        commit(repository.path(), "test: declare verification");
        repository
    }

    fn initialized_repository() -> TempDir {
        let repository = TempDir::new().expect("temporary directory should exist");
        git(repository.path(), &["init", "--quiet"]);
        fs::write(repository.path().join("README.md"), "test")
            .expect("tracked file should be created");
        git(repository.path(), &["add", "README.md"]);
        commit(repository.path(), "test: initialize");
        repository
    }

    fn head_revision(repository: &Path) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("Git should start");
        String::from_utf8(output.stdout)
            .expect("revision should be UTF-8")
            .trim()
            .to_owned()
    }

    fn commit(repository: &Path, message: &str) {
        git(
            repository,
            &[
                "-c",
                "user.name=Test User",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                message,
            ],
        );
    }

    fn git(repository: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .env("LC_ALL", "C")
            .output()
            .expect("Git should start");
        assert!(
            output.status.success(),
            "Git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
