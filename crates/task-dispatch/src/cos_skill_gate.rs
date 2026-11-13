//! ADR 2026-11-11-cos-skill-sync D3: may a CoS chat run start with the skills the KB holds?
//!
//! Before a CoS run is claimed the dispatcher asks [`SkillGate::ready`], which runs the pure
//! [`task_ops::cos_skill_gate::check`]. On drift no run is claimed and one operator notice is
//! recorded per distinct drift (its source key is durable, so a restart does not repeat it).
//! Deterministic file reads only, no judgment.

use std::path::Path;

use task_core::feed::{NoticeEvent, NoticeKind, NoticeStore};
use task_ops::cos_skill_gate::{SkillDrift, check};
use time::OffsetDateTime;

/// The CoS gate of one dispatcher: remembers the drift last noticed so a held tick does not
/// write the same notice again.
#[derive(Debug, Default)]
pub(crate) struct SkillGate {
    noticed: Option<String>,
}

impl SkillGate {
    /// `true` when CoS runs may start. `kb_root` is `[knowledge] root`.
    pub(crate) fn ready(
        &mut self,
        store: &dyn NoticeStore,
        kb_root: &Path,
        (policy_skill, policy_version): (&str, &str),
        now: OffsetDateTime,
    ) -> bool {
        let root = kb_root.join(task_core::knowledge::SKILLS_DIR);
        let drift = match check(&root, policy_skill, policy_version) {
            Ok(()) => {
                self.noticed = None;
                return true;
            }
            Err(drift) => drift,
        };
        let key = drift.key();
        if self.noticed.as_deref() != Some(key.as_str()) {
            record_notice(store, &drift, now);
            self.noticed = Some(key);
        }
        false
    }
}

fn record_notice(store: &dyn NoticeStore, drift: &SkillDrift, now: OffsetDateTime) {
    let detail = drift.describe();
    tracing::warn!(%detail, "CoS runs held: the KB's CoS skills do not fit [cos.triage]");
    let summary = format!(
        "{detail}。運用者の作業: repo の skill を KB に写す配備（promote.sh の sync-skills 段、または \
         scripts/selfdeploy/sync-skills.sh）を行うか、[cos.triage] policy_version を skill の版に合わせて reload する。\
         合った次の tick から CoS の run が起きる。"
    );
    let event = NoticeEvent {
        source_key: format!("cos:skill-drift:{}", drift.key()),
        kind: NoticeKind::BadNews,
        group_key: "bad_news:cos:skill_drift".to_owned(),
        title: "CoS の skill が config と合わないので、CoS の run を止めています".to_owned(),
        summary,
        project_id: None,
        task_id: None,
        target: None,
        links: Vec::new(),
        at: now,
    };
    if let Err(error) = store.notice_record(&event) {
        tracing::error!(%error, "failed to record the CoS skill drift notice");
    }
}

#[cfg(test)]
mod tests {
    use task_ops::cos_skill_gate::CORE_SKILL_FILES;
    use task_worker::protocol::CosChatContext;

    /// The gate's list of the files the Core names (`CORE_SKILL_FILES`) is exactly the `.md`
    /// files the Core text names, so a new reference in the Core cannot go unchecked before a CoS
    /// run. Not covered elsewhere: the gate's own tests use the list, not the Core text. Lives in
    /// task-dispatch, the one crate that sees both the Core (task-worker) and the list (task-ops).
    #[test]
    fn core_skill_files_follow_the_core_text() {
        let core = task_worker::cos_chat::core(&CosChatContext::default());
        let named: std::collections::BTreeSet<&str> = core
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'))
            .filter(|w| w.ends_with(".md"))
            .collect();
        let listed: std::collections::BTreeSet<&str> = CORE_SKILL_FILES
            .iter()
            .map(|(_, f)| *f)
            .filter(|f| *f != "SKILL.md")
            .collect();
        assert_eq!(named, listed);
    }
}
