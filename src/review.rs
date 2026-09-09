//! Engine-owned scope policy and snapshot-bound, immutable review gates.
use super::Ctx;
use crate::agent::{AgentRequest, AgentResult, AgentUsage};
use crate::prompts::{FIX_PROMPT, IMPLEMENT_PROMPT, REVIEW_PROMPT};
use crate::util::unix_timestamp;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn git_bytes(root: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(out.stdout)
}
fn digest(bytes: &[u8]) -> Result<String, String> {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(bytes)
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("snapshot hashing failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .ok_or("missing digest")?
        .into())
}
fn paths(bytes: &[u8]) -> Result<Vec<String>, String> {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            String::from_utf8(p.to_vec()).map_err(|_| "non-UTF8 path requires manual review".into())
        })
        .collect()
}
fn implementation(path: &str) -> bool {
    path != ".forge" && !path.starts_with(".forge/")
}

/// A raw layout identity and a normalized final tree are both necessary: git add
/// changes the former legitimately, but must never change reviewed content.
fn snapshot(root: &str) -> Result<Value, String> {
    let head = String::from_utf8(git_bytes(root, &["rev-parse", "HEAD"])?)
        .map_err(|e| e.to_string())?
        .trim()
        .to_string();
    let names = paths(&git_bytes(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?)?;
    let mut names: std::collections::BTreeSet<_> = names
        .into_iter()
        .chain(paths(&git_bytes(
            root,
            &["ls-tree", "-r", "--name-only", "-z", "HEAD"],
        )?)?)
        .filter(|p| implementation(p))
        .collect();
    let mut content = Vec::new();
    for path in std::mem::take(&mut names) {
        let full = PathBuf::from(root).join(&path);
        let (mode, bytes) = match fs::symlink_metadata(&full) {
            Ok(m) if m.is_symlink() => (
                0o120000,
                fs::read_link(&full)
                    .map_err(|e| e.to_string())?
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            ),
            Ok(m) if m.is_file() => (
                if m.permissions().mode() & 0o111 != 0 {
                    0o100755
                } else {
                    0o100644
                },
                fs::read(&full).map_err(|e| e.to_string())?,
            ),
            Ok(_) => {
                return Err(format!(
                    "unsupported directory/submodule in snapshot: {path}"
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (0, vec![]),
            Err(e) => return Err(e.to_string()),
        };
        let hash = digest(&bytes)?;
        content.extend_from_slice(format!("{}:{path}:{mode}:{hash}\n", path.len()).as_bytes());
    }
    let head_ref = String::from_utf8_lossy(&git_bytes(
        root,
        &["rev-parse", "--symbolic-full-name", "HEAD"],
    )?)
    .trim()
    .to_string();
    content.extend_from_slice(head_ref.as_bytes());
    let index = git_bytes(
        root,
        &["ls-files", "--stage", "-z", "--", ".", ":(exclude).forge"],
    )?;
    let staged = git_bytes(
        root,
        &[
            "diff",
            "--cached",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "--",
            ".",
            ":(exclude).forge",
        ],
    )?;
    let unstaged = git_bytes(
        root,
        &[
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "--",
            ".",
            ":(exclude).forge",
        ],
    )?;
    let content_hash = digest(&content)?;
    let mut raw = head.as_bytes().to_vec();
    for part in [&index, &staged, &unstaged, &content] {
        raw.extend_from_slice(&(part.len() as u64).to_le_bytes());
        raw.extend_from_slice(part);
    }
    // Isolated index computes the tree git add will produce without touching the real index.
    let temp = std::env::temp_dir().join(format!(
        "forge-review-index-{}",
        crate::architecture::identity()
    ));
    let tree = (|| {
        for args in [
            vec!["read-tree", "HEAD"],
            // Naming an ignored .forge even in an exclude pathspec makes git add fail.
            // Reset runtime paths afterward, using this same isolated index.
            vec!["add", "-A"],
            vec!["reset", "-q", "HEAD", "--", ".forge"],
        ] {
            let out = Command::new("git")
                .args(args)
                .env("GIT_INDEX_FILE", &temp)
                .current_dir(root)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).into_owned());
            }
        }
        let out = Command::new("git")
            .arg("write-tree")
            .env("GIT_INDEX_FILE", &temp)
            .current_dir(root)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err("cannot compute reviewed tree".into());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    })();
    let _ = fs::remove_file(temp);
    Ok(json!({"head":head,"fingerprint":digest(&raw)?,"content":content_hash,"tree":tree?}))
}
fn dual(reason: &str) -> Value {
    json!({"version":1,"required_roles":["architect","reviewer"],"scope":"code_or_contract","rationale":reason})
}
fn classify(root: &str, stage: &Value, promoted: bool) -> Result<Value, String> {
    if promoted {
        return Ok(dual("Dual review retained for this attempt"));
    }
    let intent = format!(
        "{} {}",
        stage["title"].as_str().unwrap_or(""),
        stage["instructions"].as_str().unwrap_or("")
    )
    .to_lowercase();
    let risky = |text: &str| {
        let lower = text.to_lowercase();
        [
            "api",
            "schema",
            "interface",
            "contract",
            "architect",
            "security",
            "must",
            "shall",
            "required",
            "guarantee",
            "decision",
            "config",
            "build",
            "executable",
            "```",
            "~~~",
            "<script",
            "<!--",
            "\n    ",
            "\t",
            "`",
            "permission",
            "authentication",
            "authorization",
            "always",
            "never",
            "choose",
            "chosen",
            "=",
            "{",
            "}",
            "print(",
            "#!/",
            "import ",
            "export ",
        ]
        .iter()
        .any(|word| lower.contains(word))
    };
    if !["documentation", "prose", "spelling", "typo", "explanation"]
        .iter()
        .any(|s| intent.contains(s))
        || risky(&intent)
    {
        return Ok(dual("Stage intent is code, contractual, or uncertain"));
    }
    let mut changed = paths(&git_bytes(
        root,
        &[
            "diff",
            "HEAD",
            "--name-only",
            "-z",
            "--",
            ".",
            ":(exclude).forge",
        ],
    )?)?;
    changed.extend(paths(&git_bytes(
        root,
        &[
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--",
            ".",
            ":(exclude).forge",
        ],
    )?)?);
    let untracked = paths(&git_bytes(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)?;
    // New/deleted/renamed documents, executable bits, and unknown formats are uncertain.
    if untracked.iter().any(|p| implementation(p)) {
        return Ok(dual(
            "Untracked implementation content requires dual review",
        ));
    }
    if changed.is_empty() {
        return Ok(dual("Empty or uncertain implementation diff"));
    }
    for path in changed {
        if !(path.ends_with(".md") || path.ends_with(".txt") || path.ends_with(".rst")) {
            return Ok(dual("Mixed or non-prose file changes"));
        }
        let Ok(m) = fs::symlink_metadata(PathBuf::from(root).join(&path)) else {
            return Ok(dual("Deleted or unreadable document"));
        };
        if !m.is_file() || m.mode() & 0o111 != 0 {
            return Ok(dual("Non-ordinary document mode"));
        }
        let diff = git_bytes(
            root,
            &[
                "diff",
                "HEAD",
                "--no-ext-diff",
                "--no-textconv",
                "--unified=3",
                "--",
                &path,
            ],
        )?;
        let mut diff = diff;
        diff.extend(git_bytes(
            root,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--",
                &path,
            ],
        )?);
        diff.extend(git_bytes(
            root,
            &["diff", "--no-ext-diff", "--no-textconv", "--", &path],
        )?);
        let diff = String::from_utf8(diff).map_err(|_| "non-text diff".to_string())?;
        if diff.contains("new file mode")
            || diff.contains("old mode")
            || diff.contains("deleted file")
            || risky(&diff)
        {
            return Ok(dual(
                "Full diff contains executable, normative, architectural, or uncertain content",
            ));
        }
    }
    Ok(
        json!({"version":1,"required_roles":["reviewer"],"scope":"ordinary_documentation","rationale":"Prose intent and full diff contain only existing non-executable documents without detected contractual or normative content; independent scope verification required"}),
    )
}
struct UniqueJson(Value);
impl<'de> serde::Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson(json!(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(json!(v)))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(json!(v)))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(json!(v)))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson(json!(v)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut values = vec![];
                while let Some(UniqueJson(v)) = a.next_element()? {
                    values.push(v);
                }
                Ok(UniqueJson(json!(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = a.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate verdict key"));
                    }
                    let UniqueJson(v) = a.next_value()?;
                    values.insert(key, v);
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        d.deserialize_any(V)
    }
}

fn criteria(acceptance: &str) -> Vec<&str> {
    acceptance
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}
fn normalize(output: &str, identity: &Value, acceptance: &str) -> Result<Value, String> {
    if output.len() > 128 * 1024 {
        return Err("oversized verdict".into());
    }
    // Accept a prose preamble and an optional Markdown fence, but never search
    // past an earlier JSON candidate or ignore content after the verdict.
    // UniqueJson still rejects duplicate keys at every nesting level.
    let output = output.trim();
    let start = output.find(['{', '[']).into_iter()
        .chain(output.find("```"))
        .min().unwrap_or(0);
    let output = &output[start..];
    let output = if let Some(fenced) = output.strip_prefix("```json\n")
        .or_else(|| output.strip_prefix("```\n"))
        .or_else(|| output.strip_prefix("```json\r\n"))
        .or_else(|| output.strip_prefix("```\r\n"))
    {
        fenced.trim_end().strip_suffix("```")
            .ok_or("malformed verdict: unclosed JSON fence")?.trim()
    } else {
        output
    };
    let UniqueJson(mut v) =
        serde_json::from_str(output).map_err(|e| format!("malformed verdict: {e}"))?;
    if v["identity"] != *identity {
        return Err("wrong or missing review identity".into());
    }
    if !v["approved"].is_boolean()
        || v["summary"].as_str().is_none_or(|s| s.trim().is_empty())
        || !v["requires_dual"].is_boolean()
    {
        return Err("missing verdict fields".into());
    }
    for field in ["issues", "checks"] {
        if v[field].as_array().is_none_or(|a| {
            a.iter()
                .any(|s| s.as_str().is_none_or(|s| s.trim().is_empty()))
        }) {
            return Err(format!("malformed {field}"));
        }
    }
    if v["notes"].is_null() {
        v["notes"] = json!([]);
    }
    if v["notes"].as_array().is_none_or(|a| {
        a.iter()
            .any(|s| s.as_str().is_none_or(|s| s.trim().is_empty()))
    }) {
        return Err("malformed notes".into());
    }
    if !v["notes"].as_array().unwrap().is_empty() {
        v["approved"] = json!(false);
    }
    if v["approved"] == true && !v["issues"].as_array().unwrap().is_empty() {
        return Err("contradictory approval with requests".into());
    }
    if let Some(gap) = v["architecture_context_gap"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
    {
        v["approved"] = json!(false);
        v["issues"]
            .as_array_mut()
            .unwrap()
            .push(json!(format!("Resolve architectural context gap: {gap}")));
    }
    if v["approved"] == true {
        let required = criteria(acceptance);
        if v["criteria"].as_array().is_none_or(|items| {
            items.len() != required.len()
                || required.iter().any(|criterion| {
                    items
                        .iter()
                        .filter(|item| {
                            item["criterion"] == *criterion
                                && item["status"] == "passed"
                                && item["evidence"]
                                    .as_str()
                                    .is_some_and(|s| !s.trim().is_empty())
                        })
                        .count()
                        != 1
                })
        }) {
            return Err("approval lacks individual criterion evidence".into());
        }
        if v["checks"].as_array().unwrap().is_empty()
            || v["acceptance_evidence"]["acceptance"] != acceptance
            || v["acceptance_evidence"]["verified"] != true
            || v["acceptance_evidence"]["evidence"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
            || v["project_checks"].as_array().is_none_or(|a| {
                a.is_empty()
                    || a.iter().any(|c| {
                        !matches!(c["status"].as_str(), Some("passed" | "unavailable"))
                            || c["command"].as_str().is_none_or(|s| s.trim().is_empty())
                            || c["evidence"].as_str().is_none_or(|s| s.trim().is_empty())
                    })
            })
        {
            return Err(
                "approval lacks evidenced acceptance criteria or required project checks".into(),
            );
        }
    } else if Ctx::review_requests(&v).is_empty() {
        v["issues"] =
            json!(["Review rejected without actionable details; verify the stage end to end"]);
    }
    Ok(v)
}
fn aggregate(identity: &Value, records: &[Value]) -> Value {
    let required = identity["policy"]["required_roles"].as_array().unwrap();
    let mut roles = json!({"architect":"not_required","reviewer":"pending"});
    let mut requests = vec![];
    for role in required {
        let name = role.as_str().unwrap();
        let record = records.iter().find(|r| {
            r["identity"]["role"] == *role
                && r["identity"]["snapshot"] == identity["snapshot"]
                && r["identity"]["policy"] == identity["policy"]
                && r["identity"]["plan_id"] == identity["plan_id"]
                && r["identity"]["revision"] == identity["revision"]
                && r["identity"]["stage_id"] == identity["stage_id"]
                && r["identity"]["attempt_id"] == identity["attempt_id"]
                && r["identity"]["round"] == identity["round"]
        });
        roles[name] = json!(match record {
            Some(r) if r["approved"] == true && Ctx::review_requests(r).is_empty() => "approved",
            Some(_) => "changes_requested",
            None => "pending",
        });
        if let Some(r) = record {
            for text in Ctx::review_requests(r) {
                requests.push(json!({"role":name,"text":text}));
            }
        }
    }
    let approved = required
        .iter()
        .all(|r| roles[r.as_str().unwrap()] == "approved");
    json!({"identity":identity,"policy":identity["policy"],"roles":roles,"status":if approved {"approved"} else {"blocked"},"requests":requests})
}

#[cfg(test)]
use crate::model_selection::claude_model_limit;

impl Ctx {
    fn review_with_retry(&self, plan: &mut Value, idx: usize, base: &Value, role: &str, provider: &str, model: &str) -> Result<Value,String> {
        let mut current = (provider.to_string(), model.to_string());
        let mut visited = vec![current.clone()];
        loop {
            match self.invoke_review(plan,idx,base,role,&current.0,&current.1) {
                Ok(v) => {
                    plan["stages"][idx]["reassessment"]["status"] = json!("reusing");
                    self.save_plan(plan)?;
                    return Ok(v);
                },
                Err(error) => {
                    // Quota can be learned from the pre-launch probe or a CLI refusal.
                    // Switch only the fresh independent reviewer; keep this exact
                    // snapshot, round and required roles. Never loop over a model twice.
                    if role == "reviewer"
                        && let Some(next) = self.reviewer_quota_fallback(plan, idx, &current, &error, &visited)
                            .map_err(|e| format!("model routing blocked: {e}; work and checkpoint retained"))?
                        && !visited.contains(&next) {
                        visited.push(next.clone());
                        current = next;
                        continue;
                    }
                    if self.operational_retry(plan,idx,&error,role)? { continue; }
                    return Err(format!("model routing blocked: {role}/{}: {error}; restore an eligible independent reviewer; work and checkpoint retained", current.0));
                }
            }
        }
    }

    fn reviewer_quota_fallback(&self, plan: &Value, idx: usize, current: &(String, String), error: &str, visited: &[(String, String)]) -> Result<Option<(String, String)>, String> {
        let implementer = plan["stages"][idx]["implementer_provider"].as_str().ok_or("missing implementer provider for reviewer fallback")?;
        let requirements = self.model_requirements("reviewer",Some(implementer))?;
        let choice = (current.0.clone(),current.1.clone(),"provider_default".into());
        Ok(self.model_fallback(&requirements,&choice,error,visited)?.map(|next| (next.0,next.1)))
    }

    fn architect_review_with_selection(&self, plan: &mut Value, idx: usize, base: &Value) -> Result<Value,String> {
        let requirements = self.model_requirements("architect",None)?;
        let (verdict,_) = self.with_selected_model(&requirements,None, |choice| {
            *plan = self.load_plan().ok_or("missing current review plan")?;
            let cp = self.architecture_store().checkpoint(plan)?;
            let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
            let changed = crate::catalogue::Provider::parse(&choice.0).is_some_and(|provider| {
                let facts = self.model_facts(&policy,provider,&choice.1,Some(&choice.2));
                !crate::agent::same_model(&choice.0,facts["resolved_id"].as_str().unwrap_or(&choice.1),
                    cp["effective_model"]["model"].as_str().unwrap_or(""))
            });
            if changed || cp["session"]["provider"] != choice.0 || cp["context_status"] != "ready" {
                *plan = self.architect_publish_selected(plan.clone(),Some(plan),
                    "review model recovery",Some(choice.clone()))?;
            }
            self.invoke_review(plan,idx,base,"architect",&choice.0,&choice.1)
        })?;
        Ok(verdict)
    }
    fn invoke_review(
        &self,
        plan: &mut Value,
        idx: usize,
        base: &Value,
        role: &str,
        provider: &str,
        model: &str,
    ) -> Result<Value, String> {
        let _guard = self.session.architect_lock.lock().unwrap();
        let mut identity = base.clone();
        identity["role"] = json!(role);
        *plan = self.load_plan().ok_or("missing current review plan")?;
        let mut cp = self.architecture_store().checkpoint(plan)?;
        let mut context_stage = plan["stages"][idx].clone();
        // Only preceding findings, never the other role's current endorsement.
        context_stage["last_verdict"] = context_stage["previous_requests"].clone();
        context_stage["last_verdict_valid"] = json!(true);
        let mut prompt = self.stage_prompt(REVIEW_PROMPT, plan, &context_stage)?;
        if role == "architect" {
            prompt = prompt.replacen(
                "You are an independent reviewer in a fresh session.",
                "You are this plan's persistent architect in the saved session.",
                1,
            );
        }
        if plan["plan_id"] != base["plan_id"]
            || plan["revision"] != base["revision"]
            || plan["stages"][idx]["attempt_id"] != base["attempt_id"]
        {
            return Err("plan identity changed before review".into());
        }
        prompt.push_str(&format!(
            "\nCRITERIA TO EVIDENCE: {}\n",
            json!(criteria(
                plan["stages"][idx]["acceptance"].as_str().unwrap_or("")
            ))
        ));
        prompt.push_str(&format!("\nREVIEW IDENTITY (echo exactly): {identity}\nSaved constraints: {}\nCompleted interfaces: {}\nGuidance: {}\nDecision history: .forge/architecture/{}/events.jsonl\n", cp["constraints"], cp["completed_interfaces"], cp["guidance"][plan["stages"][idx]["id"].to_string()], plan["plan_id"].as_str().unwrap()));
        crate::architecture::atomic_json(&self.forge_path("review-identity.json"), &identity)?;
        prompt.push_str("\nThe engine also wrote the exact identity to .forge/review-identity.json (read-only during this review). Assemble your final verdict in private /tmp using a script: load that file with json.load, assign the resulting object to verdict['identity'], and serialize the verdict with json.dumps. Return that exact serialized JSON. Do not manually transcribe hashes or reconstruct the identity. The engine still validates the complete identity and rejects any mismatch.\n");
        if role == "architect" {
            let requests: Vec<_> = plan["stages"][idx]["reviews"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| {
                    r["identity"]["round"] == base["round"]
                        && r["identity"]["attempt_id"] == base["attempt_id"]
                })
                .flat_map(Ctx::review_requests)
                .collect();
            prompt.push_str(&format!(
                "Current actionable independent requests (not an endorsement): {requests:?}\n"
            ));
            prompt.push_str("\nYou are the persistent architect reviewing recorded design, cross-stage interfaces and regressions. Your verdict has independent authority. For conflicting requests, record architectural clarification in architecture_context_gap without dismissing either role's unresolved findings.\n");
        }
        self.set_step(
            plan["stages"][idx]["id"].as_i64(),
            &format!("reviewing ({role})"),
        );
        for name in ["verdict.json", "architect-verdict.json"] {
            match fs::remove_file(self.forge_path(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let turn = crate::architecture::identity();
        let pending = self
            .forge_path("architecture")
            .join(plan["plan_id"].as_str().unwrap())
            .join("architect-pending.json");
        let session = cp["session"]["reference"].as_str().map(str::to_owned);
        if role == "architect" {
            if cp["session"]["provider"] != provider || cp["context_status"] != "ready" {
                return Err("architect session requires recovery".into());
            }
            crate::architecture::atomic_json(
                &pending,
                &json!({"turn":turn,"previous_session":cp["session"]}),
            )?;
        }
        let mock = provider == "mock";
        #[cfg(test)] let mock = mock || self.app.settings.lock().unwrap()["test_fake_providers"] == true;
        #[cfg(test)] {
            let mut settings = self.app.settings.lock().unwrap();
            if !settings["test_review_sessions"].is_array() { settings["test_review_sessions"] = json!([]); }
            settings["test_review_sessions"].as_array_mut().unwrap().push(json!({"role":role,"provider":provider,"model":model,"session":if role == "architect" {session.clone()} else {None},"prompt":prompt}));
        }
        let result = if mock {
            self.mock_review(&identity, &prompt, &plan["stages"][idx])
        } else {
            let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
            let facts = self.model_facts(&policy,crate::catalogue::Provider::parse(provider).ok_or("invalid review provider")?,model,None);
            let effort = facts["effort"].as_str().unwrap_or("provider_default");
            self.run_agent(&AgentRequest {
                role: if role == "architect" {
                    "architect_review"
                } else {
                    "reviewer"
                },
                provider,
                model,
                effort: &effort,
                session: if role == "architect" {
                    session.as_deref()
                } else {
                    None
                },
                prompt: &prompt,
            })
        }?;
        if self.session.stop_requested.load(Ordering::SeqCst) {
            return Err("review stopped; approval invalid".into());
        }
        if snapshot(self.project())? != base["snapshot"] {
            return Err("implementation or HEAD changed during review".into());
        }
        if !mock {
            let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
            let selected = self.model_facts(&policy, crate::catalogue::Provider::parse(provider).ok_or("invalid reviewer provider")?, model, None);
            let expected = selected["resolved_id"].as_str().unwrap_or(model);
            if !result.model_reported || selected["eligible"] != true || !crate::agent::same_model(provider, expected, &result.effective_model) {
                return Err(format!("model routing blocked: {role} effective model or eligibility changed (expected {expected}, reported {}, model_reported {}, eligible {})",
                    result.effective_model, result.model_reported, selected["eligible"]));
            }
        }
        let mut verdict = normalize(
            &result.output,
            &identity,
            plan["stages"][idx]["acceptance"].as_str().unwrap_or(""),
        )?;
        verdict["version"] = json!(1);
        verdict["id"] = json!(crate::architecture::identity());
        verdict["plan_id"] = base["plan_id"].clone();
        verdict["stage_id"] = base["stage_id"].clone();
        verdict["provider"] = json!(provider);
        verdict["model"] = json!(if result.effective_model.is_empty() {
            model
        } else {
            &result.effective_model
        });
        verdict["fresh_session"] = json!(role == "reviewer");
        verdict["role"] = json!(role);
        verdict["round"] = base["round"].clone();
        verdict["attempt_id"] = base["attempt_id"].clone();
        verdict["revision"] = base["revision"].clone();
        verdict["policy"] = base["policy"].clone();
        verdict["unix"] = json!(unix_timestamp());
        if role == "architect"
            && (result.session.as_deref() != session.as_deref()
                || result
                    .session
                    .as_deref()
                    .is_none_or(|s| !crate::agent::session_id(s)))
        {
            return Err("architect review session identity mismatch".into());
        }
        let stage = &mut plan["stages"][idx];
        stage["context_valid"] = json!(true);
        stage
            .as_object_mut()
            .unwrap()
            .entry("reviews")
            .or_insert(json!([]))
            .as_array_mut()
            .unwrap()
            .push(verdict.clone());
        if role == "reviewer" {
            stage["last_verdict"] = verdict.clone();
            stage["last_verdict_valid"] = json!(true);
        }
        if role == "architect" {
            let reference = result
                .session
                .as_deref()
                .filter(|s| crate::agent::session_id(s))
                .ok_or("missing architect session identity")?;
            if Some(reference) != session.as_deref() {
                return Err("architect review changed session identity".into());
            }
            cp["last_turn"] = json!(turn);
            cp["session"]["checkpoint_reference"] = json!(turn);
            let _lock = self.session.persistence_lock.lock().unwrap();
            *plan = self.architecture_store().publish(
                plan.clone(),
                cp,
                json!({"kind":"architect_review","reviews":[verdict.clone()],"turn":turn}),
            )?;
        } else {
            self.save_plan(plan)?;
        }
        self.record_stage_usage(plan, idx, role, provider, result.usage)?;
        Ok(verdict)
    }

    fn mock_review(
        &self,
        identity: &Value,
        prompt: &str,
        stage: &Value,
    ) -> Result<AgentResult, String> {
        let role = identity["role"].as_str().unwrap();
        let mut settings = self.app.settings.lock().unwrap();
        let key = format!("mock_{role}_prompts");
        settings
            .as_object_mut()
            .unwrap()
            .entry(key)
            .or_insert(json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!(prompt));
        let key = if role == "architect" {
            "mock_architect_verdicts"
        } else {
            "mock_verdicts"
        };
        #[cfg(test)]
        if let Some(action) = settings[format!("mock_{role}_actions")]
            .as_array_mut()
            .filter(|a| !a.is_empty())
            .map(|a| a.remove(0))
        {
            if let Some(writes) = action["write"].as_object() {
                for (path, content) in writes {
                    fs::write(
                        PathBuf::from(self.project()).join(path),
                        content.as_str().unwrap(),
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            if let Some(args) = action["git"].as_array() {
                self.git(&args.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>())?;
            }
            if action["stop"] == true {
                self.session.stop_requested.store(true, Ordering::SeqCst);
            }
            if let Some(error) = action["error"].as_str() {
                return Err(error.into());
            }
        }
        let supplied = settings[key]
            .as_array_mut()
            .filter(|a| !a.is_empty())
            .map(|a| a.remove(0));
        let mut v = supplied.unwrap_or(json!({"approved":true,"summary":"Inspected mock implementation","issues":[],"checks":["Verified fixture"]}));
        if let Some(raw) = v.as_str() {
            return Ok(AgentResult {
                output: raw.into(),
                ..AgentResult::default()
            });
        }
        if v["identity"].is_null() {
            v["identity"] = identity.clone();
        }
        if v["requires_dual"].is_null() {
            v["requires_dual"] = json!(false);
        }
        if v["criteria"].is_null() {
            v["criteria"] = json!(criteria(stage["acceptance"].as_str().unwrap_or("")).iter().map(|s| json!({"criterion":s,"status":"passed","evidence":"Mock individual criterion verified"})).collect::<Vec<_>>());
        }
        if v["notes"].is_null() {
            v["notes"] = json!([]);
        }
        if v["summary"].is_null() {
            v["summary"] = json!("Mock review");
        }
        if v["checks"].is_null() {
            v["checks"] = json!(["Verified mock fixture"]);
        }
        if v["acceptance_evidence"].is_null() {
            v["acceptance_evidence"] = json!({"acceptance":stage["acceptance"].as_str().unwrap_or(""),"verified":true,"evidence":"Mock acceptance verified"});
        }
        if v["project_checks"].is_null() {
            v["project_checks"] = json!([{"command":"mock checks","status":"passed","evidence":"Mock checks passed"}]);
        }
        let usage = settings["mock_usage"].as_object().map(|u| AgentUsage {
            input_tokens: u.get("input").and_then(Value::as_i64).unwrap_or(0),
            output_tokens: u.get("output").and_then(Value::as_i64).unwrap_or(0),
            total_tokens: u.get("total").and_then(Value::as_i64).unwrap_or(0),
            model: u.get("model").and_then(Value::as_str).unwrap_or("").into(),
        });
        drop(settings);
        if usage.is_some() {
            self.log_agent_finished(role, "mock", "", usage.as_ref());
        }
        let session = self
            .architecture_store()
            .checkpoint(&self.load_plan().unwrap())?["session"]["reference"]
            .as_str()
            .map(str::to_owned);
        Ok(AgentResult {
            output: v.to_string(),
            session,
            usage,
            completed: true,
            ..AgentResult::default()
        })
    }

    pub(super) fn run_review_stage(
        &self,
        plan: &mut Value,
        idx: usize,
    ) -> Result<&'static str, String> {
        let budget = plan["stages"][idx]["review_budget"].as_u64().unwrap_or(0);
        let sid = plan["stages"][idx]["id"].as_i64().unwrap();
        if plan["stages"][idx]["attempt_head"].is_null() {
            plan["stages"][idx]["attempt_head"] = json!(self.git(&["rev-parse", "HEAD"])?);
            self.save_plan(plan)?;
        }
        if plan["stages"][idx]["attempt_head"] != self.git(&["rev-parse", "HEAD"])? {
            return Err("HEAD changed during stage attempt".into());
        }
        let start = plan["stages"][idx]["rounds"].as_u64().unwrap_or(0);
        if start > budget {
            let architect =
                if plan["stages"][idx]["review_policy"]["scope"] == "ordinary_documentation" {
                    "not_required"
                } else {
                    "no_current_verdict"
                };
            plan["stages"][idx]["review_gate"] = json!({"status":"exhausted","roles":{"architect":architect,"reviewer":"no_current_verdict"}});
            self.save_plan(plan)?;
            return Ok("exhausted");
        }
        for round in start..=budget {
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Ok("stopped");
            }
            let mut assignment = self.assignment_boundary(plan, idx).map_err(|e| format!("model routing blocked: {e}"))?;
            let (mut reviewer, mut reviewer_model) = self.reviewer_config(assignment["effective"]["provider"].as_str().ok_or("missing agreed provider")?)
                .map_err(|e| format!("model routing blocked: {e}"))?;
            // Reserve the round before any invocation; errors/restarts cannot replenish it.
            plan["stages"][idx]["rounds"] = json!(round + 1);
            plan["stages"][idx]["review_gate"] =
                json!({"status":"pending","roles":{"architect":"pending","reviewer":"pending"}});
            plan["stages"][idx]["last_verdict_valid"] = json!(false);
            self.save_plan(plan)?;
            let template = if round == 0 {
                IMPLEMENT_PROMPT
            } else {
                FIX_PROMPT
            };
            self.set_step(
                Some(sid),
                if round == 0 { "implementing" } else { "fixing" },
            );
            let mut stage = plan["stages"][idx].clone();
            if round > 0 {
                stage["last_verdict"] = stage["previous_requests"].clone();
                stage["last_verdict_valid"] = json!(true);
            }
            let role = if round == 0 { "implementer" } else { "fixer" };
            let (output, turn) = loop {
                if self.session.stop_requested.load(Ordering::SeqCst) { return Ok("stopped"); }
                let turn = crate::architecture::identity();
                stage = plan["stages"][idx].clone();
                if round > 0 { stage["last_verdict"] = stage["previous_requests"].clone(); stage["last_verdict_valid"] = json!(true); }
                let prompt = self.stage_prompt(template, plan, &stage)? + &self.outcome_prompt(plan, idx, &turn);
                let effective = &assignment["effective"];
                let implementer = effective["provider"].as_str().ok_or("missing agreed provider")?;
                let model = effective["model"].as_str().ok_or("missing agreed model")?;
                let effort = effective["native_effort"].as_str().ok_or("missing agreed effort")?;
                plan["stages"][idx]["implementer_provider"] = json!(implementer);
                let invocation = json!({"turn_id":turn,"agreement_id":assignment["id"],"role":role,"proposed":assignment["validated_proposal"],"requested":effective,"unix":crate::util::unix_timestamp(),"status":"launching"});
                if !plan["stages"][idx]["model_invocations"].is_array() { plan["stages"][idx]["model_invocations"] = json!([]); }
                plan["stages"][idx]["model_invocations"].as_array_mut().unwrap().push(invocation);
                self.save_plan(plan)?;
                let result = self.run_agent(&crate::agent::AgentRequest { role, provider:implementer, model, effort, session:None, prompt:&prompt });
                let record = plan["stages"][idx]["model_invocations"].as_array_mut().unwrap().last_mut().unwrap();
                match &result {
                    Ok(output) => {
                        record["effective"] = json!({"provider":implementer,"model":output.effective_model,"native_effort":effort});
                        record["model_reported"] = json!(output.model_reported);
                        record["unexpected_substitution"] = json!((!output.model_reported || !crate::agent::same_model(implementer, model, &output.effective_model)));
                        record["status"] = json!("completed");
                        record["usage"] = output.usage.as_ref().map(|u| json!({"input":u.input_tokens,"output":u.output_tokens,"total":u.total_tokens})).unwrap_or(Value::Null);
                        record["verification_state"] = json!(if output.model_reported && crate::agent::same_model(implementer, model, &output.effective_model) { "execution_verified" } else { "unexpected_substitution" });
                    }
                    Err(error) => { record["status"] = json!("failed"); record["error"] = json!(error); record["failure_kind"] = json!(super::reassessment::failure_kind(error)); }
                }
                self.save_plan(plan)?;
                if self.session.stop_requested.load(Ordering::SeqCst) { return Ok("stopped"); }
                match result {
                    Ok(output) => {
                        self.record_stage_usage(plan, idx, role, implementer, output.usage.clone())?;
                        if !output.model_reported || !crate::agent::same_model(implementer, model, &output.effective_model) {
                            let error = "model routing blocked: unexpected provider model substitution; saved work retained. Correct the stage/global model constraint and reconcile before retrying";
                            plan["stages"][idx]["model_block"] = json!(error);
                            self.save_plan(plan)?;
                            return Err(error.into());
                        }
                        plan["stages"][idx]["reassessment"]["status"] = json!("reusing");
                        self.save_plan(plan)?;
                        break (output, turn);
                    }
                    Err(error) => {
                        if self.operational_retry(plan, idx, &error,role)? { continue; }
                        self.reassess(plan, idx, "provider_operational_failure", json!({"failure_kind":super::reassessment::failure_kind(&error),"error":crate::util::last_chars(&error,2000),"provider":implementer}))?;
                        assignment = self.assignment_boundary(plan, idx)?;
                        (reviewer, reviewer_model) = self.reviewer_config(assignment["effective"]["provider"].as_str().unwrap())?;
                    }
                }
            };
            let trigger = self.implementer_outcome(plan, idx, &turn, &output)?;
            if let Some((kind,evidence)) = trigger {
                if round < budget { self.reassess(plan, idx, &kind, evidence)?; continue; }
                return Ok("exhausted");
            }
            if output.usage.as_ref().is_some_and(|u| {
                let limit = assignment["policy_inputs"]["limits"]["context_window"].as_u64().unwrap_or(0);
                let percent = plan["stages"][idx]["reassessment"]["limits"]["context_percent"].as_u64().unwrap_or(85);
                limit > 0 && u.input_tokens.max(0) as u64 >= limit.saturating_mul(percent) / 100
            }) && round < budget {
                self.reassess(plan, idx, "context_pressure", json!({"measured_input_tokens":output.usage.as_ref().unwrap().input_tokens,"context_window":assignment["policy_inputs"]["limits"]["context_window"]}))?;
            }
            let snap = snapshot(self.project())?;
            if snap["head"] != plan["stages"][idx]["attempt_head"] {
                return Err("implementer changed HEAD".into());
            }
            let mut policy = classify(
                self.project(),
                &plan["stages"][idx],
                plan["stages"][idx]["dual_promoted"] == true,
            )?;
            if assignment["validated_proposal"]["task"] == "documentation" && policy["scope"] != "ordinary_documentation"
                && plan["stages"][idx]["routing_scope_floor"].is_null() {
                plan["stages"][idx]["routing_scope_floor"] = json!(2);
                self.save_plan(plan)?;
                self.reassess(plan,idx,"material_scope_change",json!({"engine_scope":policy,"planned_task":"documentation"}))?;
            }
            let mut base = json!({"plan_id":plan["plan_id"],"revision":plan["revision"],"stage_id":sid,"attempt_id":plan["stages"][idx]["attempt_id"],"round":round+1,"policy":policy,"snapshot":snap});
            let mut records = vec![];
            loop {
                plan["stages"][idx]["review_policy"] = policy.clone();
                if policy["scope"] != "ordinary_documentation" {
                    plan["stages"][idx]["dual_promoted"] = json!(true);
                }
                plan["stages"][idx]["review_gate"] = aggregate(&base, &records);
                self.save_plan(plan)?;
                // Independent goes first: a scope promotion can bind both required
                // verdicts to the promoted policy before any fixer is allowed.
                let v =
                    self.review_with_retry(plan, idx, &base, "reviewer", &reviewer, &reviewer_model)?;
                let promote = v["requires_dual"] == true
                    || v["architecture_context_gap"]
                        .as_str()
                        .is_some_and(|s| !s.trim().is_empty());
                records.push(v);
                plan["stages"][idx]["review_gate"] = aggregate(&base, &records);
                self.save_plan(plan)?;
                if policy["scope"] == "ordinary_documentation" && promote {
                    let findings = records.last().map(Ctx::review_requests).unwrap_or_default();
                    plan["stages"][idx]["previous_requests"] = json!({"approved":false,"summary":"Scope promotion; verify prior requests under dual policy", "issues":findings,"notes":[],"checks":[]});
                    policy = dual("Independent reviewer identified architectural impact");
                    base["policy"] = policy.clone();
                    plan["stages"][idx]["dual_promoted"] = json!(true);
                    continue;
                }
                if policy["scope"] != "ordinary_documentation" {
                    records.push(self.architect_review_with_selection(plan, idx, &base)?);
                }
                break;
            }
            let gate = aggregate(&base, &records);
            let requests: Vec<_> = gate["requests"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    format!(
                        "[{}] {}",
                        r["role"].as_str().unwrap(),
                        r["text"].as_str().unwrap()
                    )
                })
                .collect();
            plan["stages"][idx]["previous_requests"] = json!({"approved":false,"summary":"Combined unresolved role requests; both retain authority", "issues":requests,"notes":[],"checks":[]});
            plan["stages"][idx]["review_gate"] = gate.clone();
            self.save_plan(plan)?;
            self.log_event(
                "review",
                &format!(
                    "stage {sid} gate {}: architect {}, reviewer {}",
                    gate["status"], gate["roles"]["architect"], gate["roles"]["reviewer"]
                ),
            );
            if gate["status"] == "approved" {
                return Ok("approved");
            }
            // Record clarification without erasing either role's requests.
            let gaps: Vec<_> = records
                .iter()
                .filter_map(|r| r["architecture_context_gap"].as_str())
                .filter(|s| !s.trim().is_empty())
                .collect();
            if !gaps.is_empty() && round < budget {
                let current = self.load_plan().ok_or("missing plan")?;
                let mut cp = self.architecture_store().checkpoint(&current)?;
                cp["guidance"][sid.to_string()]["valid"] = json!(false);
                cp["guidance"][sid.to_string()]["invalidation_trigger"] =
                    json!("reviewer_context_gap");
                cp["context_gap"] = json!({"stage_id":sid,"text":gaps.join("\n"),"unresolved_requests":gate["requests"]});
                let current = self.architecture_store().publish(current, cp, json!({"kind":"review_clarification_requested","requests":gate["requests"],"gaps":gaps}))?;
                *plan = current.clone();
                if self.repeated_findings(plan, idx, &gate["requests"])? { continue; }
                let current = self.load_plan().ok_or("missing clarification plan")?;
                *plan = self.architect_publish(
                    current.clone(),
                    Some(&current),
                    "review architectural clarification; retain both roles' unresolved authority",
                )?;
            } else if round < budget {
                self.repeated_findings(plan, idx, &gate["requests"])?;
            }
        }
        Ok("exhausted")
    }

    fn validate_commit_approval(&self, plan: &Value, idx: usize) -> Result<(), String> {
        let stage = &plan["stages"][idx];
        let gate = &stage["review_gate"];
        if gate["status"] != "approved"
            || gate["identity"]["plan_id"] != plan["plan_id"]
            || gate["identity"]["revision"] != plan["revision"]
            || gate["identity"]["attempt_id"] != stage["attempt_id"]
            || gate["identity"]["round"] != stage["rounds"]
        {
            return Err("no current aggregate review approval".into());
        }
        let persisted = self.load_plan().ok_or("missing persisted review gate")?;
        if persisted["plan_id"] != plan["plan_id"]
            || persisted["revision"] != plan["revision"]
            || persisted["stages"][idx]["review_gate"] != *gate
            || crate::plan::stage_inputs(&persisted, idx) != crate::plan::stage_inputs(plan, idx)
        {
            return Err("plan changed after review".into());
        }
        let current = aggregate(
            &gate["identity"],
            stage["reviews"].as_array().ok_or("missing reviews")?,
        );
        if current["status"] != "approved" {
            return Err("missing current role approvals".into());
        }
        for role in gate["policy"]["required_roles"]
            .as_array()
            .ok_or("invalid gate roles")?
        {
            let mut identity = gate["identity"].clone();
            identity["role"] = role.clone();
            let record = stage["reviews"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["identity"] == identity)
                .ok_or("missing current immutable verdict")?;
            if normalize(
                &record.to_string(),
                &identity,
                stage["acceptance"].as_str().unwrap_or(""),
            )?["approved"]
                != true
            {
                return Err("current verdict is not clean and evidenced".into());
            }
        }
        Ok(())
    }

    /// Recover the narrow crash window after Git's CAS commit and before the
    /// completed-stage checkpoint publication. Never synthesize an approval.
    pub(crate) fn recover_committed_stages(&self) -> Result<Vec<i64>, String> {
        let Some(mut plan) = self.load_plan() else { return Ok(Vec::new()); };
        let Some(idx) = plan["stages"].as_array().ok_or("invalid stages")?.iter()
            .position(|stage| stage["status"] != "committed") else { return Ok(Vec::new()); };
        let stage = &plan["stages"][idx];
        if stage["review_gate"]["status"] != "approved" { return Ok(Vec::new()); }
        let expected = &stage["review_gate"]["identity"]["snapshot"];
        let actual = snapshot(self.project())?;
        if actual["head"] == expected["head"] { return Ok(Vec::new()); }
        self.validate_commit_approval(&plan, idx)?;
        self.git(&["diff", "--quiet"])
            .and_then(|_| self.git(&["diff", "--cached", "--quiet"]))
            .map_err(|_| "uncommitted index/worktree changes prevent commit recovery")?;
        let parents = self.git(&["rev-list", "--parents", "-n", "1", "HEAD"])?;
        let parents: Vec<_> = parents.split_whitespace().collect();
        if parents.len() != 2 || Some(parents[1]) != expected["head"].as_str()
            || actual["tree"] != expected["tree"] || actual["content"] != expected["content"]
            || self.git(&["rev-parse", "HEAD^{tree}"])? != expected["tree"]
            || self.git(&["show", "-s", "--format=%B", "HEAD"])? != stage["commit"].as_str().unwrap_or("forge: stage") {
            return Err("HEAD/worktree does not match the saved independently reviewed commit; recovery refused".into());
        }
        let sid = stage["id"].as_i64().ok_or("missing stage ID")?;
        plan["stages"][idx]["sha"] = json!(self.git(&["rev-parse", "--short", "HEAD"])?);
        self.finish_stage(&mut plan, idx, "committed")?;
        self.log_event("stage", &format!("stage {sid} recovered: exact reviewed commit already exists; completion checkpoint restored"));
        Ok(vec![sid])
    }

    pub(super) fn commit_reviewed(
        &self,
        plan: &Value,
        idx: usize,
        message: &str,
    ) -> Result<Option<String>, String> {
        self.validate_commit_approval(plan, idx)?;
        let stage = &plan["stages"][idx];
        let gate = &stage["review_gate"];
        let expected = &gate["identity"]["snapshot"];
        if snapshot(self.project())? != *expected {
            return Err("implementation/index/HEAD changed after review".into());
        }
        if classify(self.project(), stage, stage["dual_promoted"] == true)?["scope"]
            != gate["policy"]["scope"]
        {
            return Err("review scope changed before commit".into());
        }
        // Match snapshot's staging: git add rejects an explicitly excluded ignored path.
        self.git(&["add", "-A"])?;
        self.git(&["reset", "-q", "HEAD", "--", ".forge"])?;
        let staged = self.git(&["write-tree"])?;
        let actual = snapshot(self.project())?;
        if actual["head"] != expected["head"]
            || actual["content"] != expected["content"]
            || actual["tree"] != expected["tree"]
            || staged != expected["tree"]
        {
            return Err("staged tree differs from reviewed content".into());
        }
        if staged == self.git(&["rev-parse", "HEAD^{tree}"])? {
            return Ok(None);
        }
        // commit-tree fixes the exact reviewed tree; CAS update-ref rejects concurrent HEAD
        // changes, and avoids hooks mutating the tree between verification and commit.
        let head = expected["head"].as_str().unwrap();
        let sha = self.git(&["commit-tree", &staged, "-p", head, "-m", message])?;
        let last = snapshot(self.project())?;
        if last != actual || self.session.stop_requested.load(Ordering::SeqCst) {
            return Err("external edit or stop before commit".into());
        }
        self.git(&["update-ref", "-m", message, "HEAD", &sha, head])?;
        let short = self.git(&["rev-parse", "--short", "HEAD"])?;
        self.log_event("git", &format!("committed {short}: {message}"));
        Ok(Some(short))
    }
}

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;
