//! ADR 2026-11-08-health-enforcement-loop D6.6・D6.7: 全体棚卸しの 1 repo の評価から、debt の draft task を起票する。
//!
//! - 起票の対象は [`HealthEvidence::blocking`] の fingerprint だけ。fail 級かどうか（severity・delta・exception・
//!   候補）の正本は base 側の検査器で、gate の judge が `blocking` に入れている。ここでは判定し直さない。
//!   [`HealthBlockingDetail`] は題と objective の文面（path・rule_id・message）に使うだけで、`blocking` に無い
//!   fingerprint の detail は読まない。
//! - verdict が [`HealthVerdict::Fail`] のときだけ起票する（Pass・PolicyOnly・Error・NotApplicable は起票しない）。
//! - 冪等キーは `health-debt:<repo_id>:<fingerprint>:<head_sha>`（ADR 2026-10-27-task-idempotency-keys）。同じ repo と
//!   fingerprint のキーを持つ終端でない task があれば起票しない。debt の task が終端になった後に同じ fingerprint が
//!   別の head で blocking に出れば、新しいキーで起票する。
//! - task は draft（人か CoS の承認で始まる）。1 件の finding に 1 件で、objective は その finding の path に限る。
//!   計画・子 task・複数の debt のまとめは作らない。作成は `POST /tasks` と同じ組み立て
//!   （[`crate::add::build_task_with_roles`]）と、冪等キーを同じ transaction で書く `SqliteStore::create_task_with`。
use std::cell::Cell;

use sha2::{Digest, Sha256};
use task_core::required_checks::health::{HealthBlockingDetail, HealthEvidence, HealthVerdict};
use task_core::store::{IdempotencyClaim, IdempotencyLookup};
use task_core::{
    GenreSpec, ProjectId, RoleSpec, SqliteStore, Status, StoreError, TaskId, TaskStore,
};
use time::OffsetDateTime;

use crate::add::{CriterionSpec, NewTaskSpec, SpecProvenance, build_task_with_roles};
use crate::error::OpsError;

/// 冪等キーの接頭辞（D6.6）。
pub const DEBT_KEY_PREFIX: &str = "health-debt:";
/// 冪等キーの行に残す操作名。
pub const DEBT_OPERATION: &str = "health_inventory.file_debt";
/// 起票した task に付ける label。
pub const DEBT_LABEL: &str = "health-debt";
/// 1 回の発火で起票する上限（D6.6）。
pub const MAX_DEBTS_PER_FIRE: usize = 3;
/// objective に写す検査器の message の字数の上限。
const MAX_MESSAGE_CHARS: usize = 500;
/// 題に写す path の字数の上限。
const MAX_TITLE_PATH_CHARS: usize = 120;

/// 1 repo の評価の起票の入力。repo id は `evidence.repo_id` だけを正本にする。
#[derive(Debug, Clone, Copy)]
pub struct HealthDebtInput<'a> {
    pub project_id: ProjectId,
    /// その repo の評価の証跡（同じ検査器の実行の結果）。
    pub evidence: &'a HealthEvidence,
    /// 同じ検査器の実行から得た blocking の finding の説明。
    pub details: &'a [HealthBlockingDetail],
    /// この発火で残っている起票の枠（[`MAX_DEBTS_PER_FIRE`] を超える値は切り詰める）。
    pub remaining: usize,
}

/// 起票の結果（D6.5 の記録の `task_ids`・`existing_skipped`・`limit_remaining` に写す）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FiledHealthDebts {
    /// 起票した draft task（`blocking` の順）。
    pub task_ids: Vec<TaskId>,
    /// 同じ repo と fingerprint の終端でない task があった、または同じキーが既にあったので省いた数。
    pub existing_skipped: u64,
    /// 枠が尽きて残した数。
    pub limit_remaining: u64,
    /// `blocking` にあるのに detail が無い（または detail に path が無い）ので起票しなかった数。
    pub missing_detail: u64,
}

/// debt の冪等キー `health-debt:<repo_id>:<fingerprint>:<head_sha>`。
pub fn debt_key(repo_id: &str, fingerprint: &str, head_sha: &str) -> String {
    format!("{}{head_sha}", debt_key_stem(repo_id, fingerprint))
}

/// 同じ repo と fingerprint のキーの共通部分（head_sha の前まで）。
fn debt_key_stem(repo_id: &str, fingerprint: &str) -> String {
    format!("{DEBT_KEY_PREFIX}{repo_id}:{fingerprint}:")
}

