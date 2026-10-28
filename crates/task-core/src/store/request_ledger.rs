//! 論理要求の台帳の組み立て（ADR 2026-10-27-eval-request-ledger）。読み取りだけ。
use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;

use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::decision_log::intervention_of;
use super::{SqliteStore, StoreError};
use crate::decision_log::InterventionActorKind;
use crate::request_ledger::{
    ActorAt, HumanWaitSpan, REQUEST_LEDGER_SCHEMA, RequestCampaign, RequestLedger,
    RequestLedgerSummary, RequestReturn, RequestRevision, RequestRow,
};

/// 一次対応の入力 message の本文の頭（`cos_triage_claim` が書く）。
const CLAIM_PREFIX: &str = crate::chat::triage::COS_TRIAGE_CLAIM_PREFIX;

struct Item {
    id: String,
    kind: String,
    key: String,
    revision: String,
    thread_id: String,
    state: String,
    run_id: Option<String>,
    reason: Option<String>,
    policy_version: String,
    operation_id: Option<String>,
    created_at: String,
    updated_at: String,
}

struct Op {
    state: String,
    created_at: String,
    updated_at: String,
    policy_version: String,
    payload_json: String,
    target_kind: String,
    target_id: String,
}

struct ChatRun {
    thread_id: String,
    first_output_at: Option<String>,
}

/// 表から一度に読む材料。
struct Raw {
    items: Vec<Item>,
    ops: HashMap<String, Op>,
    claims: HashMap<String, Vec<String>>,
    chat_runs: HashMap<String, ChatRun>,
    thread_kinds: HashMap<String, String>,
    routes: HashMap<String, (String, String)>,
}

struct TaskEvent {
    task_id: String,
    seq: i64,
    ts: String,
    at: Option<OffsetDateTime>,
    event: crate::Event,
}

fn ts(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

fn read_raw(conn: &Connection) -> Result<Raw, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id,source_kind,source_key,source_revision,thread_id,state,run_id,reason,policy_version,\
         operation_id,created_at,updated_at FROM cos_inbox_items ORDER BY created_at,id",
    )?;
    let items = stmt
        .query_map([], |r| {
            Ok(Item {
                id: r.get(0)?,
                kind: r.get(1)?,
                key: r.get(2)?,
                revision: r.get(3)?,
                thread_id: r.get(4)?,
                state: r.get(5)?,
                run_id: r.get(6)?,
                reason: r.get(7)?,
                policy_version: r.get(8)?,
                operation_id: r.get(9)?,
                created_at: r.get(10)?,
                updated_at: r.get(11)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt =
        conn.prepare("SELECT id,state,created_at,updated_at,policy_version,payload_json,target_kind,target_id FROM cos_operations")?;
    let ops = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Op {
                    state: r.get(1)?,
                    created_at: r.get(2)?,
                    updated_at: r.get(3)?,
                    policy_version: r.get(4)?,
                    payload_json: r.get(5)?,
                    target_kind: r.get(6)?,
                    target_id: r.get(7)?,
                },
            ))
        })?
        .collect::<Result<HashMap<_, _>, _>>()?;
    // 担当の再起動で item の run_id は消えるので、item を取った run は一次対応の入力 message から引く。
    let mut claims: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT run_id,text FROM chat_messages WHERE role='system' AND run_id IS NOT NULL \
         AND substr(text,1,?1)=?2 ORDER BY created_at,id",
    )?;
    let rows = stmt
        .query_map(params![CLAIM_PREFIX.chars().count(), CLAIM_PREFIX], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (run_id, text) in rows {
        // 取り次ぎの再判断の run は 2 行目に取り次ぎを書く（ADR relayed-instructions 付記 A2）。
        let Some(json) = text
            .lines()
            .next()
            .and_then(|l| l.strip_prefix(CLAIM_PREFIX))
        else {
            continue;
        };
        let Ok(ids) = serde_json::from_str::<Vec<String>>(json) else {
            continue;
        };
        for id in ids {
            let runs = claims.entry(id).or_default();
            if !runs.contains(&run_id) {
                runs.push(run_id.clone());
            }
        }
    }
    let mut stmt = conn.prepare("SELECT run_id,thread_id,first_output_at FROM chat_runs")?;
    let chat_runs = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                ChatRun {
                    thread_id: r.get(1)?,
                    first_output_at: r.get(2)?,
                },
            ))
        })?
        .collect::<Result<HashMap<_, _>, _>>()?;
    let mut stmt = conn.prepare("SELECT id,kind FROM chat_threads")?;
    let thread_kinds = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<HashMap<_, _>, _>>()?;
    let mut stmt = conn.prepare("SELECT item_id,route,created_at FROM cos_notification_routes")?;
    let routes = stmt
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
        .collect::<Result<HashMap<_, _>, _>>()?;
    Ok(Raw {
        items,
        ops,
        claims,
        chat_runs,
        thread_kinds,
        routes,
    })
}

