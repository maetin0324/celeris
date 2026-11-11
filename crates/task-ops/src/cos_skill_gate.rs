//! ADR 2026-11-11-cos-skill-sync D3: may a CoS chat run start with the skills the KB holds?
//!
//! The repo (`config/skills/`) is the source of the CoS skills and the KB's `skills/` is where a
//! promotion deploys them (`scripts/selfdeploy/sync-skills.sh`). Before a CoS run is claimed the
//! dispatcher asks [`SkillGate::ready`]: the triage skill's `metadata.version` must equal
//! `[cos.triage] policy_version`, and every file the Core names ([`CORE_SKILL_FILES`]) or a
//! mounted `SKILL.md` links to (`](name.md)` next to it) must exist. On drift no run is claimed and
//! one operator notice is recorded per distinct drift (its source key is durable, so a restart does
//! not repeat it). Deterministic file reads only, no judgment.

use std::collections::BTreeSet;
use std::path::Path;

use task_core::feed::{NoticeEvent, NoticeKind, NoticeStore};
use time::OffsetDateTime;

/// The CoS gate of one dispatcher: remembers the drift last noticed so a held tick does not
/// write the same notice again.
#[derive(Debug, Default)]
pub struct SkillGate {
    noticed: Option<String>,
}

impl SkillGate {
    /// `true` when CoS runs may start. `kb_root` is `[knowledge] root`.
    pub fn ready(
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

/// The CoS skills every CoS run may mount (`cos-operator` always; the triage skill is
/// `[cos.triage] policy_skill`).
pub const OPERATOR_SKILL: &str = "cos-operator";

/// The skill files the Core (`task_worker::cos_chat::core`) tells the CoS to read, `(skill, file)`.
/// `task-dispatch`'s `cos_skill_gate` test pins this list to the Core text.
pub const CORE_SKILL_FILES: &[(&str, &str)] = &[
    (OPERATOR_SKILL, "SKILL.md"),
    (OPERATOR_SKILL, "consult.md"),
    (OPERATOR_SKILL, "operations.md"),
    (OPERATOR_SKILL, "deploy.md"),
];

/// Why the KB's CoS skills do not fit this daemon's config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDrift {
    pub policy_skill: String,
    /// `metadata.version` of the triage skill (`None`: no `SKILL.md` or no version in it).
    pub skill_version: Option<String>,
    pub policy_version: String,
    /// `<skill>/<file>` paths under the KB's `skills/` that are missing, sorted.
    pub missing: Vec<String>,
}

impl SkillDrift {
    /// One string per distinct drift: the notice is recorded once per key.
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.policy_skill,
            self.skill_version.as_deref().unwrap_or("-"),
            self.policy_version,
            self.missing.join(",")
        )
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.skill_version.as_deref() != Some(self.policy_version.as_str()) {
            parts.push(format!(
                "KB の skill {} の版は {}、config の [cos.triage] policy_version は {}",
                self.policy_skill,
                self.skill_version.as_deref().unwrap_or("（無し）"),
                self.policy_version
            ));
        }
        if !self.missing.is_empty() {
            parts.push(format!(
                "KB の skills/ に無い file: {}",
                self.missing.join(", ")
            ));
        }
        parts.join("。")
    }
}

/// See the module doc. `skills_root` is the KB's `skills/` directory.
pub fn check(
    skills_root: &Path,
    policy_skill: &str,
    policy_version: &str,
) -> Result<(), SkillDrift> {
    let mut missing = BTreeSet::new();
    for (skill, file) in CORE_SKILL_FILES {
        if !skills_root.join(skill).join(file).is_file() {
            missing.insert(format!("{skill}/{file}"));
        }
    }
    let mut skill_version = None;
    for skill in [OPERATOR_SKILL, policy_skill] {
        let Ok(text) = std::fs::read_to_string(skills_root.join(skill).join("SKILL.md")) else {
            missing.insert(format!("{skill}/SKILL.md"));
            continue;
        };
        for file in sibling_links(&text) {
            if !skills_root.join(skill).join(&file).is_file() {
                missing.insert(format!("{skill}/{file}"));
            }
        }
        if skill == policy_skill {
            skill_version = metadata_version(&text);
        }
    }
    if missing.is_empty() && skill_version.as_deref() == Some(policy_version) {
        return Ok(());
    }
    Err(SkillDrift {
        policy_skill: policy_skill.to_owned(),
        skill_version,
        policy_version: policy_version.to_owned(),
        missing: missing.into_iter().collect(),
    })
}

