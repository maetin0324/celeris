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
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};
use task_core::variant_wire::{CampaignOrgPin, CampaignVariantRef};

use crate::{
    aggregate::Outcome,
    executor::{
        ExecError, ExecResult, ExecStatus, Executor, ObservedRun, ObservedVersions, TaskRef,
        TokenUsage, TrialRequest, Usage,
    },
    manifest::{Config, ConfigId},
    variant::{OrgPin, VariantRef},
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
    // 全体 config の既定に頼らず、manifest の arm 設定を必ず task 単位で固定する。
    let r = cfg.routing.clone().ok_or_else(|| {
        format!(
            "config {:?}: routing must be pinned in the manifest",
            cfg.id
        )
    })?;
    let evidence_mode = r.effective_evidence_mode(cfg.id)?;
    let mut o = json!({
        "mode": mode,
        "lower_policy": r.lower_policy.unwrap_or_else(|| DEFAULT_LOWER_POLICY.into()),
        "evidence_mode": evidence_mode,
        "online_update": r.online_update.unwrap_or(false),
    });
    if let Some(e) = r.evidence_table_id {
        o["evidence_table_id"] = json!(e);
    }
    Ok(o)
}

/// `--task-template` の検査。`acceptance` は task-api の形（`{"type": …, "text": …}`）だけを受ける。
/// 旧い入れ子（`{"text", "check": {"type": …}}`）は API が 400 を返すので、投入前に拒否する。
/// 実行器が埋める欄（title・objective・labels 以外の上書き不可欄）を template に書くことも拒否する。
pub fn check_template(t: &Map<String, Value>) -> Result<(), String> {
    // max_turns を書かない雛形は task の既定（10）に黙って落ち、worker・reviewer が 10 turn で止まる
    // （mlsys-main-3）。worker と reviewer は同じ task.budget.max_turns を --max-turns に渡す。
    if !t
        .get("max_turns")
        .and_then(Value::as_u64)
        .is_some_and(|n| n > 0)
    {
        return Err(
            "--task-template must set max_turns to a positive integer (e.g. 100); \
             without it the task silently falls back to the default of 10"
                .to_string(),
        );
    }
    for k in [
        "title",
        "objective",
        "workspace",
        "repos",
        "workspace_mode",
        "max_wall_secs",
        "max_tokens",
        "routing_override",
        "campaign",
        "idempotency_key",
    ] {
        if t.contains_key(k) {
            return Err(format!(
                "--task-template must not set {k:?} (the executor fills it per trial)"
            ));
        }
    }
    for k in ["assignee", "tier"] {
        if t.contains_key(k) {
            return Err(format!(
                "--task-template must not set {k:?} (Celeris chooses it)"
            ));
        }
    }
    let Some(acc) = t.get("acceptance") else {
        return Ok(());
    };
    let items = acc
        .as_array()
        .ok_or("--task-template: acceptance must be an array")?;
    for (i, a) in items.iter().enumerate() {
        if a.get("check").is_some() {
            return Err(format!(
                "--task-template: acceptance[{i}] uses the nested {{\"text\", \"check\": {{…}}}} form; \
                 write {{\"type\": \"reviewer\", \"text\": \"…\"}} (the task-api shape)"
            ));
        }
        if !a.get("type").is_some_and(Value::is_string) {
            return Err(format!(
                "--task-template: acceptance[{i}] needs a string \"type\" (e.g. \"reviewer\")"
            ));
        }
    }
    Ok(())
}

