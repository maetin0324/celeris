//! B/C 用の実行器: 通常の Celeris task 投入経路（task-api の HTTP）で試行を投入・待機・再開する。
//! 新しいスケジューラは作らず、本番 SQLite にも書かない。HTTP は [`Transport`] の背後に置き、
//! この crate には実 network 実装を置かない（試験は in-process の偽 server）。
//!
//! - 冪等: 冪等キーの先頭 64 文字を task の label にし、`GET /tasks?label=` で再照会する。投入の直前にも
//!   照会し、二重起動で重複が出来たら新しい方を cancel して古い方へ寄せる。
//! - routing: B=fixed、C=decision_assisted を `routing_override` で task に固定する（全体 config に依存しない）。
//! - 追跡: 子 task を再帰的にたどり、全 task が終端になり、cluster job が終わるまで `Wait`。
//! - 打ち切り: 試行の timeout 超過で task と子孫を cancel API で閉じる。cluster job は自動で消さず、
//!   `scancel`/`qdel` の人手手順を [`CLUSTER_CANCEL_FILE`] に追記する。
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use serde_json::{Map, Value, json};

use crate::{
    aggregate::Outcome,
    executor::{ExecResult, ExecStatus, Executor, TaskRef, TrialRequest},
    manifest::{Config, ConfigId},
};

/// cluster job の人手 cancel 手順の台帳（追記専用 JSONL、out 直下に置く）。
pub const CLUSTER_CANCEL_FILE: &str = "cluster-cancel.jsonl";
pub const CLUSTER_CANCEL_SCHEMA: &str = "celeris.campaign-cluster-cancel/1";
/// `routing_override.lower_policy` の既定（task-core `DEFAULT_POLICY_VERSION` と同じ値を明示して固定する）。
pub const DEFAULT_LOWER_POLICY: &str = "mlsys-min-1";
/// 冪等キー（label）で引く task の上限。
const LOOKUP_LIMIT: u32 = 50;
/// label の最大長（task-core `is_valid_label`）。
const LABEL_MAX: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Value,
}

/// task-api への HTTP。`Err` は応答が得られなかった（接続断・応答喪失）ことを表し、要求が届いたかは不明。
pub trait Transport: Send + Sync {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String>;
}

/// 冪等キーから作る task label。
pub fn idempotency_label(key: &str) -> String {
    key.chars().take(LABEL_MAX).collect()
}

/// arm の routing 固定（`POST /tasks` の `routing_override`）。A は Celeris で走らせないので Err。
pub fn routing_override(cfg: &Config) -> Result<Value, String> {
    let mode = match cfg.id {
        ConfigId::A => {
            return Err("config A is not run via Celeris (use the harness executor)".into());
        }
        ConfigId::B => "fixed",
        ConfigId::C => "decision_assisted",
    };
    let r = cfg.routing.clone().unwrap_or_default();
    if cfg.id == ConfigId::C && r.evidence_table_id.is_none() {
        return Err("config C: routing.evidence_table_id must be pinned".into());
    }
    let mut o = json!({
        "mode": mode,
        "lower_policy": r.lower_policy.unwrap_or_else(|| DEFAULT_LOWER_POLICY.into()),
        "online_update": r.online_update.unwrap_or(false),
    });
    if let Some(e) = r.evidence_table_id {
        o["evidence_table_id"] = json!(e);
    }
    Ok(o)
}

pub struct CelerisExecutor<T: Transport> {
    transport: T,
    /// `POST /tasks` 本文の雛形（acceptance・project_id・repos・role 等）。title・objective・labels・
    /// workspace・max_wall_secs・routing_override はこの実行器が埋める。
    template: Map<String, Value>,
    /// 試行の打ち切り（task 作成からの経過秒、server の時計で測る）。
    trial_timeout_secs: u64,
    cluster_cancel_log: PathBuf,
}

/// 1 回の走査で見た task 木。
struct TreeNode {
    id: String,
    status: String,
    detail: Value,
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "done" | "failed" | "cancelled")
}

fn ok(r: &HttpResponse) -> bool {
    (200..300).contains(&r.status)
}

/// RFC 3339 の時刻（`YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`）を unix 秒にする。
/// 新しい依存（time crate）を足さないための最小の読み取り（端数秒は切り捨て）。
fn parse_ts(v: &Value) -> Option<i64> {
    let s = v.as_str()?;
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse::<i64>().ok() };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let offset = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let o = num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60;
            if *sign == b'-' { -o } else { o }
        }
        _ => return None,
    };
    // days from civil（proleptic Gregorian）
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se - offset)
}

