//! campaign runner: 試行ごとに workspace/KB を分離し、実行器を呼び、生記録（JSONL）を追記する。
//! 状態の正本は out ディレクトリ（再起動時は trials.jsonl から続きを実行する）。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use routing_eval::manifest::Manifest as InputManifest;
use serde_json::{Value, json};

use crate::{
    aggregate::Outcome,
    executor::{ExecResult, ExecStatus, Executor, TrialRequest, Usage},
    ledger::{Ledger, LedgerRow, LedgerState, TrialKey},
    manifest::{Budget, CampaignManifest, Cap, ConfigId},
    record::*,
    snapshot::{copy_tree, hash_tree},
};

#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    pub out: PathBuf,
    /// 初期 repo / 資料 / KB の取り込み元。省略時は空の木。
    pub repo_src: Option<PathBuf>,
    pub materials_src: Option<PathBuf>,
    pub kb_src: Option<PathBuf>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    pub executed: u64,
    pub resumed_skipped: u64,
    pub budget_cancelled: u64,
    /// 前提が成立せず blocked として記録した試行数。
    pub blocked: u64,
    /// 前提が外部待ちで今回は実行も記録もしなかった試行数（再実行で続きを実行する）。
    pub waiting: u64,
    /// 観測できた tokens の和（欠測は含めない）。
    pub tokens_total: u64,
    /// 上限判定に使う tokens の和。欠測の試行は per_case.max_tokens を消費したものとして加算する。
    pub tokens_charged_total: u64,
    /// tokens が欠測（None）のまま終わった試行数（0 として無料扱いしない）。
    pub tokens_missing: u64,
    /// executor が Err を返し、その試行だけ error として記録した件数（run は続行）。
    pub executor_errors: u64,
    pub wall_secs_total: u64,
    pub budget_stopped: bool,
    /// per_case 超過で止めた試行数（reason = case_budget）。
    pub case_budget_cancelled: u64,
    /// per_arm の残りが尽きて止めた／実行しなかった試行数（reason = arm_budget）。
    pub arm_budget_cancelled: u64,
    /// 実効上限に対する最大超過量。粒度は `OVERSHOOT_GRANULARITY`。
    pub max_overshoot_tokens: u64,
    pub max_overshoot_wall_secs: u64,
}

/// 最大超過の粒度（summary にも書く）。ADR 2026-10-16-campaign-budget-cap D4。
pub const OVERSHOOT_GRANULARITY: &str = "async executor: up to one poll interval of usage (one runner pass); sync executor: up to one whole trial unless the executor stops itself via TrialRequest.budget; arm cap: up to the last trial";

/// 試行の実効上限 = min(per_case, per_arm の残り)。
fn effective_cap(b: &Budget, arm_tokens: u64, arm_wall: u64) -> Cap {
    Cap {
        max_tokens: b
            .per_case
            .max_tokens
            .min(b.per_arm.max_tokens.saturating_sub(arm_tokens)),
        max_wall_secs: b
            .per_case
            .max_wall_secs
            .min(b.per_arm.max_wall_secs.saturating_sub(arm_wall)),
    }
}

/// 使用量が上限を超えていればその reason。tokens 欠測は tokens 側を判定しない（終了後に保守的に課金する）。
fn breach_reason(
    b: &Budget,
    arm: (u64, u64),
    tokens: Option<u64>,
    wall: u64,
) -> Option<&'static str> {
    if tokens.is_some_and(|t| t > b.per_case.max_tokens) || wall > b.per_case.max_wall_secs {
        Some(REASON_CASE_BUDGET)
    } else if tokens.is_some_and(|t| arm.0.saturating_add(t) > b.per_arm.max_tokens)
        || arm.1.saturating_add(wall) > b.per_arm.max_wall_secs
    {
        Some(REASON_ARM_BUDGET)
    } else {
        None
    }
}