pub struct CelerisExecutor<T: Transport> {
    transport: T,
    /// `POST /tasks` 本文の雛形（acceptance・project_id・role 等）。title・objective・labels・
    /// workspace・repos（空配列）・max_wall_secs・max_tokens・routing_override はこの実行器が埋める。
    template: Map<String, Value>,
    /// 試行の打ち切り（task 作成からの経過秒、server の時計で測る）。
    trial_timeout_secs: u64,
    cluster_cancel_log: PathBuf,
    /// manifest の variant 参照。Some のとき POST は `campaign.variant` に載せ、既存 task の照会（lookup・
    /// submit 前後）は `GET /tasks/{id}` の `task.campaign.variant` が一致しない限り採用しない。
    variant: Option<VariantRef>,
    /// variant が前提とする構成 pin（`VariantSpec.org`）。variant があるとき必須（POST の `campaign.variant_org`）。
    variant_org: Option<OrgPin>,
    /// 測定 daemon の worker が起動する Claude Code（`--worker-program claude_code=<path>`）。試行の開始時に
    /// sha256 と `--version` を観測する。無ければ Claude Code の版は観測できない（契約 revision の試行は拒否される）。
    claude_code: Option<PathBuf>,
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

/// 木の cluster job（参照と、まだ終わっていないか）。
fn tree_jobs(tree: &[TreeNode]) -> Vec<(String, bool)> {
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

/// 木の全 task が終端で、cluster job もすべて終わった（PBS `F` / Slurm の終端 / scheduler が「知らない」）か。
/// task が cancelled でも、job が live なら停止とみなさない（task cancel と job 停止は別の事実）。
fn all_stopped(tree: &[TreeNode]) -> bool {
    tree.iter().all(|n| is_terminal(&n.status)) && !tree_jobs(tree).iter().any(|(_, live)| *live)
}

/// 根 task がこの campaign の投入物か（冪等キー由来の label `campaign-…` を持つ）。
/// 台帳の task id が他の task を指していても、無関係な task・job を止めないための所有判定。
fn owned_by_campaign(root: &TreeNode) -> bool {
    root.detail["task"]["labels"]
        .as_array()
        .is_some_and(|ls| ls.iter().any(|l| l.as_str().is_some_and(is_campaign_label)))
}

fn is_campaign_label(l: &str) -> bool {
    l.strip_prefix("campaign-")
        .is_some_and(|h| h.len() >= 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
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
            variant: None,
            variant_org: None,
            claude_code: None,
        }
    }

    /// 試行の開始時に観測する Claude Code の実行ファイル（daemon の `[adapters.claude_code] command` と同じ実体）。
    pub fn with_claude_code(mut self, program: Option<PathBuf>) -> Self {
        self.claude_code = program;
        self
    }

    /// manifest の variant 参照を固定する（省略 = 従来どおり）。
    pub fn with_variant(mut self, variant: Option<VariantRef>) -> Self {
        self.variant = variant;
        self
    }

    /// variant の構成 pin を固定する（variant があるときは必須。無ければ投入前に拒否する）。
    pub fn with_variant_org(mut self, org: Option<OrgPin>) -> Self {
        self.variant_org = org;
        self
    }

    /// 既存 task を採用してよいか（ADR 2026-10-20-mlsys-main2-fixes D2）: 案件のリポジトリを持たず（`repos` が空）、
    /// 作業場所が試行の trial workspace そのものであること。違えば成果物が採点の場所に出ないので、採点しない
    /// （Refused）。応答に欄が無いときは判定しない（旧い応答の形）。`req` が無い照会では repos だけを見る。
    fn check_workspace(&self, id: &str, req: Option<&TrialRequest>) -> Result<(), ExecError> {
        let detail = self
            .get(&format!("/api/v1/tasks/{id}"))
            .map_err(ExecError::interrupted)?;
        let task = &detail["task"];
        if task["repos"].as_array().is_some_and(|r| !r.is_empty()) {
            return Err(ExecError::refused(format!(
                "task {id}: repos {} is not empty; a campaign task runs in the trial workspace only; not reused",
                task["repos"]
            )));
        }
        if let (Some(req), Some(path)) = (req, task["workspace"]["path"].as_str())
            && Path::new(path) != req.workspace
        {
            return Err(ExecError::refused(format!(
                "task {id}: workspace {path:?} is not the trial workspace {:?}; not reused",
                req.workspace
            )));
        }
        Ok(())
    }

    /// 既存 task を冪等照会の結果として採用してよいか。variant があるときは task の記録と一致する場合だけ。
    /// 欠落（API 未対応・応答欠落）・不一致は適用済み扱いにせず拒否する（副作用なし）。
    fn check_existing(&self, id: &str, req: Option<&TrialRequest>) -> Result<(), ExecError> {
        let Some(want) = &self.variant else {
            return Ok(());
        };
        let Some(org) = &self.variant_org else {
            return Err(ExecError::refused("variant_org missing in the executor"));
        };
        let detail = self
            .get(&format!("/api/v1/tasks/{id}"))
            .map_err(ExecError::interrupted)?;
        // 解決済みの識別は `TaskDetail.campaign`（祖先解決後）。無ければ適用・記録を確認できない。
        let got = &detail["campaign"];
        if !got.is_object() {
            return Err(ExecError::refused(format!(
                "task {id}: campaign missing in the response (variant {} not applied/recorded); not reused",
                want.content_sha256
            )));
        }
        let want_v = serde_json::to_value(CampaignVariantRef::from(want))
            .map_err(|e| ExecError::refused(e.to_string()))?;
        let want_o = serde_json::to_value(CampaignOrgPin::from(org))
            .map_err(|e| ExecError::refused(e.to_string()))?;
        for (name, w) in [("variant", &want_v), ("variant_org", &want_o)] {
            let g = &got[name];
            if g.is_null() {
                return Err(ExecError::refused(format!(
                    "task {id}: campaign.{name} missing in the response; not reused"
                )));
            }
            if g != w {
                return Err(ExecError::refused(format!(
                    "task {id}: campaign.{name} {g} != expected {w}; not reused"
                )));
            }
        }
        if let Some(req) = req {
            // 識別の他の欄も POST した値と一致するときだけ採用する（欠落・変更は拒否）。
            let sent = self
                .body(req, "")
                .map_err(ExecError::refused)?
                .get("campaign")
                .cloned()
                .unwrap_or(Value::Null);
            for k in [
                "campaign_id",
                "case_id",
                "arm",
                "repetition",
                "manifest_hash",
                "routing_setting",
            ] {
                if got.get(k) != sent.get(k) {
                    return Err(ExecError::refused(format!(
                        "task {id}: campaign.{k} {} != expected {}; not reused",
                        got.get(k).unwrap_or(&Value::Null),
                        sent.get(k).unwrap_or(&Value::Null)
                    )));
                }
            }
        }
        Ok(())
    }

    /// 試行の木を既存の GET で読み、封印期待値（L2、propagation §8.6）に照合する。読み取りの失敗は中断
    /// （再読できる）。欠落・null・不一致・再起動・非対象 drift は Refused で、その試行は集計しない。
    pub fn prove_variant(
        &self,
        t: &TaskRef,
        exp: &crate::variant_proof::RunProofExpectation<'_>,
    ) -> Result<(), ExecError> {
        let tree = self.walk(&t.task_id).map_err(ExecError::interrupted)?;
        let org = self.get("/api/v1/org").map_err(ExecError::interrupted)?;
        let config = self.get("/api/v1/config").map_err(ExecError::interrupted)?;
        let details: Vec<Value> = tree.into_iter().map(|n| n.detail).collect();
        crate::variant_proof::prove_runs(exp, &details, &org, &config)
            .map_err(|e| ExecError::refused(format!("task {}: {e}", t.task_id)))
    }

    /// 投入前の variant 整合（請求の参照と executor の参照が同一で、固定 selection と一致する）。
    fn check_request_variant(&self, req: &TrialRequest) -> Result<(), String> {
        if req.variant != self.variant.as_ref() {
            return Err(format!(
                "variant mismatch: request {:?} != executor {:?}",
                req.variant, self.variant
            ));
        }
        if let Some(v) = &self.variant {
            v.check_fixed().map_err(|e| e.to_string())?;
        }
        Ok(())
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

    /// POST /tasks の body。識別（`campaign`）と冪等キーを含む。
    pub fn body(&self, req: &TrialRequest, key: &str) -> Result<Value, String> {
        self.check_request_variant(req)?;
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
                "campaign {:?} {} rep{} #{}",
                req.config.id, req.problem_id, req.rep, req.trial_no
            )),
        );
        b.insert(
            "objective".into(),
            json!(format!(
                "{}\n\ncampaign-idempotency-key: {key}\ncluster-job-walltime-secs: {}",
                req.prompt,
                req.timeout_secs.min(req.budget.max_wall_secs)
            )),
        );
        b.insert(
            "workspace".into(),
            json!(req.workspace.to_string_lossy().into_owned()),
        );
        // 作業場所は trial workspace だけ。`repos: []` だけでは `project_id` があると案件の primary repo が補われる
        // （task-ops `resolve_repos`）ので、補完を止める `workspace_mode: shared`（path をそのまま cwd にする）も送る。
        b.insert("repos".into(), json!([]));
        b.insert("workspace_mode".into(), json!("shared"));
        b.insert(
            "max_wall_secs".into(),
            json!(req.timeout_secs.min(req.budget.max_wall_secs)),
        );
        // 試行の token 上限は task の予算として渡す。Celeris が run の途中で消費を観測し、上限後は再開しない
        // （ADR 2026-10-20-mlsys-main2-fixes D1）。`req.budget` は `min(per_case, per_arm の残り)`。
        b.insert("max_tokens".into(), json!(req.budget.max_tokens));
        let ro = routing_override(req.config)?;
        // 採点結果と同じ試行へ export できるよう、通常の作成 API に識別を載せる（task-api NewTaskBody）。
        // 冪等キーは label と同じ文字列。arm は A/B/C、case は問題 ID。
        let mut campaign = Map::new();
        campaign.insert("campaign_id".into(), json!(req.campaign_id));
        campaign.insert("case_id".into(), json!(req.problem_id));
        campaign.insert("arm".into(), json!(format!("{:?}", req.config.id)));
        campaign.insert("repetition".into(), json!(req.rep));
        campaign.insert("manifest_hash".into(), json!(req.manifest_sha256));
        campaign.insert("routing_setting".into(), ro["mode"].clone());
        if let Some(v) = &self.variant {
            let org = self
                .variant_org
                .as_ref()
                .ok_or("variant requires variant_org (the verified org pin)")?;
            campaign.insert(
                "variant".into(),
                serde_json::to_value(CampaignVariantRef::from(v)).map_err(|e| e.to_string())?,
            );
            campaign.insert(
                "variant_org".into(),
                serde_json::to_value(CampaignOrgPin::from(org)).map_err(|e| e.to_string())?,
            );
        }
        b.insert("campaign".into(), Value::Object(campaign));
        b.insert("idempotency_key".into(), json!(idempotency_label(key)));
        b.insert("routing_override".into(), ro);
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