/// 1 repo の評価から debt の draft task を起票する（D6.6・D6.7）。
pub fn file_health_debts(
    store: &SqliteStore,
    input: &HealthDebtInput<'_>,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<FiledHealthDebts, OpsError> {
    let mut filed = FiledHealthDebts::default();
    let evidence = input.evidence;
    if evidence.verdict != HealthVerdict::Fail {
        return Ok(filed);
    }
    let repo_id = evidence.repo_id.to_string();
    let repo_name = store
        .repo_list(input.project_id)?
        .into_iter()
        .find(|r| r.id == evidence.repo_id)
        .map(|r| r.name)
        .ok_or_else(|| {
            OpsError::Validation(format!(
                "repo {repo_id} is not a repository of project {}",
                input.project_id
            ))
        })?;
    let budget = input.remaining.min(MAX_DEBTS_PER_FIRE);
    let mut seen: Vec<&str> = Vec::new();
    for fingerprint in &evidence.blocking {
        if seen.contains(&fingerprint.as_str()) {
            continue;
        }
        seen.push(fingerprint);
        if has_open_debt(store, &repo_id, fingerprint)? {
            filed.existing_skipped += 1;
            continue;
        }
        let Some(detail) = input
            .details
            .iter()
            .find(|d| &d.fingerprint == fingerprint && !d.paths.is_empty())
        else {
            filed.missing_detail += 1;
            continue;
        };
        if filed.task_ids.len() >= budget {
            filed.limit_remaining += 1;
            continue;
        }
        let key = debt_key(&repo_id, fingerprint, &evidence.head_sha);
        let spec = debt_spec(input.project_id, &repo_name, evidence, detail)?;
        match create_debt(store, spec, key, roles, genres, now)? {
            Some(id) => filed.task_ids.push(id),
            None => filed.existing_skipped += 1,
        }
    }
    Ok(filed)
}

/// 同じ repo と fingerprint のキーを持つ終端でない task があるか。fingerprint が別の fingerprint の接頭辞に
/// なる場合に備え、共通部分の後ろ（head_sha）に `:` を含むキーは別の fingerprint として数えない。
fn has_open_debt(store: &SqliteStore, repo_id: &str, fingerprint: &str) -> Result<bool, OpsError> {
    let stem = debt_key_stem(repo_id, fingerprint);
    for (key, task_id, _) in store.idempotency_keys_with_prefix(&stem)? {
        let Some(head) = key.strip_prefix(&stem) else {
            continue;
        };
        if head.contains(':') {
            continue;
        }
        let Ok(id) = task_id.parse::<TaskId>() else {
            continue;
        };
        if store.get(id)?.is_some_and(|t| !t.status.is_terminal()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 1 件の finding の draft task の spec。objective は detail の path に限る。
fn debt_spec(
    project_id: ProjectId,
    repo_name: &str,
    evidence: &HealthEvidence,
    detail: &HealthBlockingDetail,
) -> Result<NewTaskSpec, OpsError> {
    let first_path: String = detail.paths[0].chars().take(MAX_TITLE_PATH_CHARS).collect();
    let title = format!("[health debt] {}: {first_path}", detail.rule_id);
    let paths = detail
        .paths
        .iter()
        .map(|p| format!("- {p}"))
        .collect::<Vec<_>>()
        .join("\n");
    let message: String = detail.message.chars().take(MAX_MESSAGE_CHARS).collect();
    let message = if message.is_empty() {
        "（無し）".to_owned()
    } else {
        message
    };
    let objective = format!(
        "repo health の全体棚卸し（ADR 2026-11-08-health-enforcement-loop D6）が、repo {repo_name} の \
         {base}..{head} で fail 級の finding を 1 件見つけた。この 1 件を直す。\n\
         - fingerprint: {fp}\n- rule: {rule}\n- 検査器の message: {message}\n\
         変更してよい path（finding の evidence の path）:\n{paths}\n\
         上の path の外を変えない。計画・子 task・ほかの debt をまとめた書き換えを作らない。",
        base = evidence.base_sha,
        head = evidence.head_sha,
        fp = detail.fingerprint,
        rule = detail.rule_id,
    );
    let acceptance = vec![CriterionSpec::Reviewer {
        text: format!(
            "repo health の検査器の report に fingerprint {} の fail 級の finding が無く、変更が objective の path に限られている",
            detail.fingerprint
        ),
    }];
    let body = serde_json::json!({
        "title": title,
        "objective": objective,
        "acceptance": acceptance,
        "project_id": project_id,
        "repos": [repo_name],
        "labels": [DEBT_LABEL],
    });
    let mut spec: NewTaskSpec = serde_json::from_value(body)
        .map_err(|e| OpsError::Validation(format!("health debt spec: {e}")))?;
    spec.status = Some(Status::Draft);
    spec.provenance = SpecProvenance::system();
    Ok(spec)
}

/// `POST /tasks` と同じ組み立てで task を作り、冪等キーを同じ transaction で書く。キーが既にあれば作らず `None`。
fn create_debt(
    store: &SqliteStore,
    spec: NewTaskSpec,
    key: String,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<Option<TaskId>, OpsError> {
    let body = serde_json::to_string(&spec)
        .map_err(|e| OpsError::Validation(format!("health debt spec: {e}")))?;
    let claim = IdempotencyClaim {
        key,
        operation: DEBT_OPERATION.to_owned(),
        request_hash: format!("{:x}", Sha256::digest(body.as_bytes())),
    };
    if store.idempotency_lookup(&claim)? != IdempotencyLookup::New {
        return Ok(None);
    }
    let task = build_task_with_roles(store, spec, roles, genres, now)?;
    let taken = Cell::new(false);
    let created = store.create_task_with(&task, task_core::DAEMON_ACTOR, vec![], |tx| {
        if SqliteStore::idempotency_lookup_tx(tx, &claim)? != IdempotencyLookup::New {
            taken.set(true);
            return Err(StoreError::Invalid(format!(
                "idempotency key {} is taken",
                claim.key
            )));
        }
        SqliteStore::idempotency_record_tx(tx, &claim, &task.id.to_string(), None, &now.to_string())
    });
    match created {
        Ok(()) => Ok(Some(task.id)),
        Err(_) if taken.get() => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests;