fn arm_exhausted(b: &Budget, arm: (u64, u64)) -> bool {
    arm.0 >= b.per_arm.max_tokens || arm.1 >= b.per_arm.max_wall_secs
}

/// drive_async の結果。
enum Driven {
    Done(ExecResult),
    /// 実行中に上限を超えたので cancel した（観測した使用量と reason）。
    Cancelled(ExecResult, &'static str),
}

fn write_json(path: &Path, v: &Value) -> Result<(), String> {
    let mut s = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    s.push('\n');
    fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
}

fn safe_component(id: &str) -> Result<(), String> {
    if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\', '\0']) {
        return Err(format!(
            "problem id {id:?} is not usable as a path component"
        ));
    }
    Ok(())
}

/// 初回は snapshot を保存し、再開時は保存済み snapshot が壊れていないことを確かめる。
/// 戻り値は (repo_dir, kb_dir, repo_sha256, kb_sha256)。
fn prepare_snapshot(
    m: &CampaignManifest,
    opts: &RunOptions,
) -> Result<(PathBuf, PathBuf, String, String), String> {
    let snap = opts.out.join("snapshot");
    let (repo, mat, kb) = (snap.join("repo"), snap.join("materials"), snap.join("kb"));
    let meta_path = opts.out.join(SNAPSHOT_FILE);
    if meta_path.exists() {
        let v: Value = serde_json::from_slice(
            &fs::read(&meta_path).map_err(|e| format!("{}: {e}", meta_path.display()))?,
        )
        .map_err(|e| format!("snapshot.json: {e}"))?;
        let rs = hash_tree(&repo)?;
        let ks = hash_tree(&kb)?;
        if v["repo_sha256"] != rs
            || v["kb_sha256"] != ks
            || v["materials_sha256"] != hash_tree(&mat)?
        {
            return Err("saved snapshot no longer matches its recorded hash".into());
        }
        return Ok((repo, kb, rs, ks));
    }
    for (src, dst) in [
        (&opts.repo_src, &repo),
        (&opts.materials_src, &mat),
        (&opts.kb_src, &kb),
    ] {
        match src {
            Some(s) => copy_tree(s, dst)?,
            None => fs::create_dir_all(dst).map_err(|e| e.to_string())?,
        }
    }
    let (rs, ms, ks) = (hash_tree(&repo)?, hash_tree(&mat)?, hash_tree(&kb)?);
    let verified = opts.materials_src.is_some();
    if verified && ms != m.snapshot.materials_sha256 {
        return Err(format!(
            "materials hash {ms} does not match the manifest pin {}",
            m.snapshot.materials_sha256
        ));
    }
    write_json(
        &meta_path,
        &json!({
            "repo_commit": m.snapshot.repo_commit,
            "kb_snapshot": m.snapshot.kb_snapshot,
            "repo_sha256": rs,
            "materials_sha256": ms,
            "kb_sha256": ks,
            "materials_pin_verified": verified,
        }),
    )?;
    Ok((repo, kb, rs, ks))
}

/// 既存の trials.jsonl を読む。crash による末尾の不完全な行は切り落とす。
fn load_records(path: &Path) -> Result<Vec<TrialRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let keep = match text.rfind('\n') {
        Some(i) => i + 1,
        None => 0,
    };
    if keep < text.len() {
        let f = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        f.set_len(keep as u64).map_err(|e| e.to_string())?;
    }
    text[..keep]
        .lines()
        .enumerate()
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|e| format!("trials.jsonl line {}: {e}", i + 1))
        })
        .collect()
}

fn append(path: &Path, r: &TrialRecord) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut line = serde_json::to_string(r).map_err(|e| e.to_string())?;
    line.push('\n');
    f.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    f.flush().map_err(|e| e.to_string())
}