    fn jobs(tree: &[TreeNode]) -> Vec<(String, bool)> {
        tree_jobs(tree)
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

    /// 木の全 run の usage の内訳ごとの合計。input/output の無い run が 1 つでもあれば欠測（None）、
    /// cache の内訳が無い run があればその内訳だけ欠測。
    fn usage(tree: &[TreeNode]) -> (Option<TokenUsage>, Option<u64>) {
        let (mut tokens, mut cost) = (Some(TokenUsage::full(0, 0, 0, 0)), Some(0u64));
        for r in tree
            .iter()
            .flat_map(|n| n.detail["runs"].as_array().into_iter().flatten())
        {
            let u = &r["usage"];
            let t = match (u["input_tokens"].as_u64(), u["output_tokens"].as_u64()) {
                (Some(input), Some(output)) => Some(TokenUsage {
                    input,
                    output,
                    cache_read: u["cache_read_tokens"].as_u64(),
                    cache_creation: u["cache_creation_tokens"].as_u64(),
                }),
                _ => None,
            };
            tokens = tokens.zip(t).map(|(a, b)| a.add(&b));
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
            // 試行の実行時間の上限。job の scheduler 側 walltime はこれを超えて取らない（job は終端確認まで予約を解かない）。
            "walltime_secs": self.trial_timeout_secs,
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

    /// 木の全 run（根・子・孫・再試行）の実測。選択済み（`selection.status = selected`）の run だけが
    /// harness/source/model を持つ。選択前の run は欠測のまま（宣言で埋めない）。
    fn observed_runs(tree: &[TreeNode]) -> Vec<ObservedRun> {
        let text = |v: &Value| v.as_str().map(String::from);
        let mut out = Vec::new();
        for r in tree
            .iter()
            .flat_map(|n| n.detail["runs"].as_array().into_iter().flatten())
        {
            let sel = &r["selection"];
            let chosen = sel["status"] == "selected";
            let rr = &sel["basis"]["route_decision"]["routing_resolved"];
            out.push(ObservedRun {
                run_id: text(&r["run_id"]).unwrap_or_default(),
                // role 欠測は空のまま（worker で補わない）。新形式の checker が違反にする。
                role: text(&r["role"]).unwrap_or_default(),
                harness: chosen.then(|| text(&sel["harness_id"])).flatten(),
                source: chosen.then(|| text(&sel["source_id"])).flatten(),
                model: chosen.then(|| text(&sel["model_id"])).flatten(),
                routing_mode: text(&rr["mode"]),
                routing_source: text(&rr["source"]),
                lower_policy: text(&rr["lower_policy"]),
                evidence_mode: text(&rr["evidence_mode"]),
                evidence_table_id: text(&rr["evidence_table_id"]),
                online_update: rr["online_update"].as_bool(),
            });
        }
        out
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

    fn routing(&self, req: &TrialRequest) -> Result<Value, String> {
        routing_override(req.config)
    }

    /// daemon の `GET /health`（release・単価表・CoS 方針の版）と、worker が起動する Claude Code の実体。
    fn observe_versions(&self, _req: &TrialRequest) -> Result<ObservedVersions, String> {
        let h = self.get("/api/v1/health")?;
        let text = |k: &str| h.get(k).and_then(Value::as_str).map(String::from);
        let claude_code = match &self.claude_code {
            Some(p) => Some(crate::env_lock::pin_program(
                p,
                std::env::var_os("PATH").as_deref(),
            )?),
            None => None,
        };
        Ok(ObservedVersions {
            release: text("release"),
            claude_code,
            pricing_version: text("pricing_version"),
            cos_policy_version: text("cos_policy_version"),
        })
    }

    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        let first = self
            .tasks_with_label(key)
            .map_err(ExecError::interrupted)?
            .into_iter()
            .next();
        if let Some(id) = &first {
            self.check_workspace(id, None)?;
            self.check_existing(id, None)?;
        }
        Ok(first.map(|id| TaskRef {
            task_id: id,
            ..TaskRef::default()
        }))
    }

    fn submit(&self, req: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        // body を作れない（template・設定の不備）のは何も送る前の拒否。
        let body = self.body(req, key).map_err(ExecError::refused)?;
        // 二重起動の防止: 投入直前にも照会する。照会が失敗しても POST はまだ送っていない。
        match self.tasks_with_label(key) {
            Ok(ids) => {
                if let Some(id) = ids.into_iter().next() {
                    self.check_workspace(&id, Some(req))?;
                    self.check_existing(&id, Some(req))?;
                    return Ok(TaskRef {
                        task_id: id,
                        ..TaskRef::default()
                    });
                }
            }
            Err(e) => return Err(ExecError::not_sent(e)),
        }
        // POST の失敗（応答なし）は、要求が届いたか不明。
        let r = self
            .transport
            .request("POST", "/api/v1/tasks", Some(&body))
            .map_err(ExecError::unknown)?;
        if !ok(&r) {
            let msg = format!("POST /api/v1/tasks: HTTP {} {}", r.status, r.body);
            // 4xx は server が受理しなかった確定。5xx は処理後に落ちた可能性がある。
            return Err(if (400..500).contains(&r.status) {
                ExecError::refused(msg)
            } else {
                ExecError::unknown(msg)
            });
        }
        let created = r.body["id"]
            .as_str()
            .ok_or_else(|| ExecError::unknown("POST /api/v1/tasks: id missing"))?
            .to_string();
        // 照会と投入の間に別の起動が投入していたら、古い方を正とし、新しい方を閉じる。
        let ids = self.tasks_with_label(key).map_err(ExecError::unknown)?;
        let first = ids.first().cloned().unwrap_or_else(|| created.clone());
        // 自分が POST した task は送った値のまま（workspace・repos は照会しない）。別の起動が作った task だけ確かめる。
        let adopted = if first == created {
            Ok(())
        } else {
            self.check_workspace(&first, Some(req))
        };
        if let Err(e) = adopted.and_then(|()| self.check_existing(&first, Some(req))) {
            // 応答に variant が無い／違う task は適用済みとみなさない。自分が作った task なら閉じる。
            if first == created {
                let _ = self.cancel_task(&created);
            }
            return Err(e);
        }
        if first != created {
            self.cancel_task(&created).map_err(ExecError::unknown)?;
        }
        Ok(TaskRef {
            task_id: first,
            ..TaskRef::default()
        })
    }

    fn track(&self, t: &TaskRef) -> Result<TaskRef, ExecError> {
        let tree = self.walk(&t.task_id).map_err(ExecError::interrupted)?;
        Ok(Self::task_ref(t, &tree))
    }

    /// 実行中の累積（子 task を含む全 run の usage の内訳と、task 作成からの server 時計の経過）。
    fn usage(&self, t: &TaskRef) -> Result<Option<Usage>, ExecError> {
        let tree = self.walk(&t.task_id).map_err(ExecError::interrupted)?;
        let root = tree
            .first()
            .ok_or_else(|| ExecError::interrupted("empty task tree"))?;
        let elapsed = secs_between(
            parse_ts(&root.detail["task"]["created_at"]),
            parse_ts(&root.detail["timers"]["now"]),
        );
        Ok(Some(Usage {
            tokens: Self::usage(&tree).0,
            wall_secs: elapsed.unwrap_or(0),
        }))
    }

    fn observed(&self, t: &TaskRef) -> Result<Vec<ObservedRun>, String> {
        Ok(Self::observed_runs(&self.walk(&t.task_id)?))
    }

    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, ExecError> {
        let tree = self.walk(&t.task_id).map_err(ExecError::interrupted)?;
        let root = tree
            .first()
            .ok_or_else(|| ExecError::interrupted("empty task tree"))?;
        let created = parse_ts(&root.detail["task"]["created_at"]);
        let elapsed = secs_between(created, parse_ts(&root.detail["timers"]["now"]));
        let pending = tree.iter().any(|n| !is_terminal(&n.status))
            || Self::jobs(&tree).iter().any(|(_, live)| *live);
        let (tokens, cost_micro_usd) = Self::usage(&tree);
        if pending {
            if elapsed.is_some_and(|e| e > self.trial_timeout_secs) {
                self.cancel(&Self::task_ref(t, &tree), "trial_timeout")?;
                // cancel の受理は停止ではない。task と job の終端を読み直して確かめるまで Wait（予約も解かない）。
                let after = self.walk(&t.task_id).map_err(ExecError::interrupted)?;
                if !all_stopped(&after) {
                    return Ok(ExecStatus::Wait);
                }
                let (tokens, cost_micro_usd) = Self::usage(&after);
                return Ok(ExecStatus::Done(ExecResult {
                    outcome: Outcome::Timeout,
                    tokens,
                    cost_micro_usd,
                    wall_secs: self.trial_timeout_secs,
                }));
            }
            return Ok(ExecStatus::Wait);
        }
        // 前回の poll が timeout で cancel した後、job の終了を待って終端に至った場合。
        if root.status == "cancelled" && elapsed.is_some_and(|e| e > self.trial_timeout_secs) {
            return Ok(ExecStatus::Done(ExecResult {
                outcome: Outcome::Timeout,
                tokens,
                cost_micro_usd,
                wall_secs: self.trial_timeout_secs,
            }));
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

    /// task 木がすべて終端で、cluster job も終了（F / 終端 / gone）するまで false。
    fn stopped(&self, t: &TaskRef) -> Result<bool, ExecError> {
        let tree = self.walk(&t.task_id).map_err(ExecError::interrupted)?;
        Ok(all_stopped(&tree))
    }

    /// 子孫（深い順）→ 根の順に終端でない task を cancel API で閉じ、cluster job は人手手順を記録する。
    /// 失敗があっても全対象を試してから Err を返す。
    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), ExecError> {
        let mut errors = Vec::new();
        let (tree, mut jobs) = match self.walk(&t.task_id) {
            Ok(tree) => {
                // 所有判定: 根が本 campaign の label を持たない task は止めない（job も記録しない）。
                if !tree.first().is_some_and(owned_by_campaign) {
                    return Err(ExecError::refused(format!(
                        "task {} is not owned by this campaign (no campaign-… label); not cancelled",
                        t.task_id
                    )));
                }
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
            Err(ExecError::interrupted(errors.join("; ")))
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
