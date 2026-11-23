//! ADR 2026-11-23-project-observability-publish D3: publish manifest の状態（読むだけ）と、署名の前の内容の検査。
//!
//! - `GET /api/v1/tasks/{id}/publish/{manifest_id}?version=`: その版（省けば最新の版）の manifest を claims → evidence → 再現条件 →
//!   limitations・未解決 → 人の署名 の順で返す。検査の違反・根拠の無い claim・file の上書き・署名の状態は
//!   [`manifest_status`] が読むたびに導く（保存しない。D3.4）。
//! - 人の HTTP の回答で `sign` を選ぶ前に [`check_before_sign`] が manifest を検査する（422
//!   `publish_manifest_invalid`・409 `publish_manifest_stale`）。主体の拒否は `task_ops::decision` の共有の owner。
//! - Celeris は署名を受けて外へ何も送らない（D3.5）。この module は store と workspace を読むだけ。

use axum::http::StatusCode;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use task_core::Project;
use task_core::decision::{DecisionRow, DecisionStatus};
use task_core::local_machine::WorkspaceRoots;
use task_core::model::{ArtifactRef, TaskId};
use task_core::publish::{
    self, Manifest, ManifestContext, ManifestRule, SignoffRecord, SignoffState,
};
use task_core::store::SqliteStore;
use task_core::strategy::{RefStatus, Reference};

use crate::files::{artifact_bytes, produced_artifacts};
use crate::problem::{ApiProblem, store_problem};

/// 参照（`url` を除く）を引く関数。呼び出し側（`crate::decisions`）が戦略の文書と同じ
/// `cos::strategy_gate::Reader::lookup` を渡す（この module は循環の外に置き、`cos`・`state` を使わない）。
pub(crate) type Lookup<'a> = dyn FnMut(&Project, &Reference) -> Result<RefStatus, ApiProblem> + 'a;

/// 検査の違反 1 件（`task_core::publish::ManifestViolation` の写し）。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ManifestViolationView {
    pub rule: ManifestRule,
    pub code: String,
    pub path: String,
    pub detail: String,
}

/// 署名の状態（D3.3・D3.4。決定と file から導く）。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ManifestSignoffView {
    pub state: SignoffState,
    /// 署名の決定の key（`publish-signoff-v<n>`）。
    pub key: String,
    /// manifest を作った task の、この版の署名の決定の id（出ていなければ `None`）。
    pub decision_id: Option<String>,
    pub decision_status: Option<DecisionStatus>,
    /// 最後の回答の主体（`signed` のときは人）。
    pub answered_by: Option<String>,
    pub answered_at: Option<String>,
}

/// `GET /tasks/{id}/publish/{manifest_id}` の応答。欄の順は人が読む順（claims → evidence → 再現条件 →
/// limitations・未解決 → 人の署名）。manifest の各項目は文書の形のまま（`task_core::publish::Manifest`）。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ManifestView {
    pub task_id: String,
    pub project_id: Option<String>,
    pub manifest_id: String,
    pub version: u32,
    /// 同じ `manifest_id` の版（昇順）。
    pub versions: Vec<u32>,
    pub name: String,
    /// `ArtifactProduced` の sha256（dispatcher が計算した値）。
    pub recorded_sha256: String,
    /// file の今の byte 列の sha256（読めなければ `None`）。
    pub current_sha256: Option<String>,
    /// file の今の byte 列が記録と同じか。`false` の file は検査済みと出さない（違反に数える）。
    pub file_matches_record: bool,
    pub violations: Vec<ManifestViolationView>,
    /// 根拠の無い claim の id（証拠・内部の参照・再現条件の欠け）。
    pub unsupported_claims: Vec<String>,
    /// 概要の強い警告（`manifest_confidential`・`manifest_blocking_question`）。
    pub strong_warnings: Vec<String>,
    pub target: Option<Value>,
    pub claims: Vec<Value>,
    pub evidence: Vec<Value>,
    pub reproduction: Vec<Value>,
    pub limitations: Vec<Value>,
    pub open_questions: Vec<Value>,
    pub signoff: ManifestSignoffView,
}

pub(crate) const MANIFEST_NOT_FOUND: &str = "publish_manifest_not_found";
pub(crate) const MANIFEST_INVALID: &str = "publish_manifest_invalid";
pub(crate) const MANIFEST_STALE: &str = "publish_manifest_stale";

fn bare(sha: &str) -> &str {
    sha.strip_prefix("sha256:").unwrap_or(sha)
}

