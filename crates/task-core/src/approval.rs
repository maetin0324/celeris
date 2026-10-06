//! 認可（ADR-0033 D5。Phase 26）。SPEC §3.6「少しでも聞くべきだとエージェントが判断したら、あなたに
//! 指示を仰ぐ。あなたはそれに対して『今回だけ』か『同じようなことは今後ずっと』のどちらかの認可を出す。
//! 永続の認可は文字で記録してエージェントに注入する」。
//!
//! - 既存の `Question` 終端（ADR-0010: `Status::Blocked` と `answers[]`）はそのまま残る。ここは、その終端が
//!   起きたときに `approvals` の行を 1 件**追記するだけ**（人が読む・答えるための窓口）。
//! - 人が `once` / `standing` / `denied` のどれで答えても、最終的には**同じ `answers[]` の経路**
//!   （`task_ops::gate::answer`）でタスクを再開する。二重の実装はしない（`task-ops/src/approval.rs` 参照）。
//! - `standing` のときだけ `standing_rules` に 1 行増え、以後の run の前置きに常に注入される
//!   （`crate::approval::StandingRuleStore` を読むのは `task-dispatch`、前置きに描くのは `task-worker::preamble`）。
//! - 自動判定・自動化は今回はやらない（ADR-0033 D5「規則がまだ無い」）。
//!
//! `store.rs` は `TaskStore` の supertrait として `ApprovalStore` を要求するだけで、実装（SQL）はここにある
//! （`report.rs` と同じ形）。

use rusqlite::types::Value as SqlValue;
use rusqlite::{OptionalExtension, params, params_from_iter};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ulid::Ulid;

use crate::model::{Event, Status, TaskId};
use crate::org::ProjectId;
use crate::store::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};

/// 認可 1 件の識別子（ULID）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct ApprovalId(#[schemars(with = "String")] pub Ulid);

impl ApprovalId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl Default for ApprovalId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ApprovalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for ApprovalId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Ulid::from_string(s)?))
    }
}

/// 永続の規則の識別子（ULID）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct StandingRuleId(#[schemars(with = "String")] pub Ulid);

impl StandingRuleId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl Default for StandingRuleId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for StandingRuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for StandingRuleId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Ulid::from_string(s)?))
    }
}

/// 人の決定（SPEC §3.6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// 今回だけ。
    Once,
    /// 同じようなことは今後ずっと（`standing_rules` に 1 行増える）。
    Standing,
    /// 認めない。
    Denied,
    /// 取り下げ（Phase F7。**celeris だけが書く**。人は `POST /approvals/{id}/decide` で選べない）:
    /// 認可元のタスクが終端（`done` / `failed` / `cancelled`）になったので、答える相手がいなくなった。
    /// 人の「認めない」（`Denied`）とは区別する（部をまたぐ委譲の `cross_authorization` は `Withdrawn` を
    /// 「まだ決まっていない」と読む。やり直した run がもう一度聞けるように）。
    Withdrawn,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Once => "once",
            Decision::Standing => "standing",
            Decision::Denied => "denied",
            Decision::Withdrawn => "withdrawn",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "once" => Some(Decision::Once),
            "standing" => Some(Decision::Standing),
            "denied" => Some(Decision::Denied),
            "withdrawn" => Some(Decision::Withdrawn),
            _ => None,
        }
    }
}

/// 1 件の認可の要求（`approvals` テーブル。ADR-0033 D5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Approval {
    pub id: ApprovalId,
    /// 案件（案件に紐づかない質問なら `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
    /// 聞いてきた組織のノード（`task.assignee`、無ければ秘書）。
    pub node_id: String,
    /// きっかけになったタスク（無いことは今回は無いが、`approval_decide` の後も残す前提で任意にしてある）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub created_at: OffsetDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    #[schemars(with = "Option<String>")]
    pub decided_at: Option<OffsetDateTime>,
}

