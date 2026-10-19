//! 最終報告の消費集計: campaign 表（外側の trial 記録）と decision-log export（record-contract の正式入力、
//! `records.jsonl`）を campaign/case/arm/repetition/hash で結合し、重複排除済みの消費と欠測を出す。
//!
//! - 既存 writer は使わない。入力は export の `usage` / `run` / `trial` record の JSON だけ。
//! - 外側 trial の合計と、同じ run の decision-log 消費は**加算しない**。照合だけする（一致 / 不一致 / 内訳なし）。
//!   run 内訳が無い旧記録は外側の値のまま `undecomposed` に置き、推測で分解しない。
//! - run は各 1 回、Jev/CoS の呼び出しは各 call 1 回（`dedup_key`）。retry は別の切り口（`by_retry.additive=false`）で、
//!   `by_role` に入っている同じ費用を再加算しない。
//! - 帰属できない共有は `shared_unattributable`、未取得/欠落は coverage と `gaps`。実際に呼ばれなかった 0 は
//!   根拠（`basis`）を持つときだけ `not_called`。根拠が無ければ欠測。
//! - token は manifest の `token_basis`（cache_read / cache_creation を含むか）で数え、内訳も別に出す。
//!   定価換算・実支出・executor 報告費用・quota は別項目で、互いに代用しない。
//! - 完全でない和は `observed_sum` に出し、`total` は完全なときだけ値を持つ。欠測があれば `reduction_confirmable` は false。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Value, json};

use crate::{
    checker::check,
    ledger::LedgerRow,
    manifest::{CampaignManifest, TokenBasis},
    record::{TRIALS_FILE, TrialRecord},
};

pub const SCHEMA: &str = "celeris.campaign-final-accounting/1";
const RECORD_SCHEMA: &str = "celeris.decision-log/1";
const METRICS: [&str; 3] = ["tokens", "list_cost_usd", "cash_cost_usd"];

pub struct FinalInputs<'a> {
    /// decision-log export の `records.jsonl`。
    pub records_jsonl: &'a str,
    /// CoS を呼ばなかったと言える根拠。無ければ CoS は未取得（欠測）として扱う。
    pub cos_not_called_basis: Option<&'a str>,
    /// `celeris-campaign usage` の報告など。quota は消費行に載らないので別項目として通すだけ。
    pub quota: Option<Value>,
}

/// 4 つ組の欄。観測 0 件の和は null（0 ではない）。
#[derive(Clone, Debug, Default)]
struct Stat {
    sum: u64,
    obs: u64,
    miss: u64,
}

impl Stat {
    fn add(&mut self, v: Option<u64>) {
        match v {
            Some(x) => {
                self.sum += x;
                self.obs += 1;
            }
            None => self.miss += 1,
        }
    }
    fn json(&self) -> Value {
        json!({"sum": (self.obs > 0).then_some(self.sum), "n_observed": self.obs, "n_missing": self.miss})
    }
    fn complete(&self) -> bool {
        self.miss == 0
    }
}

/// 消費の測定値（区分は別欄。互いに足さない）。
#[derive(Clone, Debug, Default)]
struct Meas {
    n_rows: u64,
    input: Stat,
    output: Stat,
    cache_read: Stat,
    cache_creation: Stat,
    /// manifest の token_basis で数えた tokens（基準に要る欄が欠ければ欠測）。
    basis_tokens: Stat,
    list_cost_micro_usd: Stat,
    cash_cost_micro_usd: Stat,
}

impl Meas {
    fn add(&mut self, u: &Usage, basis: TokenBasis) {
        self.n_rows += 1;
        self.input.add(u.input);
        self.output.add(u.output);
        self.cache_read.add(u.cache_read);
        self.cache_creation.add(u.cache_creation);
        self.basis_tokens.add(u.basis_tokens(basis));
        self.list_cost_micro_usd.add(u.list_micro);
        self.cash_cost_micro_usd.add(u.cash_micro);
    }
    fn stat(&self, metric: &str) -> &Stat {
        match metric {
            "tokens" => &self.basis_tokens,
            "list_cost_usd" => &self.list_cost_micro_usd,
            _ => &self.cash_cost_micro_usd,
        }
    }
    fn tokens_json(&self) -> Value {
        json!({
            "n_rows": self.n_rows,
            "input_tokens": self.input.json(),
            "output_tokens": self.output.json(),
            "cache_read_tokens": self.cache_read.json(),
            "cache_creation_tokens": self.cache_creation.json(),
            "basis_tokens": self.basis_tokens.json(),
        })
    }
    fn json(&self) -> Value {
        json!({
            "n_rows": self.n_rows,
            "input_tokens": self.input.json(),
            "output_tokens": self.output.json(),
            "cache_read_tokens": self.cache_read.json(),
            "cache_creation_tokens": self.cache_creation.json(),
            "basis_tokens": self.basis_tokens.json(),
            "list_cost_micro_usd": self.list_cost_micro_usd.json(),
            "cash_cost_micro_usd": self.cash_cost_micro_usd.json(),
        })
    }
}