fn is_overridden(state: &str) -> bool {
    matches!(state, "superseded" | "needs_remediation")
}

/// item を論理要求へ寄せる（D1・D3）。返り値は要求ごとの item の index（取り込み順）。
fn group_items(raw: &Raw) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut by_key: HashMap<(String, String), usize> = HashMap::new();
    let mut by_item: HashMap<&str, usize> = HashMap::new();
    for (i, item) in raw.items.iter().enumerate() {
        // 差戻しで作られた item: 覆された operation の item と同じ source_kind・thread で、
        // operation の updated_at と同じ時刻に作られた item（同じ transaction）。
        let returned_from = raw.items[..i].iter().find(|from| {
            from.kind == item.kind
                && from.thread_id == item.thread_id
                && from
                    .operation_id
                    .as_ref()
                    .and_then(|o| raw.ops.get(o))
                    .is_some_and(|op| is_overridden(&op.state) && op.updated_at == item.created_at)
        });
        let key = (item.kind.clone(), item.key.clone());
        let group = match (
            returned_from.and_then(|f| by_item.get(f.id.as_str())),
            by_key.get(&key),
        ) {
            (Some(&g), _) | (None, Some(&g)) => g,
            (None, None) => {
                groups.push(Vec::new());
                groups.len() - 1
            }
        };
        groups[group].push(i);
        by_key.entry(key).or_insert(group);
        by_item.insert(item.id.as_str(), group);
    }
    groups
}

/// key から関係 task と decision id を引く。
fn key_tasks(
    conn: &Connection,
    kind: &str,
    key: &str,
) -> Result<(Vec<String>, Option<String>), StoreError> {
    let rest = key
        .strip_prefix(kind)
        .and_then(|r| r.strip_prefix('-'))
        .unwrap_or(key);
    let one = |sql: &str, id: &str| -> Result<Option<String>, StoreError> {
        Ok(conn
            .query_row(sql, [id], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten())
    };
    let task = match kind {
        "decision" => {
            let task = one("SELECT task_id FROM decisions WHERE id=?1", rest)?;
            return Ok((task.into_iter().collect(), Some(rest.to_string())));
        }
        "authorization" | "acceptance_check" => {
            one("SELECT task_id FROM approvals WHERE id=?1", rest)?
        }
        "notice" => one("SELECT task_id FROM feed_notices WHERE id=?1", key)?,
        _ => match rest.get(..26).map(crate::TaskId::from_str) {
            Some(Ok(id)) => one("SELECT id FROM tasks WHERE id=?1", &id.to_string())?,
            _ => None,
        },
    };
    Ok((task.into_iter().collect(), None))
}

fn task_events(conn: &Connection, tasks: &[String]) -> Result<Vec<TaskEvent>, StoreError> {
    let mut out = Vec::new();
    let mut stmt =
        conn.prepare("SELECT task_id,seq,ts,json FROM events WHERE task_id=?1 ORDER BY id")?;
    for task in tasks {
        let rows = stmt
            .query_map([task], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (task_id, seq, ts_s, json) in rows {
            let Ok(event) = serde_json::from_str::<crate::Event>(&json) else {
                continue;
            };
            out.push(TaskEvent {
                at: ts(&ts_s),
                task_id,
                seq,
                ts: ts_s,
                event,
            });
        }
    }
    Ok(out)
}

/// この source_kind の要求を解決する event なら、その `by`（D6）。
fn resolving_by<'e>(
    kind: &str,
    decision_id: Option<&str>,
    event: &'e crate::Event,
) -> Option<Option<&'e str>> {
    use crate::Event;
    match (kind, event) {
        ("question", Event::Answered { by, .. }) => Some(by.as_deref()),
        ("decision", Event::DecisionAnswered { id, by, .. })
            if Some(id.as_str()) == decision_id =>
        {
            Some(Some(by.as_str()))
        }
        ("decision", Event::DecisionWithdrawn { id, .. }) if Some(id.as_str()) == decision_id => {
            Some(None)
        }
        ("authorization" | "acceptance_check", Event::ApprovalDecided { by, .. })
        | ("plan_gate", Event::PlanGateDecided { by, .. }) => Some(Some(by.as_str())),
        ("integration_request", Event::IntegrationAnswered { by, .. }) => Some(by.as_deref()),
        (
            "failed"
            | "stalled"
            | "unroutable"
            | "review_resync_halted"
            | "unintegrated"
            | "delivery_skipped",
            Event::Retried { by, .. },
        ) => Some(by.as_deref()),
        _ => None,
    }
}