/// D3.2〜D3.4: manifest の検査と署名の状態。`version` が `None` なら最新の版。
pub(crate) fn manifest_status(
    store: &SqliteStore,
    roots: &WorkspaceRoots,
    task_id: TaskId,
    manifest_id: &str,
    version: Option<u32>,
    lookup: &mut Lookup<'_>,
) -> Result<ManifestView, ApiProblem> {
    let not_found = |detail: String| {
        ApiProblem::new(StatusCode::NOT_FOUND, MANIFEST_NOT_FOUND, detail)
            .with_extra("manifest_id", manifest_id)
    };
    let task = store
        .get(task_id)
        .map_err(store_problem)?
        .ok_or_else(|| ApiProblem::task_not_found(task_id))?;
    let project_id = task.project_id.map(|p| p.to_string());
    let produced = produced_artifacts(store, task.id)?;
    let manifests: Vec<(String, u32, &ArtifactRef)> = produced
        .iter()
        .filter_map(|(_, a)| publish::parse_artifact_name(&a.name).map(|(id, v)| (id, v, a)))
        .collect();
    let mut versions: Vec<u32> = manifests
        .iter()
        .filter(|(id, _, _)| id == manifest_id)
        .map(|(_, v, _)| *v)
        .collect();
    versions.sort_unstable();
    versions.dedup();
    let Some(version) = version.or_else(|| versions.last().copied()) else {
        return Err(not_found(format!(
            "task {task_id} has no publish manifest {manifest_id}"
        )));
    };
    let name = publish::artifact_name(manifest_id, version);
    // 同じ name の記録が複数あれば最後の記録を正とする（違う sha256 は idempotency の衝突として返す）。
    let Some(artifact) = produced
        .iter()
        .rev()
        .map(|(_, a)| a)
        .find(|a| a.name == name)
    else {
        return Err(not_found(format!("task {task_id} has no {name}")));
    };
    let bytes = artifact_bytes(store, roots, &task, &artifact.path);
    let current_sha256 = bytes.as_ref().map(|b| format!("{:x}", Sha256::digest(b)));
    let file_matches_record = current_sha256.as_deref() == Some(bare(&artifact.sha256));
    let mut violations = Vec::new();
    let mut unsupported_claims = Vec::new();
    let mut doc: Option<Manifest> = None;
    match &bytes {
        None => violations.push(ManifestViolationView {
            rule: ManifestRule::Shape,
            code: MANIFEST_INVALID.into(),
            path: String::new(),
            detail: "成果物の file が workspace に無い".into(),
        }),
        Some(bytes) => {
            if !file_matches_record {
                violations.push(ManifestViolationView {
                    rule: ManifestRule::Shape,
                    code: MANIFEST_STALE.into(),
                    path: String::new(),
                    detail: "成果物の file が記録の後に書き換えられた".into(),
                });
            }
            doc = serde_json::from_slice::<Manifest>(bytes).ok();
            let previous = match version.checked_sub(1).filter(|v| *v >= 1) {
                Some(prev) => {
                    let prev_name = publish::artifact_name(manifest_id, prev);
                    produced
                        .iter()
                        .rev()
                        .find(|(_, a)| a.name == prev_name)
                        .and_then(|(_, a)| artifact_bytes(store, roots, &task, &a.path))
                }
                None => None,
            };
            let references = match (&doc, project_id.as_deref()) {
                (Some(doc), Some(pid)) => {
                    let project = store
                        .project_get(
                            pid.parse()
                                .map_err(|_| ApiProblem::project_not_found(pid))?,
                        )
                        .map_err(store_problem)?;
                    match project {
                        Some(project) => resolve(lookup, &project, doc)?,
                        None => Vec::new(),
                    }
                }
                _ => Vec::new(),
            };
            let ctx = ManifestContext {
                project_id: project_id.clone().unwrap_or_default(),
                artifact_name: name.clone(),
                key_sha256s: produced
                    .iter()
                    .filter(|(_, a)| a.name == name)
                    .map(|(_, a)| a.sha256.clone())
                    .collect(),
                task_manifest_ids: manifests.iter().map(|(id, _, _)| id.clone()).collect(),
                previous,
                references,
            };
            let found = publish::validate_manifest(bytes, &ctx);
            unsupported_claims = publish::unsupported_claims(&found);
            violations.extend(found.into_iter().map(|v| ManifestViolationView {
                rule: v.rule,
                code: v.code.into(),
                path: v.path,
                detail: v.detail,
            }));
        }
    }
    let signoff = signoff_view(
        store,
        task_id,
        manifest_id,
        version,
        &artifact.sha256,
        current_sha256.as_deref(),
    )?;
    let items = |f: fn(&Manifest) -> Vec<Value>| doc.as_ref().map(f).unwrap_or_default();
    Ok(ManifestView {
        task_id: task_id.to_string(),
        project_id,
        manifest_id: manifest_id.to_string(),
        version,
        versions,
        name,
        recorded_sha256: bare(&artifact.sha256).to_string(),
        current_sha256,
        file_matches_record,
        unsupported_claims,
        strong_warnings: doc
            .as_ref()
            .map(|d| {
                publish::strong_warnings(d)
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        target: doc
            .as_ref()
            .and_then(|d| serde_json::to_value(&d.target).ok()),
        claims: items(|d| to_values(&d.claims)),
        evidence: items(|d| to_values(&d.evidence)),
        reproduction: items(|d| to_values(&d.reproduction)),
        limitations: items(|d| to_values(&d.limitations)),
        open_questions: items(|d| to_values(&d.open_questions)),
        violations,
        signoff,
    })
}

fn to_values<T: Serialize>(items: &[T]) -> Vec<Value> {
    items
        .iter()
        .filter_map(|i| serde_json::to_value(i).ok())
        .collect()
}

/// 文書の参照（`url` を除く）を strategy と同じ解決の関数で引く。
fn resolve(
    lookup: &mut Lookup<'_>,
    project: &Project,
    doc: &Manifest,
) -> Result<Vec<(Reference, RefStatus)>, ApiProblem> {
    let mut out: Vec<(Reference, RefStatus)> = Vec::new();
    for r in doc
        .evidence
        .iter()
        .flat_map(|e| &e.references)
        .take(task_core::strategy::REFS_TOTAL_MAX)
    {
        if matches!(r, Reference::Url { .. }) || out.iter().any(|(seen, _)| seen == r) {
            continue;
        }
        let status = lookup(project, r)?;
        out.push((r.clone(), status));
    }
    Ok(out)
}

/// manifest を作った task の、この版の署名の決定と、より新しい版の決定の有無から状態を導く。
fn signoff_view(
    store: &SqliteStore,
    task_id: TaskId,
    manifest_id: &str,
    version: u32,
    recorded_sha256: &str,
    current_sha256: Option<&str>,
) -> Result<ManifestSignoffView, ApiProblem> {
    let key = publish::signoff_decision_key(version);
    let rows: Vec<DecisionRow> = store
        .decisions_list(None)
        .map_err(store_problem)?
        .into_iter()
        .filter(|r| r.task_id == task_id && publish::is_signoff_key(&r.key))
        .collect();
    let newer = rows.iter().any(|r| {
        r.status != DecisionStatus::Withdrawn
            && publish::signoff_subject(&r.request.question)
                .is_some_and(|s| s.manifest_id == manifest_id && s.version > version)
    });
    // この版の key の決定のうち、問いの sha256 が記録と同じものを先に選ぶ（取り下げは後ろ）。
    let mut own: Vec<&DecisionRow> = rows.iter().filter(|r| r.key == key).collect();
    own.sort_by_key(|r| {
        (
            r.status == DecisionStatus::Withdrawn,
            publish::signoff_subject(&r.request.question)
                .is_none_or(|s| s.sha256 != bare(recorded_sha256)),
        )
    });
    let row = own.first().copied();
    // 決定の行の `answer` は最後の `DecisionAnswered` の写し（revise は上書き）。
    let answer = row
        .and_then(|r| r.request.answer.as_ref())
        .map(|a| (a.option.clone(), a.by.clone()));
    let record = row
        .filter(|r| r.status != DecisionStatus::Withdrawn)
        .map(|r| SignoffRecord {
            key: r.key.clone(),
            question: r.request.question.clone(),
            status: r.status,
            last_answer: answer.clone(),
        });
    Ok(ManifestSignoffView {
        state: publish::signoff_state(
            manifest_id,
            version,
            recorded_sha256,
            current_sha256,
            record.as_ref(),
            newer,
        ),
        key,
        decision_id: row.map(|r| r.id.clone()),
        decision_status: row.map(|r| r.status),
        answered_by: answer.map(|(_, by)| by),
        answered_at: row.and_then(|r| r.answered_at.clone()),
    })
}

/// D3.3: 人の HTTP の回答で `sign` を選ぶ前の内容の検査。署名の決定でない・`sign` でない・人でない主体
/// （`task_ops::decision` が 403 にする）は素通しする。違反は 422、file の今の sha256 が問いと違えば 409。
pub(crate) fn check_before_sign(
    store: &SqliteStore,
    roots: &WorkspaceRoots,
    decision_id: &str,
    option: Option<&str>,
    by: &str,
    lookup: &mut Lookup<'_>,
) -> Result<(), ApiProblem> {
    if option != Some(publish::SIGN_OPTION) || !publish::is_signoff_actor(by) {
        return Ok(());
    }
    let Some(row) = store.decision_get(decision_id).map_err(store_problem)? else {
        return Ok(());
    };
    if !publish::is_signoff_key(&row.key) {
        return Ok(());
    }
    let Some(subject) = publish::signoff_subject(&row.request.question) else {
        return Err(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            MANIFEST_INVALID,
            "署名の決定の問いに manifest の name と sha256 が 1 回ずつ無い",
        ));
    };
    let view = manifest_status(
        store,
        roots,
        row.task_id,
        &subject.manifest_id,
        Some(subject.version),
        lookup,
    )?;
    if view.current_sha256.as_deref() != Some(subject.sha256.as_str()) {
        return Err(ApiProblem::new(
            StatusCode::CONFLICT,
            MANIFEST_STALE,
            format!(
                "{} の今の byte 列は署名を問われた sha256:{} と違う",
                view.name, subject.sha256
            ),
        )
        .with_extra("current_sha256", view.current_sha256));
    }
    if !view.violations.is_empty() {
        let codes: Vec<String> = view.violations.iter().map(|v| v.code.clone()).collect();
        return Err(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            MANIFEST_INVALID,
            format!("{} は検査に通らない（{} 件の違反）", view.name, codes.len()),
        )
        .with_extra("violations", view.violations));
    }
    Ok(())
}