/// 消費行 1 件（export の usage record から読む）。
#[derive(Clone, Debug)]
struct Usage {
    role: String,
    is_retry: bool,
    shared: bool,
    run_id: Option<String>,
    task_id: Option<String>,
    arm: Option<String>,
    case_id: Option<String>,
    rep: Option<u32>,
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_creation: Option<u64>,
    list_micro: Option<u64>,
    cash_micro: Option<u64>,
}

impl Usage {
    fn basis_tokens(&self, basis: TokenBasis) -> Option<u64> {
        let io = self.input? + self.output?;
        match basis {
            TokenBasis::InputOutput => Some(io),
            TokenBasis::InputOutputCacheRead => Some(io + self.cache_read?),
            TokenBasis::All => Some(io + self.cache_read? + self.cache_creation?),
        }
    }
}

fn text(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().map(String::from)
}

fn micro(v: &Value, k: &str) -> Option<u64> {
    let x = v[k].as_f64()?;
    (x.is_finite() && x >= 0.0).then(|| (x * 1e6).round() as u64)
}

fn parse_usage(v: &Value) -> Usage {
    let u = |k: &str| v[k].as_u64();
    Usage {
        role: text(v, "role").unwrap_or_default(),
        is_retry: v["is_retry"].as_bool().unwrap_or(false),
        shared: v["shared_overhead"].as_bool().unwrap_or(false),
        run_id: text(v, "run_id").or_else(|| text(&v["usage_key"], "run_id")),
        task_id: text(v, "task_id"),
        arm: text(v, "arm"),
        case_id: text(v, "case_id"),
        rep: v["repetition"].as_u64().map(|x| x as u32),
        input: u("input_tokens"),
        output: u("output_tokens"),
        cache_read: u("cache_read_tokens"),
        cache_creation: u("cache_creation_tokens"),
        list_micro: micro(v, "list_cost_usd"),
        cash_micro: micro(v, "cash_cost_usd"),
    }
}

type CellKey = (String, String, u32);

#[derive(Default)]
struct Cell {
    ran: bool,
    outer_tokens: Option<u64>,
    outer_tokens_known: bool,
    runs: BTreeSet<String>,
    inner: Meas,
    inner_missing_runs: u64,
    tasks: BTreeSet<String>,
    classifiers: BTreeSet<String>,
    jev_rows: Meas,
}

struct Gap {
    metric: &'static str,
    code: String,
    detail: String,
}

fn gap(metric: &'static str, code: &str, detail: impl Into<String>) -> Gap {
    Gap {
        metric,
        code: code.into(),
        detail: detail.into(),
    }
}

struct RunInfo {
    cell: Option<CellKey>,
    task_id: Option<String>,
    classifier: Option<String>,
}

fn read_trials(out: &Path) -> Result<Vec<TrialRecord>, String> {
    let p = out.join(TRIALS_FILE);
    let t = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    t.lines()
        .map(|l| serde_json::from_str::<TrialRecord>(l).map_err(|e| e.to_string()))
        .collect()
}

/// 台帳の未完了行（予約）。読むだけで、末尾の不完全な行も直さない。
/// 台帳は状態遷移ごとに行を追記するので、鍵ごとに最後の行だけを見る（途中の状態を予約として数えない）。
fn unfinished_rows(out: &Path, hash: &str) -> Vec<LedgerRow> {
    let Ok(t) = fs::read_to_string(out.join(crate::ledger::LEDGER_FILE)) else {
        return Vec::new();
    };
    let keep = t.rfind('\n').map_or(0, |i| i + 1);
    let mut latest = BTreeMap::new();
    for r in t[..keep]
        .lines()
        .filter_map(|l| serde_json::from_str::<LedgerRow>(l).ok())
    {
        latest.insert(r.key.clone(), r);
    }
    latest
        .into_values()
        .filter(|r| r.key.manifest_sha256 == hash && !r.state.is_terminal())
        .collect()
}