impl Approval {
    /// まだ人が答えていない。
    pub fn is_pending(&self) -> bool {
        self.decision.is_none()
    }
}

/// 永続の認可 1 行（`standing_rules` テーブル。ADR-0033 D5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StandingRule {
    pub id: StandingRuleId,
    /// `None` = 全員（どのノードの run にも注入される）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub rule: String,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub created_at: OffsetDateTime,
}

// ---- `approvals` / `standing_rules` 表の読み書き（SQL はここだけ。`store.rs` は supertrait で要求するだけ）----

/// ADR-0033 D5: `approvals` と `standing_rules` の読み書き。`TaskStore` の supertrait で、実装は
/// `SqliteStore` のみ（ディスパッチャは `Arc<dyn TaskStore>` から呼ぶ）。
pub trait ApprovalStore: Send + Sync {
    /// 1 件追記する（`Question` 終端のたび。ADR-0010 の既存の `answers[]` の仕組みには触れない）。
    fn approval_append(&self, approval: &Approval) -> Result<(), StoreError>;
    fn approval_get(&self, id: ApprovalId) -> Result<Option<Approval>, StoreError>;
    /// 古い順（`created_at` 昇順、同値は `id` 昇順。答える順に並ぶキューとして扱う）。
    ///
    /// `pending`: `Some(true)` = 未決定だけ（`decision IS NULL`）、`Some(false)` = **決定済みだけ**
    /// （`decision IS NOT NULL`。人が決めたものの履歴）、`None` = 絞り込み無し。
    /// GUI からの依頼 R5（Phase 27）で三値にした（以前は `bool` で、`false` が「絞り込み無し」だった）。
    fn approval_list(
        &self,
        pending: Option<bool>,
        project_id: Option<ProjectId>,
        node_id: Option<&str>,
    ) -> Result<Vec<Approval>, StoreError>;
    /// `decision` / `answer` / `decided_at` を書く（既に決まっていても上書きする。人が答え直せるように）。
    /// 無い id は `Ok(None)`。
    fn approval_decide(
        &self,
        id: ApprovalId,
        decision: Decision,
        answer: Option<String>,
        at: OffsetDateTime,
    ) -> Result<Option<Approval>, StoreError>;

    /// Phase F7（取りこぼしの照合）: 未決のまま、認可元のタスクが既に終端（`done` / `failed` /
    /// `cancelled`）になっている行を `Withdrawn` で閉じ、タスクごとに `Event::ApprovalsWithdrawn`
    /// （`reason = "reconcile"`）を追記する（1 トランザクション）。通常の経路は終端への遷移
    /// （`apply_transition` と同じトランザクション）で閉じるので、ここに来るのは F7 より前の残りと、
    /// 遷移と追記の競合だけ。閉じたものが無ければ空。
    fn approval_withdraw_stale(
        &self,
        at: OffsetDateTime,
    ) -> Result<Vec<WithdrawnApprovals>, StoreError>;

    fn standing_rule_append(&self, rule: &StandingRule) -> Result<(), StoreError>;
    /// `node_id = Some(id)` なら「全員向け（`node_id IS NULL`）」+「そのノード向け」、`None` なら絞り込み無し
    /// （全ノード分。GUI の一覧・編集に使う）。古い順。
    fn standing_rule_list(&self, node_id: Option<&str>) -> Result<Vec<StandingRule>, StoreError>;
    /// 無い id は `Ok(false)`。
    fn standing_rule_delete(&self, id: StandingRuleId) -> Result<bool, StoreError>;
}

/// Phase F7: `Event::ApprovalsWithdrawn.reason` — 終端への遷移と同じトランザクションで閉じた。
pub const WITHDRAWN_BY_TRANSITION: &str = "task_terminal";
/// Phase F7: `Event::ApprovalsWithdrawn.reason` — 照合（`approval_withdraw_stale`）で閉じた。
pub const WITHDRAWN_BY_RECONCILE: &str = "reconcile";

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Draft => "draft",
        Status::Ready => "ready",
        Status::Running => "running",
        Status::Blocked => "blocked",
        Status::Reviewing => "reviewing",
        Status::Done => "done",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
    }
}