fn secs_between(a: Option<i64>, b: Option<i64>) -> Option<u64> {
    Some((b? - a?).max(0) as u64)
}

/// `scheduler:cluster:job_id` の形の cluster job 参照。
fn job_ref(scheduler: &str, cluster: &str, job_id: &str) -> String {
    format!("{scheduler}:{cluster}:{job_id}")
}

impl<T: Transport> CelerisExecutor<T> {
    pub fn new(
        transport: T,
        template: Map<String, Value>,
        trial_timeout_secs: u64,
        cluster_cancel_log: PathBuf,
    ) -> Self {
        Self {
            transport,
            template,
            trial_timeout_secs,
            cluster_cancel_log,
        }
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let r = self.transport.request("GET", path, None)?;
        if !ok(&r) {
            return Err(format!("GET {path}: HTTP {} {}", r.status, r.body));
        }
        Ok(r.body)
    }

    /// 同じ label の task を id 昇順（ULID = 作成順）で返す。
    fn tasks_with_label(&self, key: &str) -> Result<Vec<String>, String> {
        let path = format!(
            "/api/v1/tasks?label={}&archived=1&limit={LOOKUP_LIMIT}",
            idempotency_label(key)
        );
        let v = self.get(&path)?;
        let mut ids: Vec<String> = v["items"]
            .as_array()
            .ok_or_else(|| format!("GET {path}: items missing"))?
            .iter()
            .filter_map(|t| t["id"].as_str().map(String::from))
            .collect();
        ids.sort();
        Ok(ids)
    }

    fn body(&self, req: &TrialRequest, key: &str) -> Result<Value, String> {
        let mut b = self.template.clone();
        let mut labels: Vec<Value> = b
            .get("labels")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        labels.push(json!(idempotency_label(key)));
        b.insert("labels".into(), Value::Array(labels));
        b.insert(
            "title".into(),
            json!(format!(
                "campaign {:?} {} #{}",
                req.config.id, req.problem_id, req.trial_no
            )),
        );
        b.insert(
            "objective".into(),
            json!(format!("{}\n\ncampaign-idempotency-key: {key}", req.prompt)),
        );
        b.insert(
            "workspace".into(),
            json!(req.workspace.to_string_lossy().into_owned()),
        );
        b.insert("max_wall_secs".into(), json!(req.timeout_secs));
        b.insert("routing_override".into(), routing_override(req.config)?);
        Ok(Value::Object(b))
    }

    /// 根から子 task を再帰的にたどる（幅優先・根が先頭）。
    fn walk(&self, root: &str) -> Result<Vec<TreeNode>, String> {
        let mut out: Vec<TreeNode> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut queue = vec![root.to_string()];
        while let Some(id) = (!queue.is_empty()).then(|| queue.remove(0)) {
            if !seen.insert(id.clone()) {
                continue;
            }
            let detail = self.get(&format!("/api/v1/tasks/{id}"))?;
            let status = detail["task"]["status"]
                .as_str()
                .ok_or_else(|| format!("task {id}: status missing"))?
                .to_string();
            for c in detail["children"].as_array().into_iter().flatten() {
                if let Some(cid) = c["id"].as_str() {
                    queue.push(cid.to_string());
                }
            }
            out.push(TreeNode { id, status, detail });
        }
        Ok(out)
    }

    /// 木の cluster job（参照と、まだ終わっていないか）。
    fn jobs(tree: &[TreeNode]) -> Vec<(String, bool)> {
        let mut v = Vec::new();
        for n in tree {
            let w = &n.detail["cluster_job_wait"];
            let (Some(s), Some(c)) = (w["scheduler"].as_str(), w["cluster"].as_str()) else {
                continue;
            };
            for j in w["jobs"].as_array().into_iter().flatten() {
                if let Some(id) = j["job_id"].as_str() {
                    let live = !matches!(j["state"].as_str(), Some("finished" | "gone"));
                    v.push((job_ref(s, c, id), live));
                }
            }
        }
        v
    }