/// 実行すべき試行の一覧（problem 依存順 → problem 内の arm 順（manifest の order/seed）→ 反復 rep）。C は事前登録 subset だけ。
/// 3 番目は独立反復 rep（1..=repeats）。内部再試行は無い（trial_no は常に 1）。
pub fn plan(m: &CampaignManifest) -> Vec<(ConfigId, String, u32)> {
    let n = m.normalized();
    let subset: BTreeSet<&str> = n.secondary.subset.iter().map(String::as_str).collect();
    // 前提が先。依存が無ければ id 昇順（validate 済みの manifest では Err にならない）。
    let order = n.problem_order().unwrap_or_else(|_| n.problems.clone());
    let mut v = Vec::new();
    for (pos, p) in order.iter().enumerate() {
        let arms: Vec<ConfigId> = [ConfigId::A, ConfigId::B, ConfigId::C]
            .into_iter()
            .filter(|c| *c != ConfigId::C || subset.contains(p.as_str()))
            .collect();
        for c in n.arm_sequence(p, pos, arms) {
            for rep in 1..=m.repeats {
                v.push((c, p.clone(), rep));
            }
        }
    }
    v
}

enum Prereq {
    Met,
    Blocked(String),
    Waiting,
}

/// 前提 problem の全 config（計画された分）の最初の試行が passed かを記録から判定する。
/// 失敗側（failed/timeout/error/cancelled/blocked）が 1 つでもあれば Blocked、未記録があれば Waiting。
fn prereq_state(m: &CampaignManifest, records: &[TrialRecord], problem: &str, rep: u32) -> Prereq {
    let mut waiting = false;
    for q in m.dependencies.get(problem).into_iter().flatten() {
        for c in m.planned_configs(q) {
            let first = records
                .iter()
                .filter(|r| r.config == c && r.problem_id == *q && r.rep == rep)
                .min_by_key(|r| r.trial_no);
            match first.map(|r| r.outcome) {
                Some(Outcome::Passed) => {}
                Some(_) => return Prereq::Blocked(q.clone()),
                None => waiting = true,
            }
        }
    }
    if waiting {
        Prereq::Waiting
    } else {
        Prereq::Met
    }
}

#[allow(clippy::too_many_arguments)]
fn new_record(
    cfg: &crate::manifest::Config,
    hash: &str,
    input: &InputManifest,
    m: &CampaignManifest,
    problem_id: String,
    config: ConfigId,
    rep: u32,
    arm_rank: u32,
) -> TrialRecord {
    TrialRecord {
        schema: TRIAL_SCHEMA.into(),
        campaign_sha256: hash.to_string(),
        input_sha256: input.sha256.clone(),
        split: m.split,
        config,
        problem_id,
        trial_no: 1,
        rep,
        arm_rank,
        outcome: Outcome::Cancelled,
        passed: false,
        tokens: None,
        cost_micro_usd: None,
        wall_secs: 0,
        reason: None,
        harness: cfg.harness.clone(),
        harness_version: cfg.harness_version.clone(),
        model: cfg.model.clone(),
        model_version: cfg.model_version.clone(),
        policy_version: cfg.policy_version.clone(),
        workspace: String::new(),
        kb: String::new(),
        workspace_sha256_initial: String::new(),
        kb_sha256_initial: String::new(),
        workspace_sha256_final: String::new(),
        kb_sha256_final: String::new(),
    }
}

/// 未完了の台帳行を cancel（外部の task・子 task・クラスタ job ごと）して台帳を閉じる。
/// task_id がまだ無い planned 行は冪等キーで照会し、投入済みなら同様に cancel する。
fn cancel_row(
    exec: &dyn Executor,
    ledger: &mut Ledger,
    row: &LedgerRow,
    reason: &str,
) -> Result<(), String> {
    let t = match row.task_ref() {
        Some(t) => Some(t),
        None => exec.lookup(&row.idempotency_key)?,
    };
    if let Some(t) = t {
        exec.cancel(&t, reason)?;
    }
    ledger.mark_cancelled(&row.key, reason)
}