/// Phase F7: 取り下げた行の `answer`（人が一覧で読む決定的な文面。先頭は `task <status>`）。
pub fn withdrawn_answer(status: Status) -> String {
    format!(
        "task {}: 認可元のタスクが終わったため、celeris が自動で取り下げました",
        status_word(status)
    )
}

/// Phase F7: 1 タスク分の取り下げ（`approval_withdraw_stale` の戻り値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawnApprovals {
    pub task_id: TaskId,
    pub task_status: Status,
    pub approval_ids: Vec<ApprovalId>,
}

/// Phase F7: `task_id` の未決の行を `Withdrawn` で閉じ、1 件以上あればそのタスクに
/// `Event::ApprovalsWithdrawn` を 1 件追記する。呼び出し側のトランザクション内で使う
/// （`SqliteStore::apply_transition_tx` の終端化と、`approval_withdraw_stale`）。
pub(crate) fn withdraw_pending_for_task_tx(
    conn: &rusqlite::Connection,
    task_id: TaskId,
    task_status: Status,
    reason: &str,
    at: OffsetDateTime,
) -> Result<Vec<ApprovalId>, StoreError> {
    let raw: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM approvals WHERE task_id = ?1 AND decision IS NULL \
             ORDER BY created_at ASC, id ASC",
        )?;
        stmt.query_map(params![task_id.to_string()], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?
    };
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::with_capacity(raw.len());
    for r in &raw {
        ids.push(
            r.parse::<ApprovalId>()
                .map_err(|_| StoreError::Invalid(format!("invalid approval id: {r}")))?,
        );
    }
    conn.execute(
        "UPDATE approvals SET decision = ?2, answer = ?3, decided_at = ?4 \
         WHERE task_id = ?1 AND decision IS NULL",
        params![
            task_id.to_string(),
            Decision::Withdrawn.as_str(),
            withdrawn_answer(task_status),
            format_rfc3339(at)?
        ],
    )?;
    SqliteStore::append_event_tx(
        conn,
        task_id,
        &Event::ApprovalsWithdrawn {
            approval_ids: ids.clone(),
            task_status,
            reason: reason.to_string(),
        },
    )?;
    Ok(ids)
}