/// 同じ鍵の行を 1 件に潰す。内容が全て一致なら重複、食い違いなら鍵ごと採らない（順序に依らない）。
fn dedup<T: Clone>(
    rows: BTreeMap<String, Vec<(Value, T)>>,
    dup: &mut u64,
    conflicts: &mut Vec<String>,
) -> BTreeMap<String, T> {
    let mut out = BTreeMap::new();
    for (k, v) in rows {
        let Some((first, t)) = v.first() else {
            continue;
        };
        if v.iter().all(|(raw, _)| raw == first) {
            *dup += v.len() as u64 - 1;
            out.insert(k, t.clone());
        } else {
            conflicts.push(k);
        }
    }
    out
}

pub fn compute(m: &CampaignManifest, out: &Path, inp: &FinalInputs) -> Result<Value, String> {
    let rep = check(m, out);
    if !rep.ok {
        return Err(format!(
            "checker rejected the records: {}",
            rep.violations.join("; ")
        ));
    }
    let hash = m.compute_sha256()?;
    let basis = m.budget.token_basis;
    let mut gaps: Vec<Gap> = Vec::new();
    // 記録の無い cell（未完了）の消費は分からない。採点の欠落は成否の話で、消費の欠測ではない。
    let no_record: u64 = rep
        .recomputed
        .iter()
        .flat_map(|rc| rc.totals.values())
        .map(|t| t.missing)
        .sum();
    if no_record > 0 {
        gaps.push(gap(
            "all",
            "trials_without_record",
            format!("{no_record} planned trial(s) have no record; their consumption is unknown"),
        ));
    }

    // 外側 trial: (arm, problem, rep) ごとに、再試行を含む全記録を 1 回ずつ。
    let trials = read_trials(out)?;
    let mut cells: BTreeMap<CellKey, Cell> = BTreeMap::new();
    let mut outer_total = Meas::default();
    let mut outer_cost = Stat::default();
    let mut runtime_charged = 0u64;
    let mut outer_missing_trials = 0u64;
    let mut run_to_cell: BTreeMap<String, CellKey> = BTreeMap::new();
    for r in &trials {
        let key = (format!("{:?}", r.config), r.problem_id.clone(), r.rep);
        let c = cells.entry(key.clone()).or_default();
        for o in &r.observed {
            run_to_cell.insert(o.run_id.clone(), key.clone());
        }
        if !r.ran() {
            continue;
        }
        c.ran = true;
        runtime_charged += r.charged_tokens(&m.budget);
        let t = r.tokens;
        let first = !c.outer_tokens_known;
        c.outer_tokens = match (first, c.outer_tokens, t) {
            (true, _, t) => t,
            (false, Some(a), Some(b)) => Some(a + b),
            _ => None,
        };
        c.outer_tokens_known = true;
        if t.is_none() {
            outer_missing_trials += 1;
        }
        let u = r.token_usage;
        let row = Usage {
            role: String::new(),
            is_retry: false,
            shared: false,
            run_id: None,
            task_id: None,
            arm: None,
            case_id: None,
            rep: None,
            input: u.map(|x| x.input),
            output: u.map(|x| x.output),
            cache_read: u.and_then(|x| x.cache_read),
            cache_creation: u.and_then(|x| x.cache_creation),
            list_micro: None,
            cash_micro: None,
        };
        // 外側の tokens は内訳を予算基準で数えた値（checker が一致を確認済み）。内訳が無ければ欠測。
        outer_total.add(&row, basis);
        outer_cost.add(r.cost_micro_usd);
    }
    if outer_missing_trials > 0 {
        gaps.push(gap(
            "tokens",
            "trial_tokens_missing",
            format!("{outer_missing_trials} executed trial(s) have no token usage"),
        ));
    }

    // decision-log の record を読む。
    let mut malformed = 0u64;
    let mut excluded_campaign = 0u64;
    let mut excluded_hash = 0u64;
    let mut usage_in: BTreeMap<String, Vec<(Value, Usage)>> = BTreeMap::new();
    let mut run_in: BTreeMap<String, Vec<(Value, Value)>> = BTreeMap::new();
    let mut trial_rec: BTreeMap<String, Value> = BTreeMap::new();
    for line in inp.records_jsonl.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            malformed += 1;
            continue;
        };
        let kind = v["record_type"].as_str().unwrap_or("");
        if !matches!(kind, "usage" | "run" | "trial") {
            continue;
        }
        if v["schema"] != RECORD_SCHEMA {
            malformed += 1;
            continue;
        }
        if v["campaign_id"].as_str() != Some(m.campaign_id.as_str()) {
            excluded_campaign += 1;
            continue;
        }
        if v["manifest_hash"].as_str().is_some_and(|h| h != hash) {
            excluded_hash += 1;
            continue;
        }
        let dk = text(&v, "dedup_key").unwrap_or_default();
        match kind {
            "usage" => usage_in
                .entry(dk)
                .or_default()
                .push((v.clone(), parse_usage(&v))),
            "run" => run_in
                .entry(text(&v, "run_id").unwrap_or(dk))
                .or_default()
                .push((v.clone(), v)),
            _ => {
                if let Some(id) = text(&v, "run_id") {
                    trial_rec.insert(id, v);
                }
            }
        }
    }
    if malformed > 0 {
        gaps.push(gap(
            "all",
            "records_unparsable",
            format!("{malformed} record line(s) are unparsable or have an unknown schema"),
        ));
    }

    let mut dup = 0u64;
    let mut conflicts: Vec<String> = Vec::new();
    let usage = dedup(usage_in, &mut dup, &mut conflicts);
    let runs = dedup(run_in, &mut dup, &mut conflicts);
    conflicts.sort();
    if !conflicts.is_empty() {
        gaps.push(gap(
            "all",
            "conflicting_duplicates",
            format!(
                "conflicting rows were not counted: {}",
                conflicts.join(", ")
            ),
        ));
    }

    // run → 結合する cell。手がかり（arm/case/rep）が計画に無い run は計画外として別枠。
    let planned_arms: BTreeSet<String> = m.active_arms().iter().map(|c| format!("{c:?}")).collect();
    let mut info: BTreeMap<String, RunInfo> = BTreeMap::new();
    let mut outside_plan: Vec<String> = Vec::new();
    for (id, r) in &runs {
        let (arm, case, rp) = (
            text(r, "arm"),
            text(r, "case_id"),
            r["repetition"].as_u64().map(|x| x as u32),
        );
        let mut cell = match (arm, case, rp) {
            (Some(a), Some(c), Some(p))
                if planned_arms.contains(&a) && (1..=m.repeats).contains(&p) =>
            {
                Some((a, c, p))
            }
            _ => None,
        };
        if let Some(k) = &cell
            && !cells.contains_key(k)
        {
            cell = None;
        }
        // 外側が観測した run id が別の cell に属すなら、識別が矛盾する。
        if let (Some(k), Some(o)) = (&cell, run_to_cell.get(id))
            && k != o
        {
            cell = None;
        }
        if cell.is_none() {
            outside_plan.push(id.clone());
        }
        info.insert(
            id.clone(),
            RunInfo {
                cell,
                task_id: text(r, "task_id"),
                classifier: trial_rec.get(id).and_then(|t| text(t, "classifier_kind")),
            },
        );
    }
    if !outside_plan.is_empty() {
        gaps.push(gap(
            "all",
            "runs_outside_plan",
            format!(
                "run(s) whose case/arm/repetition is not a planned cell were not joined: {}",
                outside_plan.join(", ")
            ),
        ));
    }
    for (id, i) in &info {
        if let Some(k) = &i.cell {
            let c = cells.entry(k.clone()).or_default();
            c.runs.insert(id.clone());
            if let Some(t) = &i.task_id {
                c.tasks.insert(t.clone());
            }
            if let Some(cl) = &i.classifier {
                c.classifiers.insert(cl.clone());
            }
        }
    }
    let mut task_cells: BTreeMap<String, BTreeSet<CellKey>> = BTreeMap::new();
    for i in info.values() {
        if let (Some(k), Some(t)) = (&i.cell, &i.task_id) {
            task_cells.entry(t.clone()).or_default().insert(k.clone());
        }
    }

    // 消費行の振り分け。
    let mut by_role: BTreeMap<String, Meas> = ["worker", "planner", "reviewer"]
        .iter()
        .map(|r| (r.to_string(), Meas::default()))
        .collect();
    let mut by_retry: BTreeMap<&str, Meas> =
        BTreeMap::from([("initial", Meas::default()), ("retry", Meas::default())]);
    let mut run_rows: BTreeMap<String, Vec<&Usage>> = BTreeMap::new();
    let mut jev = Meas::default();
    let mut shared = Meas::default();
    let mut cos = Meas::default();
    let mut unjoined: Vec<String> = Vec::new();
    let mut identity_conflicts: Vec<String> = Vec::new();
    let mut unattributed: Vec<String> = Vec::new();
    let mut jev_unmapped: Vec<String> = Vec::new();
    for (k, u) in &usage {
        if u.shared {
            shared.add(u, basis);
            if u.role == "cos" {
                cos.add(u, basis);
            }
        } else if u.role == "jev" {
            jev.add(u, basis);
            let cs = u.task_id.as_ref().and_then(|t| task_cells.get(t));
            match cs {
                Some(s) if s.len() == 1 => {
                    if let Some(c) = s.iter().next().and_then(|key| cells.get_mut(key)) {
                        c.jev_rows.add(u, basis);
                    }
                }
                _ => jev_unmapped.push(k.clone()),
            }
        } else if let Some(rid) = &u.run_id {
            match info.get(rid) {
                None => unjoined.push(k.clone()),
                Some(i) => {
                    let id_ok = match &i.cell {
                        Some((a, c, p)) => {
                            u.arm.as_ref().is_none_or(|x| x == a)
                                && u.case_id.as_ref().is_none_or(|x| x == c)
                                && u.rep.is_none_or(|x| x == *p)
                        }
                        None => true,
                    };
                    if !id_ok {
                        identity_conflicts.push(k.clone());
                    } else if i.cell.is_some() {
                        run_rows.entry(rid.clone()).or_default().push(u);
                    }
                }
            }
        } else {
            unattributed.push(k.clone());
        }
    }
    for (code, v, what) in [
        (
            "usage_without_run",
            &unjoined,
            "usage rows whose run record is not in the export (late observation) were not joined",
        ),
        (
            "identity_conflict",
            &identity_conflicts,
            "usage rows whose case/arm/repetition disagree with their run were not counted",
        ),
        (
            "usage_unattributed",
            &unattributed,
            "non-shared usage rows with neither a run nor a Jev call were not counted",
        ),
    ] {
        if !v.is_empty() {
            gaps.push(gap("all", code, format!("{what}: {}", v.join(", "))));
        }
    }
    if !jev_unmapped.is_empty() {
        // 総和には入れるが、どの cell の呼び出しか割れない。
        gaps.push(gap(
            "all",
            "jev_call_unmapped",
            format!(
                "Jev calls not mappable to one cell: {}",
                jev_unmapped.join(", ")
            ),
        ));
    }

    // run ごとに 1 回だけ数える。同じ run に複数行（別 source）があるときは二重の恐れがあるので数えない。
    let mut ambiguous: Vec<String> = Vec::new();
    let mut without_usage: Vec<String> = Vec::new();
    for (rid, i) in &info {
        let Some(k) = &i.cell else { continue };
        let rows = run_rows.get(rid).map(Vec::as_slice).unwrap_or(&[]);
        let c = cells.entry(k.clone()).or_default();
        match rows {
            [] => {
                c.inner_missing_runs += 1;
                without_usage.push(rid.clone());
            }
            [u] => {
                by_role.entry(u.role.clone()).or_default().add(u, basis);
                by_retry
                    .entry(if u.is_retry { "retry" } else { "initial" })
                    .or_default()
                    .add(u, basis);
                c.inner.add(u, basis);
            }
            _ => {
                c.inner_missing_runs += 1;
                ambiguous.push(rid.clone());
            }
        }
    }
    for (code, v, what) in [
        (
            "run_without_usage",
            &without_usage,
            "run(s) have no usage row; not guessed as 0",
        ),
        (
            "run_multiple_usage_sources",
            &ambiguous,
            "run(s) have several usage rows and could be double counted; not counted",
        ),
    ] {
        if !v.is_empty() {
            gaps.push(gap("all", code, format!("{what}: {}", v.join(", "))));
        }
    }

    // 外側と内側の照合（加算しない）と cell ごとの Jev coverage。
    let mut cell_rows: Vec<Value> = Vec::new();
    let mut undecomposed = Stat::default();
    let mut undecomposed_cells = 0u64;
    let mut reconcile_counts: BTreeMap<&str, u64> = BTreeMap::new();
    let mut jev_cov: BTreeMap<&str, u64> = BTreeMap::new();
    let mut list_cash_blocked: Vec<String> = Vec::new();
    for ((arm, case, rp), c) in &cells {
        let inner_tokens =
            if c.runs.is_empty() || c.inner_missing_runs > 0 || !c.inner.basis_tokens.complete() {
                None
            } else {
                Some(c.inner.basis_tokens.sum)
            };
        let tag = format!("{arm}/{case}/rep{rp}");
        let state = if !c.ran {
            if c.runs.is_empty() {
                "not_executed"
            } else {
                "inner_without_trial"
            }
        } else if c.runs.is_empty() {
            "no_breakdown"
        } else {
            match (c.outer_tokens, inner_tokens) {
                (Some(o), Some(i)) if o == i => "match",
                (Some(_), Some(_)) => "mismatch",
                _ => "incomplete",
            }
        };
        *reconcile_counts.entry(state).or_default() += 1;
        match state {
            "mismatch" | "incomplete" | "inner_without_trial" => gaps.push(gap(
                "tokens",
                &format!("reconcile_{state}"),
                format!(
                    "{tag}: trial tokens {:?} vs decision-log runs {:?}",
                    c.outer_tokens, inner_tokens
                ),
            )),
            "no_breakdown" => {
                undecomposed_cells += 1;
                undecomposed.add(c.outer_tokens);
                list_cash_blocked.push(tag.clone());
            }
            _ => {}
        }
        // Jev: 行があればその状態、無ければ「呼ばれなかった」根拠、根拠が無ければ欠測。
        let cfg = m.configs.iter().find(|x| format!("{:?}", x.id) == *arm);
        let (jstate, jbasis): (&str, Option<String>) = if c.jev_rows.n_rows > 0 {
            if c.jev_rows.basis_tokens.obs
                + c.jev_rows.list_cost_micro_usd.obs
                + c.jev_rows.cash_cost_micro_usd.obs
                > 0
            {
                ("observed", None)
            } else {
                ("missing", None)
            }
        } else if !c.ran {
            ("not_called", Some("cell_not_executed".into()))
        } else if cfg.is_some_and(|x| x.routing.is_none()) {
            ("not_called", Some("arm_without_celeris_routing".into()))
        } else if arm == "B" {
            ("not_called", Some("fixed_routing".into()))
        } else if c.classifiers.is_empty() {
            ("unknown", None)
        } else if c.classifiers.contains("jev") {
            ("unretrieved", None)
        } else {
            (
                "not_called",
                Some(format!(
                    "classifier_{}",
                    c.classifiers.iter().cloned().collect::<Vec<_>>().join("+")
                )),
            )
        };
        *jev_cov.entry(jstate).or_default() += 1;
        if matches!(jstate, "missing" | "unretrieved" | "unknown") {
            gaps.push(gap(
                "all",
                &format!("jev_{jstate}"),
                format!("{tag}: Jev consumption is {jstate}, not 0"),
            ));
        }
        cell_rows.push(json!({
            "arm": arm, "case_id": case, "repetition": rp, "ran": c.ran,
            "trial_tokens": c.outer_tokens, "decision_log_tokens": inner_tokens,
            "decision_log_runs": c.runs.len(), "reconcile": state,
            "jev": {"state": jstate, "basis": jbasis},
        }));
    }
    if !list_cash_blocked.is_empty() {
        for metric in ["list_cost_usd", "cash_cost_usd"] {
            gaps.push(gap(
                metric,
                "no_run_breakdown",
                format!(
                    "cells without decision-log runs have no {metric}: {}",
                    list_cash_blocked.join(", ")
                ),
            ));
        }
    }

    // CoS（共有。campaign 表には足さない）。
    let (cos_state, cos_basis): (&str, Option<String>) = if cos.n_rows > 0 {
        if cos.basis_tokens.obs + cos.list_cost_micro_usd.obs + cos.cash_cost_micro_usd.obs > 0 {
            ("observed", None)
        } else {
            ("missing", None)
        }
    } else if let Some(b) = inp.cos_not_called_basis {
        ("not_called", Some(b.to_string()))
    } else {
        ("not_retrieved", None)
    };
    if matches!(cos_state, "missing" | "not_retrieved") {
        gaps.push(gap(
            "all",
            &format!("cos_{cos_state}"),
            format!("CoS consumption is {cos_state}, not 0"),
        ));
    }

    // 指標ごとの欠測（行はあるが欄が欠測）。
    let runs_meas = sum_meas(by_role.values());
    for metric in METRICS {
        let lanes: [(&str, &Meas); 3] = [
            ("jev", &jev),
            ("shared_unattributable", &shared),
            ("runs", &runs_meas),
        ];
        for (name, mm) in lanes {
            let s = mm.stat(metric);
            if !s.complete() {
                gaps.push(gap(
                    metric,
                    "metric_missing",
                    format!("{name}: {} row(s) have no {metric}", s.miss),
                ));
            }
        }
    }

    // 総和。trial は外側の値、Jev は call ごと。list/cash は decision-log の run と Jev。
    let observed_tokens = outer_total.basis_tokens.sum + jev.basis_tokens.sum;
    let observed_list = runs_meas.list_cost_micro_usd.sum + jev.list_cost_micro_usd.sum;
    let observed_cash = runs_meas.cash_cost_micro_usd.sum + jev.cash_cost_micro_usd.sum;
    // 実行中の予算の見え方と最終 cutoff。
    let held: Vec<(u64, u64)> = unfinished_rows(out, &hash)
        .iter()
        .map(|r| crate::budget::hold_of(r, &m.budget))
        .collect();
    let held_tokens: u64 = held.iter().map(|h| h.0).sum();
    let observed_trial_tokens = outer_total.basis_tokens.sum;
    let budget = json!({
        "token_basis": basis,
        "max_tokens": m.budget.max_tokens,
        "runtime_view": {
            "charged_tokens": runtime_charged,
            "missing_substituted_tokens": runtime_charged - observed_trial_tokens.min(runtime_charged),
            "reserved_in_flight_tokens": held_tokens,
            "in_flight_rows": held.len(),
            "snapshot_tokens": runtime_charged + held_tokens,
            "note": "running admission charges a missing trial at per_case.max_tokens and holds a reservation for in-flight rows",
        },
        "final_cutoff": {
            "observed_trial_tokens": observed_trial_tokens,
            "missing_trials": outer_missing_trials,
            "unreleased_reservations": held.len(),
            "note": "final accounting counts observed tokens only; a missing trial stays missing and is never replaced by the cap",
        },
        "difference": {
            "runtime_charged_minus_observed_tokens": runtime_charged as i128 - observed_trial_tokens as i128,
            "reservation_tokens_not_in_final": held_tokens,
        },
    });
    if !held.is_empty() {
        gaps.push(gap(
            "all",
            "ledger_unfinished",
            format!("{} ledger row(s) still hold a reservation", held.len()),
        ));
    }

    let mut completeness = BTreeMap::new();
    for metric in METRICS {
        let ok = !gaps.iter().any(|g| g.metric == "all" || g.metric == metric);
        completeness.insert(metric, ok);
    }
    let pick = |metric: &str, v: u64| completeness[metric].then_some(v);
    gaps.sort_by(|a, b| (a.metric, &a.code, &a.detail).cmp(&(b.metric, &b.code, &b.detail)));

    let by_role_json: BTreeMap<&String, Value> =
        by_role.iter().map(|(k, v)| (k, v.json())).collect();
    let mut reduction = BTreeMap::new();
    for metric in METRICS {
        reduction.insert(metric, completeness[metric]);
    }
    let gaps_json: Vec<Value> = gaps
        .iter()
        .map(|g| json!({"metric": g.metric, "code": g.code, "detail": g.detail}))
        .collect();
    Ok(json!({
        "schema": SCHEMA,
        "campaign_id": m.campaign_id,
        "manifest_hash": hash,
        "token_basis": basis,
        "integrity": {
            "duplicates_identical": dup,
            "conflicts": conflicts,
            "identity_conflicts": identity_conflicts,
            "excluded_other_campaign": excluded_campaign,
            "excluded_manifest_mismatch": excluded_hash,
            "runs_outside_plan": outside_plan,
            "usage_without_run": unjoined,
            "run_multiple_usage_sources": ambiguous,
            "run_without_usage": without_usage,
        },
        "trials": {
            "outer": outer_total.tokens_json(),
            "executor_reported_cost_micro_usd": outer_cost.json(),
            "cells": cell_rows,
            "reconcile": reconcile_counts,
            "note": "outer trial totals and decision-log run usage describe the same runs; they are reconciled, never added",
        },
        "by_role": by_role_json,
        "by_retry": {
            "additive": false,
            "note": "retry is a tag over the runs already in by_role; it is not added again",
            "initial": by_retry["initial"].json(),
            "retry": by_retry["retry"].json(),
        },
        "undecomposed": {
            "cells": undecomposed_cells,
            "tokens": undecomposed.json(),
            "note": "trials without decision-log run usage keep their outer total; not split by guess",
        },
        "jev": {"rows": jev.json(), "cells": jev_cov},
        "shared_unattributable": {
            "rows": shared.json(),
            "added_to_total": false,
            "cos": {"state": cos_state, "basis": cos_basis, "rows": cos.json()},
        },
        "observed_sum": {
            "tokens": observed_tokens,
            "list_cost_micro_usd": observed_list,
            "cash_cost_micro_usd": observed_cash,
            "note": "sum of what was observed; not a complete total unless total.complete is true",
        },
        "total": {
            "complete": completeness,
            "tokens": pick("tokens", observed_tokens),
            "list_cost_micro_usd": pick("list_cost_usd", observed_list),
            "cash_cost_micro_usd": pick("cash_cost_usd", observed_cash),
        },
        "reduction_confirmable": reduction,
        "gaps": gaps_json,
        "quota": match &inp.quota {
            Some(q) => json!({"state": "observed", "value": q}),
            None => json!({"state": "not_observed", "value": null}),
        },
        "budget": budget,
    }))
}

