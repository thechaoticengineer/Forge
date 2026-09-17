//! Spec and scenario approvals of one feature folder (M2, S15-S18, D10).
//!
//! An approval binds exactly the reviewed content to a full Git commit:
//!
//! * every refusal check runs before Git is touched, so a refused approval
//!   leaves HEAD, the index and the runtime state untouched;
//! * the commit is path-scoped to `docs/features/<slug>/` on both `git add`
//!   and `git commit`, so unrelated staged and unstaged changes survive it;
//! * the folder hash is recomputed after the commit and the approval is only
//!   recorded when it still equals the reviewed hash;
//! * a folder that is already committed produces no new commit, which makes a
//!   retry after a failed state publication idempotent: it records the current
//!   HEAD, which already holds exactly the reviewed content (M2-RISK-GIT-STATE).
//!
//! Approvals are append-only. Any later change to the folder changes its
//! content hash, so the derived status returns to `draft` while the earlier
//! approvals remain as history (S18); both endpoints then refuse until a new
//! approving review and a new spec approval exist.
//!
//! Lock order: `queue_lock` (held by the synchronous endpoint) -> `busy`
//! (claimed by the endpoint, released by `BusyGuard`) -> `feature_lock`,
//! which stays innermost and is never held across a Git invocation. Both
//! endpoints hold `feature_lock` from the final content check through the
//! append, so what an approval records always describes the folder snapshot it
//! was decided on.
use super::{Ctx, Session};
use crate::feature_state::{self, Refusal};
use crate::util::unix_timestamp;
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::Ordering;

/// Releases the session's busy claim when a synchronous approval returns, on
/// every path including an early refusal. Unlike `WorkerGuard` it touches
/// nothing else: an approval is not a worker and owns no run state.
pub(crate) struct BusyGuard<'a>(pub(crate) &'a Session);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::SeqCst);
    }
}

/// A refusal: the request was well formed, but the feature is not in a state
/// that may be approved. Nothing was committed or recorded.
fn refuse(message: impl Into<String>) -> Refusal {
    Refusal { status: 409, message: message.into() }
}

/// A failure of the engine itself (unreadable state, a failing Git command,
/// a state file that could not be published).
fn failed(message: String) -> Refusal {
    Refusal { status: 500, message }
}

/// The latest approval of `kind`, or `None`.
fn latest_approval<'a>(state: &'a Value, kind: &str) -> Option<&'a Value> {
    state["approvals"]
        .as_array()
        .into_iter()
        .flatten()
        .rfind(|approval| approval["kind"] == json!(kind))
}

/// Appends one approval and returns the status derived from the updated
/// state, so the answer always reflects the recorded history rather than an
/// assumption about it. The caller holds `feature_lock`, so the check that
/// admitted the approval and this append share one critical section.
fn record(ctx: &Ctx, slug: &str, approval: Value, hash: &str) -> Result<&'static str, Refusal> {
    feature_state::update_locked(ctx, slug, |state| {
        state["approvals"]
            .as_array_mut()
            .ok_or("feature state approvals is not an array")?
            .push(approval);
        Ok(feature_state::spec_status(state, hash))
    })
    .map_err(failed)
}