/// `metadata.version` of a `SKILL.md` front matter (quotes stripped).
pub fn metadata_version(skill_md: &str) -> Option<String> {
    let mut lines = skill_md.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut in_metadata = false;
    for line in lines {
        if line.trim() == "---" {
            return None;
        }
        if !line.starts_with([' ', '\t']) {
            in_metadata = line.trim_end() == "metadata:";
            continue;
        }
        if let Some(value) = in_metadata
            .then(|| line.trim().strip_prefix("version:"))
            .flatten()
        {
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            return (!value.is_empty()).then(|| value.to_owned());
        }
    }
    None
}

/// Markdown links to a file next to `SKILL.md` (`](name.md)`, no directory, no URL, no anchor).
fn sibling_links(skill_md: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for piece in skill_md.split("](").skip(1) {
        let Some(target) = piece.split(')').next() else {
            continue;
        };
        let target = target.split('#').next().unwrap_or_default();
        if target.ends_with(".md") && !target.contains(['/', ':', ' ']) {
            out.insert(target.to_owned());
        }
    }
    out
}

/// A KB `skills/` tree that passes [`check`] for `policy_skill`/`policy_version` (stub bodies).
/// For tests of other crates that start CoS runs; production KBs get the repo's skills.
#[doc(hidden)]
pub fn write_fixture(
    skills_root: &Path,
    policy_skill: &str,
    policy_version: &str,
) -> std::io::Result<()> {
    for (skill, file) in CORE_SKILL_FILES {
        let dir = skills_root.join(skill);
        std::fs::create_dir_all(&dir)?;
        if *file != "SKILL.md" {
            std::fs::write(dir.join(file), format!("# {file}\n"))?;
        }
    }
    std::fs::write(
        skills_root.join(OPERATOR_SKILL).join("SKILL.md"),
        format!("---\nname: {OPERATOR_SKILL}\ndescription: test skill\n---\n# {OPERATOR_SKILL}\n"),
    )?;
    let dir = skills_root.join(policy_skill);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {policy_skill}\ndescription: test skill\nmetadata:\n  version: \"{policy_version}\"\n---\n# {policy_skill}\n"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_skills() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/skills")
    }

    /// The repo's skills pass the gate under the config default, so a promotion that syncs them
    /// opens the gate. Not covered elsewhere: nothing else reads the repo skill tree as the KB.
    #[test]
    fn repo_skills_pass_under_the_default_policy_version() {
        let version = metadata_version(
            &std::fs::read_to_string(repo_skills().join("cos-inbox-triage/SKILL.md"))
                .expect("triage skill"),
        )
        .expect("version");
        assert_eq!(check(&repo_skills(), "cos-inbox-triage", &version), Ok(()));
    }

    #[test]
    fn version_mismatch_and_missing_files_are_drift() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture(dir.path(), "cos-inbox-triage", "10").expect("fixture");
        let drift = check(dir.path(), "cos-inbox-triage", "11").expect_err("drift");
        assert_eq!(drift.skill_version.as_deref(), Some("10"));
        assert!(drift.missing.is_empty());
        assert_eq!(check(dir.path(), "cos-inbox-triage", "10"), Ok(()));

        std::fs::remove_file(dir.path().join("cos-operator/consult.md")).expect("rm");
        std::fs::write(
            dir.path().join("cos-operator/SKILL.md"),
            "---\nname: cos-operator\n---\nsee [x](project-controller.md) and [adr](../a/b.md)\n",
        )
        .expect("skill");
        let drift = check(dir.path(), "cos-inbox-triage", "10").expect_err("missing");
        assert_eq!(
            drift.missing,
            vec![
                "cos-operator/consult.md".to_owned(),
                "cos-operator/project-controller.md".to_owned()
            ]
        );
        assert_ne!(drift.key(), "");
    }

    #[test]
    fn metadata_version_reads_only_the_metadata_block() {
        assert_eq!(
            metadata_version("---\nname: x\nmetadata:\n  author: c\n  version: \"11\"\n---\n"),
            Some("11".into())
        );
        assert_eq!(metadata_version("---\nversion: 3\n---\n"), None);
        assert_eq!(metadata_version("# no front matter\n"), None);
    }
}