    fn task_ref(t: &TaskRef, tree: &[TreeNode]) -> TaskRef {
        let mut children = t.child_task_ids.clone();
        for n in tree.iter().skip(1) {
            if !children.contains(&n.id) {
                children.push(n.id.clone());
            }
        }
        let mut jobs = t.cluster_job_ids.clone();
        for (j, _) in Self::jobs(tree) {
            if !jobs.contains(&j) {
                jobs.push(j);
            }
        }
        let run_id = tree
            .first()
            .and_then(|n| n.detail["runs"].as_array()?.last()?["run_id"].as_str())
            .map(String::from)
            .or_else(|| t.run_id.clone());
        TaskRef {
            task_id: t.task_id.clone(),
            run_id,
            child_task_ids: children,
            cluster_job_ids: jobs,
        }
    }

    /// 木の全 run の usage の合計。usage の無い run が 1 つでもあれば欠測（None）。
    fn usage(tree: &[TreeNode]) -> (Option<u64>, Option<u64>) {
        let (mut tokens, mut cost) = (Some(0u64), Some(0u64));
        for r in tree
            .iter()
            .flat_map(|n| n.detail["runs"].as_array().into_iter().flatten())
        {
            let u = &r["usage"];
            let t = match (u["input_tokens"].as_u64(), u["output_tokens"].as_u64()) {
                (Some(i), Some(o)) => Some(i + o),
                _ => None,
            };
            tokens = tokens.zip(t).map(|(a, b)| a + b);
            let c = u["cost_usd"].as_f64().map(|x| (x * 1e6).round() as u64);
            cost = cost.zip(c).map(|(a, b)| a + b);
        }
        (tokens, cost)
    }

    fn record_cluster_cancel(&self, task_id: &str, job: &str, reason: &str) -> Result<(), String> {
        let existing = fs::read_to_string(&self.cluster_cancel_log).unwrap_or_default();
        let mut parts = job.splitn(3, ':');
        let (s, c, id) = (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
        );
        let command = match s {
            "slurm" => format!("scancel {id}"),
            "pbs" => format!("qdel {id}"),
            other => format!("# unknown scheduler {other}: cancel job {id}"),
        };
        let row = json!({
            "schema": CLUSTER_CANCEL_SCHEMA, "task_id": task_id, "job": job,
            "cluster": c, "scheduler": s, "job_id": id, "command": command,
            "reason": reason, "executed": false,
        });
        // 同じ job・理由の行は一度だけ（cancel の再試行で手順を重ねない）。
        if existing.lines().any(|l| {
            serde_json::from_str::<Value>(l)
                .is_ok_and(|v| v["job"] == row["job"] && v["reason"] == row["reason"])
        }) {
            return Ok(());
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.cluster_cancel_log)
            .map_err(|e| format!("{}: {e}", self.cluster_cancel_log.display()))?;
        writeln!(f, "{row}").map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())
    }

    fn cancel_task(&self, id: &str) -> Result<(), String> {
        let path = format!("/api/v1/tasks/{id}/cancel");
        let r = self.transport.request("POST", &path, Some(&json!({})))?;
        // 409 は既に終端（cancel 不要）。
        if ok(&r) || r.status == 409 {
            return Ok(());
        }
        Err(format!("POST {path}: HTTP {} {}", r.status, r.body))
    }
}

