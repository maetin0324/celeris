//! ADR 2026-11-22-strategy-review-critic D3.3・D3.5: final review の暗黙の条件 `strategy_critic`。
//!
//! 対象（`task_core::strategy_critic::critic_subjects`）がある task の final review だけに足す。合否は reviewer の同じ run が
//! `review.json` に書いた `critic` の節だけで決まり、ここは形を確かめるだけで LLM を呼ばない。節が無い・壊れている・対象や
//! 候補が文書と食い違う・理由が空か長すぎる・どれか 1 つでも `ok: false`・対象が衝突している・文書の file が記録の後に
//! 書き換えられた、のどれでも不合格（fail closed）。文書の検査は gate の仕事で、ここでは `candidates[].id` だけを読む。

use std::collections::BTreeSet;
use std::path::Path;

use task_core::strategy_critic::{CriticSubject, bare_sha256};
use task_worker::protocol::{
    CriticCheckOut, ReviewOutput, StrategyCriticRequest, StrategyCriticSubject,
};

use super::Verdict;

/// `ReviewCheckResult.kind` に出す暗黙の条件の名前。
pub(crate) const CRITIC_KIND: &str = "strategy_critic";
/// D3.5: `reason` の上限の文字数。
const REASON_MAX_CHARS: usize = 2000;

/// 1 回の final review の Critic（暗黙の条件の番号と対象）。
pub(crate) struct Critic {
    pub(crate) idx: usize,
    subjects: Vec<CriticSubject>,
}

impl Critic {
    /// 対象が空なら `None`（Critic と無関係の task。review は従来と同じ）。
    pub(crate) fn new(idx: usize, subjects: Vec<CriticSubject>) -> Option<Self> {
        (!subjects.is_empty()).then_some(Self { idx, subjects })
    }

    /// reviewer への入力（`ReviewRequest.strategy_critic`）。
    pub(crate) fn request(&self) -> StrategyCriticRequest {
        StrategyCriticRequest {
            criterion_idx: self.idx,
            subjects: self
                .subjects
                .iter()
                .map(|s| StrategyCriticSubject {
                    name: s.name.clone(),
                    path: s.path.clone(),
                    sha256: s.sha256.clone(),
                    strategy_id: s.strategy_id.clone(),
                    version: s.version,
                })
                .collect(),
            rubric: StrategyCriticRequest::fixed_rubric(),
        }
    }

    /// 判定されなかった（reviewer run が無い・決定的な条件が落ちた・run が判定できなかった）ときの不合格。
    pub(crate) fn fail(&self, reason: &str) -> Verdict {
        Verdict {
            criterion_idx: self.idx,
            pass: false,
            reason: format!("{CRITIC_KIND}: {reason}"),
            repair_hint: None,
        }
    }

    /// reviewer の `review.json` から合否を決める。
    pub(crate) fn judge(&self, text: &str, workspace_dir: &Path, tag: &str) -> Verdict {
        let problems = self.problems(text, workspace_dir);
        let (pass, detail) = if problems.is_empty() {
            (true, format!("ok ({} subject(s))", self.subjects.len()))
        } else {
            (false, problems.join("; "))
        };
        Verdict {
            criterion_idx: self.idx,
            pass,
            reason: format!("{tag}: {CRITIC_KIND}: {detail}"),
            repair_hint: None,
        }
    }

    fn problems(&self, text: &str, workspace_dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = self
            .subjects
            .iter()
            .filter(|s| s.conflict)
            .map(|s| format!("{} has two sha256 (conflict)", s.name))
            .collect();
        let Some(critic) = serde_json::from_str::<ReviewOutput>(text)
            .ok()
            .and_then(|o| o.critic)
        else {
            out.push("critic section missing or invalid in review.json".to_string());
            return out;
        };
        let expected: BTreeSet<(&str, &str)> = self
            .subjects
            .iter()
            .map(|s| (s.name.as_str(), bare_sha256(&s.sha256)))
            .collect();
        let given: BTreeSet<(&str, &str)> = critic
            .subjects
            .iter()
            .map(|v| (v.name.as_str(), bare_sha256(&v.sha256)))
            .collect();
        if expected != given || given.len() != critic.subjects.len() {
            out.push(format!(
                "critic subjects {given:?} do not match the strategy artifacts {expected:?}"
            ));
        }
        for verdict in &critic.subjects {
            let Some(subject) = self.subjects.iter().find(|s| s.name == verdict.name) else {
                continue;
            };
            match document_candidates(workspace_dir, subject) {
                Ok(ids) => {
                    let got: Vec<&str> = verdict.candidates.iter().map(|c| c.id.as_str()).collect();
                    let got_set: BTreeSet<&str> = got.iter().copied().collect();
                    let want: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
                    if got_set.len() != got.len() || got_set != want {
                        out.push(format!(
                            "{}: critic candidates {got:?} do not match the document {want:?}",
                            subject.name
                        ));
                    }
                }
                Err(e) => out.push(format!("{}: {e}", subject.name)),
            }
            let mut checks: Vec<(String, &CriticCheckOut)> = vec![
                (format!("{}.comparison", verdict.name), &verdict.comparison),
                (format!("{}.revisit", verdict.name), &verdict.revisit),
            ];
            for c in &verdict.candidates {
                checks.push((
                    format!("{}.{}.falsification", verdict.name, c.id),
                    &c.falsification,
                ));
                checks.push((
                    format!("{}.{}.experiment", verdict.name, c.id),
                    &c.experiment,
                ));
            }
            for (label, check) in checks {
                if check.reason.trim().is_empty() || check.reason.chars().count() > REASON_MAX_CHARS
                {
                    out.push(format!(
                        "{label}: reason empty or longer than {REASON_MAX_CHARS} chars"
                    ));
                } else if !check.ok {
                    out.push(format!("{label} ok=false: {}", check.reason));
                }
            }
        }
        out
    }
}

/// 文書の `candidates[].id`。file が workspace の外・読めない・記録の sha256 と違う・形が違うなら `Err`。
fn document_candidates(
    workspace_dir: &Path,
    subject: &CriticSubject,
) -> Result<Vec<String>, String> {
    let artifact =
        task_worker::artifact::resolve(workspace_dir, &subject.name, &subject.path, None)
            .map_err(|e| format!("document unreadable at {}: {e}", subject.path))?;
    if bare_sha256(&artifact.sha256) != bare_sha256(&subject.sha256) {
        return Err(format!(
            "document at {} changed after it was recorded",
            subject.path
        ));
    }
    let bytes = std::fs::read(workspace_dir.join(&artifact.path))
        .map_err(|e| format!("document unreadable at {}: {e}", subject.path))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("document at {} is not JSON: {e}", subject.path))?;
    value
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter()
                .map(|c| c.get("id").and_then(|id| id.as_str()).map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| format!("document at {} has no candidates[].id", subject.path))
}

#[cfg(test)]
mod tests;