/// CoS の thread の種類を主体の種類へ。
fn cos_kind(thread_kind: Option<&String>) -> &'static str {
    match thread_kind.map(String::as_str) {
        Some("project") => "cos_project",
        Some("general") => "cos_general",
        _ => "unknown",
    }
}

fn earliest(slot: &mut Option<(OffsetDateTime, ActorAt)>, at: Option<OffsetDateTime>, v: ActorAt) {
    if let Some(at) = at
        && slot.as_ref().is_none_or(|(cur, _)| at < *cur)
    {
        *slot = Some((at, v));
    }
}

impl SqliteStore {
    fn ledger_actor(
        &self,
        by: Option<&str>,
        threads: &HashMap<String, String>,
    ) -> (String, Option<String>) {
        let (kind, id) = InterventionActorKind::classify(by, |t| {
            threads
                .get(t)
                .and_then(|k| serde_json::from_value(serde_json::Value::String(k.clone())).ok())
        });
        (kind.as_str().to_string(), id)
    }

    /// `[since, until)` に revision が取り込まれた論理要求の台帳（ADR 2026-10-27-eval-request-ledger）。
    pub fn request_ledger(
        &self,
        since: OffsetDateTime,
        until: OffsetDateTime,
    ) -> Result<RequestLedger, StoreError> {
        let raw = self.with_read_conn(read_raw)?;
        let in_range =
            |i: &usize| ts(&raw.items[*i].created_at).is_some_and(|at| at >= since && at < until);
        let groups: Vec<Vec<usize>> = group_items(&raw)
            .into_iter()
            .filter(|g| g.iter().any(in_range))
            .collect();
        let mut summary = RequestLedgerSummary::default();
        let mut requests = Vec::with_capacity(groups.len());
        for group in groups {
            let row = self.with_read_conn(|conn| self.request_row(conn, &raw, &group))?;
            let campaign = self.request_campaign(&row.tasks)?;
            let row = RequestRow { campaign, ..row };
            summary.items += row.revisions.len();
            summary.returns += row.returns.len();
            let resolved = row
                .resolved
                .as_ref()
                .map_or("unresolved", |r| r.actor_kind.as_str());
            *summary
                .resolved_by_actor_kind
                .entry(resolved.to_string())
                .or_default() += 1;
            summary.first_ai_response_missing += usize::from(row.first_ai_response.is_none());
            summary.open_human_waits += row
                .human_waits
                .iter()
                .filter(|w| w.ended_at.is_none())
                .count();
            requests.push(row);
        }
        summary.requests = requests.len();
        Ok(RequestLedger {
            schema: REQUEST_LEDGER_SCHEMA,
            since: since.format(&Rfc3339)?,
            until: until.format(&Rfc3339)?,
            summary,
            requests,
        })
    }