/// drive_async の失敗。executor の Err はその試行だけ error にし、台帳・fs の失敗は run の失敗にする。
enum DriveErr {
    Exec(String),
    Fatal(String),
}

impl From<String> for DriveErr {
    fn from(e: String) -> Self {
        DriveErr::Fatal(e)
    }
}

/// 非同期実行器の 1 試行を台帳経由で進める。Wait なら None（workspace は消さない）。
#[allow(clippy::too_many_arguments)]
fn drive_async(
    exec: &dyn Executor,
    ledger: &mut Ledger,
    key: &TrialKey,
    req: &TrialRequest,
    out: &Path,
    rel: &str,
    repo: &Path,
    kb: &Path,
    budget: &Budget,
    arm: (u64, u64),
) -> Result<Option<Driven>, DriveErr> {
    let ws_rel = format!("{rel}/workspace");
    let idem = key.idempotency_key();
    let known = ledger.get(key).cloned();
    let mut task = known.as_ref().and_then(LedgerRow::task_ref);
    if known.is_none() {
        // 投入『前』に planned を fsync する。ここで落ちても冪等キーで再開できる。
        ledger.plan(key, &ws_rel, exec.routing(req))?;
    }
    if task.is_none() {
        // planned のまま（task_id 無し）: 同一冪等キーで照会し、無ければ投入する。
        let existing = exec.lookup(&idem).map_err(DriveErr::Exec)?;
        if existing.is_none() {
            // 外部に何も無いので、workspace は初期 snapshot から作り直してよい。
            let _ = fs::remove_dir_all(out.join(rel));
            copy_tree(repo, req.workspace)?;
            copy_tree(kb, req.kb)?;
        } else if !req.workspace.exists() {
            copy_tree(repo, req.workspace)?;
            copy_tree(kb, req.kb)?;
        }
        let t = match existing {
            Some(t) => t,
            None => exec.submit(req, &idem).map_err(DriveErr::Exec)?,
        };
        ledger.mark_submitted(key, &t)?;
        task = Some(t);
    }
    let t = task.ok_or_else(|| String::from("task ref missing"))?;
    let fresh = exec.track(&t).map_err(DriveErr::Exec)?;
    if fresh != t {
        ledger.mark_waiting(key, &fresh)?;
    }
    match exec.poll(&fresh).map_err(DriveErr::Exec)? {
        ExecStatus::Done(r) => Ok(Some(Driven::Done(r))),
        ExecStatus::Wait => {
            // poll ごとに子 task・job を含む累積使用量を見て、上限超過なら cancel する。
            if let Some(Usage { tokens, wall_secs }) = exec.usage(&fresh).map_err(DriveErr::Exec)?
                && let Some(reason) = breach_reason(budget, arm, tokens, wall_secs)
            {
                if let Some(row) = ledger.get(key).cloned() {
                    cancel_row(exec, ledger, &row, reason)?;
                }
                return Ok(Some(Driven::Cancelled(
                    ExecResult {
                        outcome: Outcome::Cancelled,
                        tokens,
                        cost_micro_usd: None,
                        wall_secs,
                    },
                    reason,
                )));
            }
            if ledger.get(key).map(|r| r.state) != Some(LedgerState::Waiting) {
                ledger.mark_waiting(key, &fresh)?;
            }
            Ok(None)
        }
    }
}