fn row_to_approval(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Approval, StoreError>> {
    let id: String = row.get(0)?;
    let project_id: Option<String> = row.get(1)?;
    let node_id: String = row.get(2)?;
    let task_id: Option<String> = row.get(3)?;
    let question: String = row.get(4)?;
    let decision_col: Option<String> = row.get(5)?;
    let answer: Option<String> = row.get(6)?;
    let created_at: String = row.get(7)?;
    let decided_at: Option<String> = row.get(8)?;
    let Ok(id) = id.parse::<ApprovalId>() else {
        return Ok(Err(StoreError::Invalid(format!(
            "invalid approval id: {id}"
        ))));
    };
    let project_id = match project_id {
        None => None,
        Some(raw) => match raw.parse::<ProjectId>() {
            Ok(v) => Some(v),
            Err(_) => {
                return Ok(Err(StoreError::Invalid(format!(
                    "invalid approval project_id: {raw}"
                ))));
            }
        },
    };
    let task_id = match task_id {
        None => None,
        Some(raw) => match raw.parse::<TaskId>() {
            Ok(v) => Some(v),
            Err(_) => {
                return Ok(Err(StoreError::Invalid(format!(
                    "invalid approval task_id: {raw}"
                ))));
            }
        },
    };
    let decision = match decision_col {
        None => None,
        Some(raw) => match Decision::parse(&raw) {
            Some(d) => Some(d),
            None => {
                return Ok(Err(StoreError::Invalid(format!(
                    "invalid approval decision: {raw}"
                ))));
            }
        },
    };
    Ok((|| {
        Ok(Approval {
            id,
            project_id,
            node_id,
            task_id,
            question,
            decision,
            answer,
            created_at: parse_rfc3339(&created_at)?,
            decided_at: match decided_at {
                Some(raw) => Some(parse_rfc3339(&raw)?),
                None => None,
            },
        })
    })())
}

fn row_to_standing_rule(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<StandingRule, StoreError>> {
    let id: String = row.get(0)?;
    let node_id: Option<String> = row.get(1)?;
    let rule: String = row.get(2)?;
    let created_at: String = row.get(3)?;
    let Ok(id) = id.parse::<StandingRuleId>() else {
        return Ok(Err(StoreError::Invalid(format!(
            "invalid standing rule id: {id}"
        ))));
    };
    Ok((|| {
        Ok(StandingRule {
            id,
            node_id,
            rule,
            created_at: parse_rfc3339(&created_at)?,
        })
    })())
}

const SELECT_APPROVAL: &str = "SELECT id, project_id, node_id, task_id, question, decision, answer, \
                               created_at, decided_at FROM approvals";
const SELECT_STANDING_RULE: &str = "SELECT id, node_id, rule, created_at FROM standing_rules";

impl ApprovalStore for SqliteStore {
    fn approval_append(&self, approval: &Approval) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO approvals (id, project_id, node_id, task_id, question, decision, answer, \
             created_at, decided_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                approval.id.to_string(),
                approval.project_id.map(|p| p.to_string()),
                approval.node_id,
                approval.task_id.map(|t| t.to_string()),
                approval.question,
                approval.decision.map(|d| d.as_str()),
                approval.answer,
                format_rfc3339(approval.created_at)?,
                approval.decided_at.map(format_rfc3339).transpose()?,
            ],
        )?;
        Ok(())
    }

    fn approval_get(&self, id: ApprovalId) -> Result<Option<Approval>, StoreError> {
        let conn = self.lock()?;
        let row = conn
            .query_row(
                &format!("{SELECT_APPROVAL} WHERE id = ?1"),
                params![id.to_string()],
                row_to_approval,
            )
            .optional()?;
        match row {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    fn approval_list(
        &self,
        pending: Option<bool>,
        project_id: Option<ProjectId>,
        node_id: Option<&str>,
    ) -> Result<Vec<Approval>, StoreError> {
        let mut where_sql = String::from(" WHERE 1 = 1");
        let mut args: Vec<SqlValue> = Vec::new();
        match pending {
            Some(true) => where_sql.push_str(" AND decision IS NULL"),
            // R5: 「未決定ではない」= 人が決めたものだけ（以前は絞り込み無しになっていた）。
            Some(false) => where_sql.push_str(" AND decision IS NOT NULL"),
            None => {}
        }
        if let Some(project_id) = project_id {
            where_sql.push_str(" AND project_id = ?");
            args.push(SqlValue::Text(project_id.to_string()));
        }
        if let Some(node_id) = node_id {
            where_sql.push_str(" AND node_id = ?");
            args.push(SqlValue::Text(node_id.to_string()));
        }
        let sql = format!("{SELECT_APPROVAL}{where_sql} ORDER BY created_at ASC, id ASC");
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), row_to_approval)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    fn approval_decide(
        &self,
        id: ApprovalId,
        decision: Decision,
        answer: Option<String>,
        at: OffsetDateTime,
    ) -> Result<Option<Approval>, StoreError> {
        let conn = self.lock()?;
        Self::approval_decide_tx(&conn, id, decision, answer, at)
    }

    fn approval_withdraw_stale(
        &self,
        at: OffsetDateTime,
    ) -> Result<Vec<WithdrawnApprovals>, StoreError> {
        const STALE_SQL: &str = "SELECT DISTINCT a.task_id, t.status FROM approvals a \
             JOIN tasks t ON t.id = a.task_id \
             WHERE a.decision IS NULL AND t.status IN ('done', 'failed', 'cancelled') \
             ORDER BY a.task_id ASC";
        fn stale_rows(conn: &rusqlite::Connection) -> Result<Vec<(String, String)>, StoreError> {
            let mut stmt = conn.prepare(STALE_SQL)?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            Ok(rows)
        }
        let mut conn = self.lock()?;
        // tick ごとに呼ばれるので、何も無いとき（ほぼ常に）は書き込みのロックを取らない。
        if stale_rows(&conn)?.is_empty() {
            return Ok(Vec::new());
        }
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let stale = stale_rows(&tx)?;
        let mut out = Vec::new();
        for (task_raw, status_raw) in stale {
            let task_id = task_raw.parse::<TaskId>().map_err(|_| {
                StoreError::Invalid(format!("invalid approval task_id: {task_raw}"))
            })?;
            let task_status = match status_raw.as_str() {
                "done" => Status::Done,
                "failed" => Status::Failed,
                _ => Status::Cancelled,
            };
            let approval_ids = withdraw_pending_for_task_tx(
                &tx,
                task_id,
                task_status,
                WITHDRAWN_BY_RECONCILE,
                at,
            )?;
            if !approval_ids.is_empty() {
                out.push(WithdrawnApprovals {
                    task_id,
                    task_status,
                    approval_ids,
                });
            }
        }
        tx.commit()?;
        Ok(out)
    }

    fn standing_rule_append(&self, rule: &StandingRule) -> Result<(), StoreError> {
        let conn = self.lock()?;
        Self::standing_rule_append_tx(&conn, rule)
    }

    fn standing_rule_list(&self, node_id: Option<&str>) -> Result<Vec<StandingRule>, StoreError> {
        let (where_sql, args): (String, Vec<SqlValue>) = match node_id {
            Some(node_id) => (
                " WHERE node_id IS NULL OR node_id = ?".to_string(),
                vec![SqlValue::Text(node_id.to_string())],
            ),
            None => (String::new(), Vec::new()),
        };
        let sql = format!("{SELECT_STANDING_RULE}{where_sql} ORDER BY created_at ASC, id ASC");
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), row_to_standing_rule)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    fn standing_rule_delete(&self, id: StandingRuleId) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let changed = conn.execute(
            "DELETE FROM standing_rules WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(changed > 0)
    }
}

