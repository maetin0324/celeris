use std::{path::Path, process::Command};
use task_api::releases::{DeploymentContext, ReleasePromoteError};

pub(super) fn read(dir: &Path) -> Vec<serde_json::Value> {
    let mut records: Vec<_> = std::fs::read_dir(dir.join("audit"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| super::read_json(&e.path()))
        .collect();
    records.sort_by(|a, b| a["started_at"].as_str().cmp(&b["started_at"].as_str()));
    records
}

pub(super) fn command(audit: &DeploymentContext) -> Result<Command, ReleasePromoteError> {
    for (key, value) in [
        ("actor", &audit.actor),
        ("reason", &audit.reason),
        ("policy_ref", &audit.policy_ref),
    ] {
        if value.trim().is_empty() {
            return Err(ReleasePromoteError::Unavailable(format!(
                "deployment {key} is required"
            )));
        }
    }
    let mut command = Command::new("sh");
    command
        .env("SD_ACTOR", &audit.actor)
        .env("SD_REASON", &audit.reason)
        .env("SD_POLICY_REF", &audit.policy_ref)
        .env(
            "SD_TASK_REFS",
            serde_json::json!(audit.task_refs).to_string(),
        )
        .env(
            "SD_DECISION_REFS",
            serde_json::json!(audit.decision_refs).to_string(),
        )
        .env_remove("SD_OPERATION");
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_preserves_provenance_and_verify_snapshot() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("aaaaaaaaaaaa");
        std::fs::create_dir_all(dir.join("audit")).expect("mkdir");
        let record = serde_json::json!({
            "actor": "external_operator:test", "actor_kind": "external_operator", "actor_id": "test", "reason": "recover", "policy_ref": "approval:1",
            "task_refs": ["task:1"], "decision_refs": ["decision:1"],
            "operation": "rollback", "verify": {"ok": false, "checks": []},
            "status": "succeeded", "started_at": "2026-10-27T00:00:00Z"
        });
        std::fs::write(dir.join("audit/one.json"), record.to_string()).expect("write");
        std::fs::write(dir.join("verify.json"), r#"{"ok":true}"#).expect("write");
        let releases = super::super::scan(root.path(), None);
        assert_eq!(releases.items[0].deployment_audit, vec![record]);
        assert!(read(root.path()).is_empty());
    }

    #[test]
    fn script_environment_carries_explicit_context() {
        let audit = DeploymentContext {
            actor: "cos_general:general".into(),
            reason: "requested recovery".into(),
            policy_ref: "policy:1".into(),
            task_refs: vec!["task:1".into()],
            decision_refs: vec!["cos_operation:1".into()],
        };
        let output = command(&audit).expect("command").args(["-c", "printf '%s\\n' \"$SD_ACTOR\" \"$SD_REASON\" \"$SD_POLICY_REF\" \"$SD_TASK_REFS\" \"$SD_DECISION_REFS\""])
            .output().expect("run");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).expect("utf8"),
            "cos_general:general\nrequested recovery\npolicy:1\n[\"task:1\"]\n[\"cos_operation:1\"]\n"
        );
    }
}