    fn request_campaign(&self, tasks: &[String]) -> Result<Option<RequestCampaign>, StoreError> {
        for task in tasks {
            if let Some(id) = self.task_campaign_resolve(task)? {
                return Ok(Some(RequestCampaign {
                    task_id: task.clone(),
                    campaign_id: id.campaign_id,
                    case_id: id.case_id,
                    arm: id.arm,
                    repetition: id.repetition,
                }));
            }
        }
        Ok(None)
    }

    fn request_row(
        &self,
        conn: &Connection,
        raw: &Raw,
        group: &[usize],
    ) -> Result<RequestRow, StoreError> {
        let items: Vec<&Item> = group.iter().map(|&i| &raw.items[i]).collect();
        let first = items[0];
        let mut keys: Vec<String> = Vec::new();
        let mut tasks: Vec<String> = Vec::new();
        let mut decision_ids: Vec<Option<String>> = Vec::new();
        for item in &items {
            let (ts_, decision) = key_tasks(conn, &item.kind, &item.key)?;
            for t in ts_ {
                if !tasks.contains(&t) {
                    tasks.push(t);
                }
            }
            if !keys.contains(&item.key) {
                keys.push(item.key.clone());
            }
            decision_ids.push(decision);
        }
        let events = task_events(conn, &tasks)?;
        // revision の窓（source の時刻、無ければ取り込み時刻から、次の revision まで）。
        let starts: Vec<Option<OffsetDateTime>> = items
            .iter()
            .map(|i| ts(&i.revision).or_else(|| ts(&i.created_at)))
            .collect();
        let mut revisions = Vec::with_capacity(items.len());
        let mut returns = Vec::new();
        let mut operations = BTreeSet::new();
        let mut policy = BTreeSet::new();
        let mut cos_runs: Vec<String> = Vec::new();
        let mut human_waits = Vec::new();
        let mut first_ai: Option<(OffsetDateTime, ActorAt)> = None;
        for (n, item) in items.iter().enumerate() {
            let op = item
                .operation_id
                .as_ref()
                .and_then(|o| raw.ops.get(o).map(|op| (o, op)));
            let returned = n > 0
                && items[..n].iter().any(|from| {
                    from.operation_id
                        .as_ref()
                        .and_then(|o| raw.ops.get(o))
                        .is_some_and(|op| {
                            is_overridden(&op.state) && op.updated_at == item.created_at
                        })
                });
            let event = match n {
                0 => "initial",
                _ if returned => "returned",
                _ => "revision",
            };
            if let Some((op_id, op)) = op {
                operations.insert(op_id.clone());
                operation_refs(op, &mut policy);
                if is_overridden(&op.state) {
                    let to = items[n + 1..]
                        .iter()
                        .find(|i| i.created_at == op.updated_at);
                    returns.push(RequestReturn {
                        at: op.updated_at.clone(),
                        operation_id: op_id.clone(),
                        operation_state: op.state.clone(),
                        from_item_id: item.id.clone(),
                        to_item_id: to.map(|i| i.id.clone()),
                    });
                }
            }
            policy.insert(format!("cos_triage_policy:{}", item.policy_version));
            let mut run_ids = raw.claims.get(&item.id).cloned().unwrap_or_default();
            if let Some(r) = &item.run_id
                && !run_ids.contains(r)
            {
                run_ids.push(r.clone());
            }
            for r in &run_ids {
                if !cos_runs.contains(r) {
                    cos_runs.push(r.clone());
                }
                if let Some(run) = raw.chat_runs.get(r)
                    && let Some(out) = &run.first_output_at
                {
                    earliest(
                        &mut first_ai,
                        ts(out),
                        ActorAt {
                            at: out.clone(),
                            actor_kind: cos_kind(raw.thread_kinds.get(&run.thread_id)).into(),
                            actor_id: Some(run.thread_id.clone()),
                            source: format!("chat_run:{r}"),
                        },
                    );
                }
            }
            // 解決（D6）: 窓の中の最初の解決 event。無ければ CoS の operation での回答。
            let (start, end) = (starts[n], starts.get(n + 1).copied().flatten());
            let in_window = |e: &&TaskEvent| {
                e.at.is_some_and(|at| start.is_none_or(|s| at >= s) && end.is_none_or(|x| at < x))
            };
            let mut resolved = events.iter().filter(in_window).find_map(|e| {
                resolving_by(&item.kind, decision_ids[n].as_deref(), &e.event).map(|by| {
                    let (actor_kind, actor_id) = match by {
                        Some(_) => self.ledger_actor(by, &raw.thread_kinds),
                        None => ("unknown".to_string(), None),
                    };
                    ActorAt {
                        at: e.ts.clone(),
                        actor_kind,
                        actor_id,
                        source: format!("event:{}:{}", e.task_id, e.seq),
                    }
                })
            });
            if resolved.is_none()
                && item.state == "answered"
                && let Some((op_id, _)) = op
            {
                resolved = Some(ActorAt {
                    at: item.updated_at.clone(),
                    actor_kind: cos_kind(raw.thread_kinds.get(&item.thread_id)).into(),
                    actor_id: Some(item.thread_id.clone()),
                    source: format!("cos_operation:{op_id}"),
                });
            }
            let (route, routed_at) = match raw.routes.get(&item.id) {
                Some((route, at)) => (Some(route.clone()), Some(at.clone())),
                None => (None, None),
            };
            if let (Some(route), Some(at)) = (&route, &routed_at) {
                human_waits.push(HumanWaitSpan {
                    item_id: item.id.clone(),
                    revision: item.revision.clone(),
                    route: route.clone(),
                    started_at: at.clone(),
                    ended_at: resolved.as_ref().map(|r| r.at.clone()),
                    end_actor_kind: resolved.as_ref().map(|r| r.actor_kind.clone()),
                    end_actor_id: resolved.as_ref().and_then(|r| r.actor_id.clone()),
                });
            }
            revisions.push(RequestRevision {
                item_id: item.id.clone(),
                source_key: item.key.clone(),
                revision: item.revision.clone(),
                event,
                created_at: item.created_at.clone(),
                state: item.state.clone(),
                reason: item.reason.clone(),
                policy_version: item.policy_version.clone(),
                thread_id: item.thread_id.clone(),
                thread_kind: raw.thread_kinds.get(&item.thread_id).cloned(),
                run_ids,
                operation_id: item.operation_id.clone(),
                operation_state: op.map(|(_, op)| op.state.clone()),
                route,
                routed_at,
                resolved,
            });
        }
        let first_start = starts[0];
        let after_first =
            |at: Option<OffsetDateTime>| at.is_some_and(|at| first_start.is_none_or(|s| at >= s));
        // 人へ回した item は escalated のまま operation を持たないので、その後に CoS が要求の対象（decision・
        // 受信箱の件）へ行った operation を同じ要求に寄せる（D8 の根拠の参照を落とさない。却下・未決は除く）。
        let decisions: Vec<&str> = decision_ids.iter().flatten().map(String::as_str).collect();
        let mut later: Vec<(&String, &Op)> = raw
            .ops
            .iter()
            .filter(|(id, op)| {
                !operations.contains(*id)
                    && matches!(
                        op.state.as_str(),
                        "applied" | "superseded" | "needs_remediation"
                    )
                    && match op.target_kind.as_str() {
                        "decision" => decisions.contains(&op.target_id.as_str()),
                        "inbox" => keys.contains(&op.target_id),
                        _ => false,
                    }
                    && after_first(ts(&op.created_at))
            })
            .collect();
        later.sort_by(|a, b| a.0.cmp(b.0));
        for (op_id, op) in later {
            operations.insert(op_id.clone());
            operation_refs(op, &mut policy);
        }
        // 外部 operator の一次対応（D4）: 関係 task への operator の comment か、`by = operator:` の event。
        let mut stmt = conn.prepare(
            "SELECT id,author,created_at FROM task_comments WHERE task_id=?1 AND author_kind='operator'",
        )?;
        for task in &tasks {
            let rows = stmt
                .query_map([task], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (id, author, at) in rows {
                if after_first(ts(&at)) {
                    let (actor_kind, actor_id) = match author.as_deref() {
                        Some(a) if a.contains(':') => self.ledger_actor(Some(a), &raw.thread_kinds),
                        Some(a) => ("external_operator".to_string(), Some(a.to_string())),
                        None => ("external_operator".to_string(), None),
                    };
                    earliest(
                        &mut first_ai,
                        ts(&at),
                        ActorAt {
                            at: at.clone(),
                            actor_kind,
                            actor_id,
                            source: format!("comment:{id}"),
                        },
                    );
                }
            }
        }
        for e in &events {
            if let Some((_, Some(by))) = intervention_of(&e.event)
                && by.starts_with("operator:")
                && after_first(e.at)
            {
                let (actor_kind, actor_id) = self.ledger_actor(Some(by), &raw.thread_kinds);
                earliest(
                    &mut first_ai,
                    e.at,
                    ActorAt {
                        at: e.ts.clone(),
                        actor_kind,
                        actor_id,
                        source: format!("event:{}:{}", e.task_id, e.seq),
                    },
                );
            }
        }
        let resolved = revisions.last().and_then(|r| r.resolved.clone());
        let (resumed_at, resume_run_id) = match resolved.as_ref().and_then(|r| ts(&r.at)) {
            Some(after) => resume_after(conn, &tasks, after)?,
            None => (None, None),
        };
        let last = items[items.len() - 1];
        Ok(RequestRow {
            request_ref: format!("{}:{}", first.kind, first.key),
            source_kind: first.kind.clone(),
            source_key: first.key.clone(),
            keys,
            first_seen_at: first.created_at.clone(),
            last_seen_at: last.created_at.clone(),
            revisions,
            returns,
            first_ai_response: first_ai.map(|(_, a)| a),
            human_waits,
            resolved,
            resumed_at,
            resume_run_id,
            policy_refs: policy.into_iter().collect(),
            tasks,
            cos_runs,
            operations: operations.into_iter().collect(),
            campaign: None,
        })
    }
}

/// operation の方針版と、根拠にした人の発言・取り次ぎの参照（D8）。
fn operation_refs(op: &Op, policy: &mut BTreeSet<String>) {
    policy.insert(format!("cos_operation_policy:{}", op.policy_version));
    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&op.payload_json)
        && let Some(by) = payload.get("instructed_by")
    {
        if let Some(m) = by.get("message_id").and_then(|v| v.as_str()) {
            policy.insert(format!("instructed_by:{m}"));
        }
        if let Some(r) = by.get("decided_by").and_then(|d| d.get("relayed_by")) {
            let kind = r.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown");
            let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("");
            policy.insert(format!("relay:{kind}:{id}"));
        }
    }
}

/// 解決の後に関係 task とその子孫で最初に始まった run（D7）。
fn resume_after(
    conn: &Connection,
    tasks: &[String],
    after: OffsetDateTime,
) -> Result<(Option<String>, Option<String>), StoreError> {
    let mut best: Option<(OffsetDateTime, String, String)> = None;
    let mut stmt = conn.prepare(
        "WITH RECURSIVE tree(id) AS (SELECT ?1 UNION SELECT t.id FROM tasks t JOIN tree ON t.parent_id = tree.id) \
         SELECT r.run_id, r.started_at FROM runs r JOIN tree ON r.task_id = tree.id",
    )?;
    for task in tasks {
        let rows = stmt
            .query_map([task], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (run_id, started) in rows {
            if let Some(at) = ts(&started)
                && at > after
                && best.as_ref().is_none_or(|(b, _, _)| at < *b)
            {
                best = Some((at, started, run_id));
            }
        }
    }
    Ok(match best {
        Some((_, at, run)) => (Some(at), Some(run)),
        None => (None, None),
    })
}

#[cfg(test)]
#[path = "request_ledger_tests.rs"]
mod tests;