fn sum_meas<'a>(it: impl Iterator<Item = &'a Meas>) -> Meas {
    let mut t = Meas::default();
    for m in it {
        t.n_rows += m.n_rows;
        for (a, b) in [
            (&mut t.input, &m.input),
            (&mut t.output, &m.output),
            (&mut t.cache_read, &m.cache_read),
            (&mut t.cache_creation, &m.cache_creation),
            (&mut t.basis_tokens, &m.basis_tokens),
            (&mut t.list_cost_micro_usd, &m.list_cost_micro_usd),
            (&mut t.cash_cost_micro_usd, &m.cash_cost_micro_usd),
        ] {
            a.sum += b.sum;
            a.obs += b.obs;
            a.miss += b.miss;
        }
    }
    t
}

fn n(v: &Value) -> Option<u64> {
    v.as_u64()
}

/// 出力だけから既知量を再計算して検査する（checker）。違反の一覧（空なら整合）。
pub fn recheck(acct: &Value) -> Vec<String> {
    let mut v = Vec::new();
    if acct["schema"] != SCHEMA {
        v.push("final accounting: unknown schema".to_string());
        return v;
    }
    // 外側 trial の合計 = cell の合計。
    let cells = acct["trials"]["cells"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let cell_sum: u64 = cells.iter().filter_map(|c| n(&c["trial_tokens"])).sum();
    if n(&acct["trials"]["outer"]["basis_tokens"]["sum"]).unwrap_or(0) != cell_sum {
        v.push("trials.outer.basis_tokens differs from the sum of the cells".into());
    }
    // 観測和 = trial 外側 + Jev。
    let want = n(&acct["trials"]["outer"]["basis_tokens"]["sum"]).unwrap_or(0)
        + n(&acct["jev"]["rows"]["basis_tokens"]["sum"]).unwrap_or(0);
    if n(&acct["observed_sum"]["tokens"]) != Some(want) {
        v.push(format!(
            "observed_sum.tokens is not outer trials + Jev ({want})"
        ));
    }
    // 照合済み cell の内訳は外側と一致し、内訳の和は照合済み + 内訳なしの外側に等しい。
    let matched: u64 = cells
        .iter()
        .filter(|c| c["reconcile"] == "match")
        .filter_map(|c| n(&c["trial_tokens"]))
        .sum();
    let by_role: u64 = ["worker", "planner", "reviewer"]
        .iter()
        .filter_map(|r| n(&acct["by_role"][*r]["basis_tokens"]["sum"]))
        .sum();
    let all_match = cells.iter().all(|c| {
        matches!(
            c["reconcile"].as_str(),
            Some("match" | "no_breakdown" | "not_executed")
        )
    });
    if all_match && by_role != matched {
        v.push(format!(
            "by_role tokens {by_role} differ from the matched cells {matched}"
        ));
    }
    // retry は同じ費用の別の切り口。
    let retry_sum = n(&acct["by_retry"]["initial"]["basis_tokens"]["sum"]).unwrap_or(0)
        + n(&acct["by_retry"]["retry"]["basis_tokens"]["sum"]).unwrap_or(0);
    if retry_sum != by_role {
        v.push(format!("by_retry tokens {retry_sum} differ from by_role {by_role} (retry was added again or dropped)"));
    }
    if acct["by_retry"]["additive"] != false {
        v.push("by_retry must not be additive".into());
    }
    // 完全でない和を総費用にしない。欠測があれば削減は確定しない。
    let gaps = acct["gaps"].as_array().cloned().unwrap_or_default();
    for metric in METRICS {
        let blocked = gaps
            .iter()
            .any(|g| g["metric"] == "all" || g["metric"] == metric);
        if acct["total"]["complete"][metric].as_bool() != Some(!blocked) {
            v.push(format!("total.complete.{metric} disagrees with the gaps"));
        }
        if acct["reduction_confirmable"][metric].as_bool() != Some(!blocked) {
            v.push(format!("reduction_confirmable.{metric} disagrees with the gaps (reductions need no missing data)"));
        }
    }
    for (metric, key) in [
        ("tokens", "tokens"),
        ("list_cost_usd", "list_cost_micro_usd"),
        ("cash_cost_usd", "cash_cost_micro_usd"),
    ] {
        let complete = acct["total"]["complete"][metric] == true;
        if complete == acct["total"][key].is_null() {
            v.push(format!(
                "total.{key} must be set exactly when {metric} is complete"
            ));
        }
        if complete && acct["total"][key] != acct["observed_sum"][key] {
            v.push(format!("total.{key} differs from the observed sum"));
        }
    }
    if acct["shared_unattributable"]["added_to_total"] != false {
        v.push("shared_unattributable must not be added to the total".into());
    }
    v
}