impl<T: Transport> Executor for CelerisExecutor<T> {
    fn name(&self) -> &'static str {
        "celeris"
    }

    fn execute(&self, _req: &TrialRequest) -> Result<ExecStatus, String> {
        Err("celeris executor is driven via submit/poll".into())
    }

    fn is_async(&self) -> bool {
        true
    }

    fn routing(&self, req: &TrialRequest) -> Value {
        routing_override(req.config).unwrap_or_else(|e| json!({ "error": e }))
    }

    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, String> {
        Ok(self
            .tasks_with_label(key)?
            .into_iter()
            .next()
            .map(|id| TaskRef {
                task_id: id,
                ..TaskRef::default()
            }))
    }

    fn submit(&self, req: &TrialRequest, key: &str) -> Result<TaskRef, String> {
        let body = self.body(req, key)?;
        // 二重起動の防止: 投入直前にも照会する。
        if let Some(t) = self.lookup(key)? {
            return Ok(t);
        }
        let r = self
            .transport
            .request("POST", "/api/v1/tasks", Some(&body))?;
        if !ok(&r) {
            return Err(format!("POST /api/v1/tasks: HTTP {} {}", r.status, r.body));
        }
        let created = r.body["id"]
            .as_str()
            .ok_or("POST /api/v1/tasks: id missing")?
            .to_string();
        // 照会と投入の間に別の起動が投入していたら、古い方を正とし、新しい方を閉じる。
        let ids = self.tasks_with_label(key)?;
        let first = ids.first().cloned().unwrap_or_else(|| created.clone());
        if first != created {
            self.cancel_task(&created)?;
        }
        Ok(TaskRef {
            task_id: first,
            ..TaskRef::default()
        })
    }

    fn track(&self, t: &TaskRef) -> Result<TaskRef, String> {
        let tree = self.walk(&t.task_id)?;
        Ok(Self::task_ref(t, &tree))
    }

    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, String> {
        let tree = self.walk(&t.task_id)?;
        let root = tree.first().ok_or("empty task tree")?;
        let created = parse_ts(&root.detail["task"]["created_at"]);
        let elapsed = secs_between(created, parse_ts(&root.detail["timers"]["now"]));
        let pending = tree.iter().any(|n| !is_terminal(&n.status))
            || Self::jobs(&tree).iter().any(|(_, live)| *live);
        let (tokens, cost_micro_usd) = Self::usage(&tree);
        if pending {
            if elapsed.is_some_and(|e| e > self.trial_timeout_secs) {
                self.cancel(&Self::task_ref(t, &tree), "trial_timeout")?;
                return Ok(ExecStatus::Done(ExecResult {
                    outcome: Outcome::Timeout,
                    tokens,
                    cost_micro_usd,
                    wall_secs: self.trial_timeout_secs,
                }));
            }
            return Ok(ExecStatus::Wait);
        }
        let outcome = match root.status.as_str() {
            "done" => Outcome::Passed,
            "cancelled" => Outcome::Cancelled,
            _ => Outcome::Failed,
        };
        // 壁時計: task 作成から木の最後の run の終了まで（無ければ server の現在時刻まで）。
        let last_finish = tree
            .iter()
            .flat_map(|n| n.detail["runs"].as_array().into_iter().flatten())
            .filter_map(|r| parse_ts(&r["finished_at"]))
            .max();
        let wall_secs = secs_between(created, last_finish).or(elapsed).unwrap_or(0);
        Ok(ExecStatus::Done(ExecResult {
            outcome,
            tokens,
            cost_micro_usd,
            wall_secs,
        }))
    }

    /// 子孫（深い順）→ 根の順に終端でない task を cancel API で閉じ、cluster job は人手手順を記録する。
    /// 失敗があっても全対象を試してから Err を返す。
    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), String> {
        let mut errors = Vec::new();
        let (tree, mut jobs) = match self.walk(&t.task_id) {
            Ok(tree) => {
                let jobs: Vec<String> = Self::jobs(&tree)
                    .into_iter()
                    .filter(|(_, live)| *live)
                    .map(|(j, _)| j)
                    .collect();
                (Some(tree), jobs)
            }
            Err(e) => {
                errors.push(e);
                (None, t.cluster_job_ids.clone())
            }
        };
        let targets: Vec<String> = match &tree {
            Some(tree) => tree
                .iter()
                .rev()
                .filter(|n| !is_terminal(&n.status))
                .map(|n| n.id.clone())
                .collect(),
            // 木が読めないときは台帳の記録（子 → 根）で cancel を試す。
            None => t
                .child_task_ids
                .iter()
                .rev()
                .chain(std::iter::once(&t.task_id))
                .cloned()
                .collect(),
        };
        for id in targets {
            if let Err(e) = self.cancel_task(&id) {
                errors.push(e);
            }
        }
        jobs.dedup();
        for j in jobs {
            if let Err(e) = self.record_cluster_cancel(&t.task_id, &j, reason) {
                errors.push(e);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

#[cfg(test)]
mod ts_tests {
    use super::parse_ts;
    use serde_json::json;

    #[test]
    fn parses_rfc3339_to_unix_secs() {
        assert_eq!(parse_ts(&json!("1970-01-01T00:00:00Z")), Some(0));
        assert_eq!(
            parse_ts(&json!("2026-10-16T00:00:00Z")),
            Some(1_792_108_800)
        );
        assert_eq!(
            parse_ts(&json!("2026-10-16T09:00:00.123+09:00")),
            Some(1_792_108_800)
        );
        assert_eq!(
            parse_ts(&json!("2000-02-29T12:00:00-01:30")),
            Some(951_831_000)
        );
        assert_eq!(parse_ts(&json!("not a time")), None);
        assert_eq!(parse_ts(&json!(5)), None);
    }
}