pub fn run(
    m: &CampaignManifest,
    input: &InputManifest,
    exec: &dyn Executor,
    opts: &RunOptions,
) -> Result<RunReport, String> {
    m.check_against_input(input)?;
    let hash = m.compute_sha256()?;
    let out = &opts.out;
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let meta_path = out.join(META_FILE);
    if meta_path.exists() {
        let v: Value = serde_json::from_slice(&fs::read(&meta_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("run-meta.json: {e}"))?;
        if v["campaign_sha256"] != hash {
            return Err("out dir belongs to a different campaign hash; use an empty dir".into());
        }
    }
    let (repo, kb, repo_sha, kb_sha) = prepare_snapshot(m, opts)?;
    write_json(
        &out.join(CAMPAIGN_FILE),
        &serde_json::to_value(m.normalized()).map_err(|e| e.to_string())?,
    )?;
    write_json(
        &out.join(INPUT_FILE),
        &serde_json::to_value(input).map_err(|e| e.to_string())?,
    )?;
    write_json(
        &meta_path,
        &json!({
            "schema": "celeris.campaign-run-meta/1",
            "campaign_id": m.campaign_id,
            "campaign_sha256": hash,
            "input_sha256": input.sha256,
            "split": m.split,
            "executor": exec.name(),
            "configs": m.normalized().configs,
            "budget": m.budget,
            "termination": m.termination,
            "snapshot": m.snapshot,
            "expected_trials": plan(m).len(),
            "order": m.order,
            "seed": m.seed,
        }),
    )?;

    let trials_path = out.join(TRIALS_FILE);
    let mut records = load_records(&trials_path)?;
    for r in &records {
        if r.campaign_sha256 != hash || r.split != m.split {
            return Err("existing trials.jsonl contains records of another campaign/split".into());
        }
    }
    let done: BTreeSet<(ConfigId, String, u32)> = records
        .iter()
        .map(|r| (r.config, r.problem_id.clone(), r.rep))
        .collect();
    let mut ledger = Ledger::open(out)?;
    // trials.jsonl に記録済みなのに台帳が未完了のまま（記録直後の crash）は done に直す。
    let stale: Vec<TrialKey> = ledger
        .unfinished()
        .into_iter()
        .filter(|r| r.key.manifest_sha256 == hash)
        .filter(|r| done.contains(&(r.key.arm, r.key.problem_id.clone(), r.key.rep)))
        .map(|r| r.key.clone())
        .collect();
    for k in stale {
        ledger.mark_done(&k)?;
    }
    let mut rep = RunReport {
        resumed_skipped: records.len() as u64,
        tokens_total: records.iter().filter_map(|r| r.tokens).sum(),
        wall_secs_total: records.iter().map(|r| r.wall_secs).sum(),
        ..RunReport::default()
    };
    // arm ごとの累積（欠測 tokens は保守的に課金する）。再開時は記録から作り直す。
    let mut arm_use: BTreeMap<ConfigId, (u64, u64)> = BTreeMap::new();
    for r in records.iter().filter(|r| r.ran()) {
        let e = arm_use.entry(r.config).or_default();
        e.0 += r.charged_tokens(&m.budget);
        e.1 += r.wall_secs;
        rep.tokens_charged_total += r.charged_tokens(&m.budget);
        rep.tokens_missing +=
            u64::from(r.tokens.is_none() && r.reason.as_deref() != Some(REASON_EXECUTOR_ERROR));
        rep.executor_errors += u64::from(r.reason.as_deref() == Some(REASON_EXECUTOR_ERROR));
    }
    let over = |rep: &RunReport| {
        m.termination.stop_on_budget
            && (rep.tokens_charged_total >= m.budget.max_tokens
                || rep.wall_secs_total >= m.budget.max_wall_secs)
    };
    rep.budget_stopped = over(&rep);
    let prompts: BTreeMap<&str, &str> = input
        .problems
        .iter()
        .map(|p| (p.id.as_str(), p.prompt.as_str()))
        .collect();
    let cfgs: BTreeMap<ConfigId, &crate::manifest::Config> =
        m.configs.iter().map(|c| (c.id, c)).collect();

    let planned = plan(m);
    // problem 内の arm 実行位置（1 始まり）。
    let mut ranks: BTreeMap<(String, ConfigId), u32> = BTreeMap::new();
    for (c, p, _) in &planned {
        let next = ranks.keys().filter(|(q, _)| q == p).count() as u32 + 1;
        ranks.entry((p.clone(), *c)).or_insert(next);
    }
    for (cid, pid, rep_no) in planned {
        if done.contains(&(cid, pid.clone(), rep_no)) {
            continue;
        }
        safe_component(&pid)?;
        let cfg = cfgs.get(&cid).ok_or("config missing")?;
        let arm_rank = ranks.get(&(pid.clone(), cid)).copied().unwrap_or(0);
        let mut rec = new_record(cfg, &hash, input, m, pid.clone(), cid, rep_no, arm_rank);
        let key = TrialKey {
            manifest_sha256: hash.clone(),
            problem_id: pid.clone(),
            arm: cid,
            trial_no: 1,
            rep: rep_no,
        };
        let blocked_by = match prereq_state(m, &records, &pid, rep_no) {
            Prereq::Met => None,
            Prereq::Blocked(q) => Some(q),
            Prereq::Waiting => {
                rep.waiting += 1;
                continue;
            }
        };
        if blocked_by.is_some() {
            rec.outcome = Outcome::Blocked;
            rec.reason = Some(REASON_BLOCKED.into());
            rep.blocked += 1;
        } else if rep.budget_stopped {
            rec.reason = Some(REASON_BUDGET_STOP.into());
            rep.budget_cancelled += 1;
            // 打ち切り: 既に外部へ投入済みの試行は、子 task・job ごと cancel して台帳を閉じる。
            if let Some(row) = ledger.get(&key).filter(|r| !r.state.is_terminal()).cloned() {
                cancel_row(exec, &mut ledger, &row, REASON_BUDGET_STOP)?;
            }
        } else if arm_exhausted(&m.budget, arm_use.get(&cid).copied().unwrap_or_default()) {
            // この arm の累積上限が尽きた。実行せず cancelled として分母に残す（他 arm は止めない）。
            rec.reason = Some(REASON_ARM_BUDGET.into());
            rep.arm_budget_cancelled += 1;
        } else {
            let arm = arm_use.get(&cid).copied().unwrap_or_default();
            let cap = effective_cap(&m.budget, arm.0, arm.1);
            let rel = format!("trials/{cid:?}/{pid}/rep{rep_no}");
            let (ws, kbd) = (out.join(&rel).join("workspace"), out.join(&rel).join("kb"));
            let is_async = exec.is_async();
            if !is_async {
                // 中断した試行の残骸があれば捨て、必ず初期 snapshot から作り直す。
                let _ = fs::remove_dir_all(out.join(&rel));
                copy_tree(&repo, &ws)?;
                copy_tree(&kb, &kbd)?;
                rec.workspace_sha256_initial = hash_tree(&ws)?;
                rec.kb_sha256_initial = hash_tree(&kbd)?;
                if rec.workspace_sha256_initial != repo_sha || rec.kb_sha256_initial != kb_sha {
                    return Err(format!(
                        "trial dir {rel} does not start from the saved snapshot"
                    ));
                }
            } else {
                // 非同期: 作成・再利用は drive_async が台帳を見て決める（参照中は消さない）。
                rec.workspace_sha256_initial = repo_sha.clone();
                rec.kb_sha256_initial = kb_sha.clone();
            }
            rec.workspace = format!("{rel}/workspace");
            rec.kb = format!("{rel}/kb");
            let timeout = m.termination.trial_timeout_secs;
            let req = TrialRequest {
                config: cfg,
                problem_id: &pid,
                prompt: prompts.get(pid.as_str()).copied().unwrap_or(""),
                trial_no: 1,
                rep: rep_no,
                workspace: &ws,
                kb: &kbd,
                timeout_secs: timeout,
                budget: cap,
            };
            let mut cancelled_by: Option<&'static str> = None;
            let mut errored: Option<String> = None;
            let failed_result = || ExecResult {
                outcome: Outcome::Error,
                tokens: None,
                cost_micro_usd: None,
                wall_secs: 0,
            };
            let mut res = if is_async {
                match drive_async(
                    exec,
                    &mut ledger,
                    &key,
                    &req,
                    out,
                    &rel,
                    &repo,
                    &kb,
                    &m.budget,
                    arm,
                ) {
                    Err(DriveErr::Fatal(e)) => return Err(e),
                    Err(DriveErr::Exec(e)) => {
                        // 外部 task があれば best-effort で cancel して台帳を閉じる。run は続行する。
                        if let Some(row) =
                            ledger.get(&key).filter(|r| !r.state.is_terminal()).cloned()
                        {
                            let _ = cancel_row(exec, &mut ledger, &row, REASON_EXECUTOR_ERROR);
                        }
                        errored = Some(e);
                        failed_result()
                    }
                    Ok(Some(Driven::Done(r))) => r,
                    Ok(Some(Driven::Cancelled(r, why))) => {
                        cancelled_by = Some(why);
                        r
                    }
                    Ok(None) => {
                        // 外部待ち: 台帳が参照中なので workspace は消さず、再実行で同じ task を追う。
                        rep.waiting += 1;
                        continue;
                    }
                }
            } else {
                match exec.execute(&req) {
                    Err(e) => {
                        errored = Some(e);
                        failed_result()
                    }
                    Ok(ExecStatus::Done(r)) => r,
                    Ok(ExecStatus::Wait) => {
                        // 外部待ち: 記録を残さず、残骸も消して、再実行（resume）で改めて実行する。
                        let _ = fs::remove_dir_all(out.join(&rel));
                        rep.waiting += 1;
                        continue;
                    }
                }
            };
            if errored.is_some() {
                rec.reason = Some(REASON_EXECUTOR_ERROR.into());
                rep.executor_errors += 1;
            }
            if res.wall_secs > timeout && res.outcome != Outcome::Timeout {
                res.outcome = Outcome::Timeout;
                rec.reason = Some("runner_timeout".into());
            }
            res.wall_secs = res.wall_secs.min(timeout);
            // 同期 executor（と実行中に観測できなかった async）は完了後に上限を判定する。Timeout が優先。
            if cancelled_by.is_none() && errored.is_none() && res.outcome != Outcome::Timeout {
                cancelled_by = breach_reason(&m.budget, arm, res.tokens, res.wall_secs);
            }
            if let Some(why) = cancelled_by {
                res.outcome = Outcome::Cancelled;
                rec.reason = Some(why.into());
                if why == REASON_CASE_BUDGET {
                    rep.case_budget_cancelled += 1;
                } else {
                    rep.arm_budget_cancelled += 1;
                }
            }
            if cancelled_by.is_some() || res.wall_secs > cap.max_wall_secs {
                rep.max_overshoot_wall_secs = rep
                    .max_overshoot_wall_secs
                    .max(res.wall_secs.saturating_sub(cap.max_wall_secs));
            }
            if let Some(t) = res.tokens {
                rep.max_overshoot_tokens = rep
                    .max_overshoot_tokens
                    .max(t.saturating_sub(cap.max_tokens));
            }
            let charged = if errored.is_some() {
                0
            } else {
                m.budget.charged_tokens(res.tokens)
            };
            let e = arm_use.entry(cid).or_default();
            e.0 += charged;
            e.1 += res.wall_secs;
            rep.tokens_charged_total += charged;
            rep.tokens_missing += u64::from(res.tokens.is_none() && errored.is_none());
            rec.outcome = res.outcome;
            rec.passed = res.outcome == Outcome::Passed;
            rec.tokens = res.tokens;
            rec.cost_micro_usd = res.cost_micro_usd;
            rec.wall_secs = res.wall_secs;
            rec.workspace_sha256_final = hash_tree(&ws)?;
            rec.kb_sha256_final = hash_tree(&kbd)?;
            rep.executed += 1;
            rep.tokens_total += res.tokens.unwrap_or(0);
            rep.wall_secs_total += res.wall_secs;
            rep.budget_stopped = over(&rep);
        }
        append(&trials_path, &rec)?;
        if ledger.get(&key).is_some_and(|r| !r.state.is_terminal()) {
            ledger.mark_done(&key)?;
        }
        records.push(rec);
    }
    // 打ち切り後に残った未完了の台帳行（前提待ちで今回の走査に来なかったもの）も cancel する。
    if rep.budget_stopped {
        for row in ledger.unfinished().into_iter().cloned().collect::<Vec<_>>() {
            if row.key.manifest_sha256 != hash {
                continue;
            }
            cancel_row(exec, &mut ledger, &row, REASON_BUDGET_STOP)?;
            let cfg = cfgs.get(&row.key.arm).ok_or("config missing")?;
            let mut rec = new_record(
                cfg,
                &hash,
                input,
                m,
                row.key.problem_id.clone(),
                row.key.arm,
                row.key.rep,
                ranks
                    .get(&(row.key.problem_id.clone(), row.key.arm))
                    .copied()
                    .unwrap_or(0),
            );
            rec.reason = Some(REASON_BUDGET_STOP.into());
            rec.workspace = row.workspace.clone();
            rep.budget_cancelled += 1;
            append(&trials_path, &rec)?;
            records.push(rec);
        }
    }
    write_summary(m, &hash, &records, &rep, out)?;
    Ok(rep)
}

/// runner 自身の集計（checker は別実装で再計算して突き合わせる）。
fn write_summary(
    m: &CampaignManifest,
    hash: &str,
    records: &[TrialRecord],
    rep: &RunReport,
    out: &Path,
) -> Result<(), String> {
    let mut cells = serde_json::Map::new();
    for c in [ConfigId::A, ConfigId::B, ConfigId::C] {
        // 分母 = 計画された試行総数（problems × repeats、C は subset × repeats）。
        let n = plan(m).iter().filter(|(k, _, _)| *k == c).count();
        let mut counts = [0u64; 6]; // passed failed error timeout cancelled blocked
        let mut seen = 0u64;
        for r in records.iter().filter(|r| r.config == c) {
            seen += 1;
            counts[match r.outcome {
                Outcome::Passed => 0,
                Outcome::Failed => 1,
                Outcome::Error => 2,
                Outcome::Timeout => 3,
                Outcome::Cancelled => 4,
                Outcome::Blocked => 5,
            }] += 1;
        }
        cells.insert(
            format!("{c:?}"),
            json!({"n": n, "passed": counts[0], "failed": counts[1], "error": counts[2],
                   "timeout": counts[3], "cancelled": counts[4], "blocked": counts[5], "missing": (n as u64).saturating_sub(seen)}),
        );
    }
    write_json(
        &out.join(SUMMARY_FILE),
        &json!({
            "schema": "celeris.campaign-summary/1",
            "campaign_sha256": hash,
            "records": records.len(),
            "budget_stopped": rep.budget_stopped,
            "budget": {
                "per_case": m.budget.per_case,
                "per_arm": m.budget.per_arm,
                "case_budget_cancelled": rep.case_budget_cancelled,
                "arm_budget_cancelled": rep.arm_budget_cancelled,
                "tokens_missing": rep.tokens_missing,
                "executor_errors": rep.executor_errors,
                "tokens_charged_total": rep.tokens_charged_total,
                "missing_tokens_policy": "charged as per_case.max_tokens for cap decisions; never counted as zero",
                "max_overshoot_tokens": rep.max_overshoot_tokens,
                "max_overshoot_wall_secs": rep.max_overshoot_wall_secs,
                "overshoot_granularity": OVERSHOOT_GRANULARITY,
            },
            "cells": Value::Object(cells),
        }),
    )
}