#[cfg(test)]
#[path = "approval/tests.rs"]
mod tests;

impl SqliteStore {
    /// `approval_decide` の本体。呼び出し側の transaction 内で使う（CoS の監査付き操作。ADR 2026-10-05 D3）。
    pub fn approval_decide_tx(
        conn: &rusqlite::Connection,
        id: ApprovalId,
        decision: Decision,
        answer: Option<String>,
        at: OffsetDateTime,
    ) -> Result<Option<Approval>, StoreError> {
        let ts = format_rfc3339(at)?;
        let changed = conn.execute(
            "UPDATE approvals SET decision = ?2, answer = ?3, decided_at = ?4 WHERE id = ?1",
            params![id.to_string(), decision.as_str(), answer, ts],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let row = conn
            .query_row(
                &format!("{SELECT_APPROVAL} WHERE id = ?1"),
                params![id.to_string()],
                row_to_approval,
            )
            .optional()?;
        match row {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// `standing_rule_append` の本体。呼び出し側の transaction 内で使う。
    pub fn standing_rule_append_tx(
        conn: &rusqlite::Connection,
        rule: &StandingRule,
    ) -> Result<(), StoreError> {
        conn.execute(
            "INSERT INTO standing_rules (id, node_id, rule, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                rule.id.to_string(),
                rule.node_id,
                rule.rule,
                format_rfc3339(rule.created_at)?
            ],
        )?;
        Ok(())
    }
}