impl Ctx {
    /// Approves the spec: commits `docs/features/<slug>/` and records the
    /// commit with the reviewed content hash.
    pub(crate) fn approve_feature_spec(&self, slug: &str) -> Result<Value, Refusal> {
        let dir = feature_state::feature_dir(self, slug);
        let pathspec = format!("docs/features/{slug}");

        // Every refusal check happens before the first Git command, so a
        // refusal can never leave a commit or a staged change behind.
        let reviewed_hash = {
            let _guard = self.session.feature_lock.lock().unwrap();
            let state = feature_state::load(self, slug).map_err(failed)?;
            let hash = feature_state::content_hash(&dir).map_err(failed)?;
            let review = feature_state::latest_review(&state);
            if review.is_null() {
                return Err(refuse("no architect spec review"));
            }
            if review["approved"] != json!(true) {
                return Err(refuse("latest architect spec review requested changes"));
            }
            if review["content_hash"] != json!(&hash) {
                return Err(refuse("feature changed since the approving review"));
            }
            hash
        };

        // Stage and commit only this feature folder. `git commit -- <path>`
        // writes a commit of HEAD plus that path, leaving every other index
        // entry exactly as the user left it.
        self.git(&["add", "-A", "--", &pathspec]).map_err(failed)?;
        let staged = self
            .git(&["diff", "--cached", "--name-only", "HEAD", "--", &pathspec])
            .map_err(failed)?;
        if !staged.trim().is_empty() {
            let message = format!("docs(features): approve {slug} spec");
            self.git(&["commit", "-q", "-m", &message, "--", &pathspec]).map_err(failed)?;
        }
        let commit = self.git(&["rev-parse", "HEAD"]).map_err(failed)?;

        // The commit holds exactly the content that was hashed above; a folder
        // changed in between is never recorded as approved. The check and the
        // append share the lock, so nothing this engine does can slip between
        // them.
        let _guard = self.session.feature_lock.lock().unwrap();
        let current = feature_state::content_hash(&dir).map_err(failed)?;
        if current != reviewed_hash {
            return Err(refuse("feature changed during approval"));
        }

        let approval = json!({
            "kind": "spec",
            "commit": commit,
            "content_hash": reviewed_hash,
            "unix": unix_timestamp(),
        });
        let spec_status = record(self, slug, approval, &reviewed_hash)?;
        Ok(json!({
            "ok": true,
            "commit": commit,
            "content_hash": reviewed_hash,
            "spec_status": spec_status,
        }))
    }

    /// Approves the scenarios of a feature whose spec approval still describes
    /// the current content. Creates no commit of its own: it records the
    /// commit of that spec approval (S17).
    pub(crate) fn approve_feature_scenarios(&self, slug: &str) -> Result<Value, Refusal> {
        self.approve_feature_scenarios_at(slug, "")
    }

    /// `approve_feature_scenarios` with the failure seam the tests use:
    /// `stop == "concurrent_edit"` rewrites `scenarios.md` right after it was
    /// parsed, which is the one moment an edit could otherwise pair recorded
    /// scenario IDs with a hash of different content.
    pub(crate) fn approve_feature_scenarios_at(
        &self,
        slug: &str,
        stop: &str,
    ) -> Result<Value, Refusal> {
        let dir = feature_state::feature_dir(self, slug);
        // One critical section for the whole approval: the hash that admits
        // it, the scenarios.md the IDs are read from, the re-check below and
        // the append all belong to the same folder snapshot. No Git command
        // runs here, so the lock is never held across one.
        let _guard = self.session.feature_lock.lock().unwrap();
        let state = feature_state::load(self, slug).map_err(failed)?;
        let hash = feature_state::content_hash(&dir).map_err(failed)?;
        let Some(approval) = latest_approval(&state, "spec") else {
            return Err(refuse("spec is not approved"));
        };
        if approval["content_hash"] != json!(&hash) {
            return Err(refuse("feature changed since spec approval"));
        }
        let commit = approval["commit"].as_str().unwrap_or_default().to_string();

        let path = dir.join("scenarios.md");
        let scenarios = fs::read_to_string(&path)
            .map_err(|e| failed(format!("could not read {}: {e}", path.display())))?;
        let scenario_ids = crate::features::scenario_ids(&scenarios);
        if stop == "concurrent_edit" {
            fs::write(&path, format!("{scenarios}\n## S99: Injected\n"))
                .map_err(|e| failed(format!("could not inject an edit: {e}")))?;
        }

        // The lock serializes this engine's own work, but an editor or a user
        // writing into the folder takes no lock at all. So the IDs just parsed
        // are checked against the folder once more before they are recorded:
        // an edit that landed while scenarios.md was being read refuses the
        // approval instead of binding those IDs to a hash of other content.
        let current = feature_state::content_hash(&dir).map_err(failed)?;
        if current != hash {
            return Err(refuse("feature changed during approval"));
        }

        let approval = json!({
            "kind": "scenarios",
            "scenario_ids": scenario_ids,
            "commit": commit,
            "content_hash": hash,
            "unix": unix_timestamp(),
        });
        let spec_status = record(self, slug, approval, &hash)?;
        Ok(json!({
            "ok": true,
            "scenario_ids": scenario_ids,
            "commit": commit,
            "content_hash": hash,
            "spec_status": spec_status,
        }))
    }
}
