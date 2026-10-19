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
    budget::{self, Gate, effective_cap},
    executor::{ErrKind, ExecError, ExecResult, ExecStatus, Executor, TrialRequest, Usage},
    global_ledger::{
        Admission, AttemptId, AttemptRow, AttemptState, Caps, GlobalLedger, Terminal,
        TrialAdmission,
    },
    inputs::{self, INPUTS_DIR, Seeder},
    ledger::{Ledger, LedgerRow, LedgerState, ObservedUsage, TrialKey},
    lock::CampaignLock,
    lock_exec,
    manifest::{Budget, CampaignManifest, ConfigId},
    record::*,
    series,
    snapshot::{copy_tree, hash_tree, hash_tree_excluding},
};

#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    pub out: PathBuf,
    /// 初期 repo / 資料 / KB の取り込み元。省略時は空の木。
    pub repo_src: Option<PathBuf>,
    pub materials_src: Option<PathBuf>,
    pub kb_src: Option<PathBuf>,
    /// 外部入力の取り込み元（`<dir>/<name>`）。`case_inputs` が空でなければ必須。
    pub inputs_src: Option<PathBuf>,
    /// 問題ごとの宣言入力（case.json の `inputs[]`）。各試行の workspace の `inputs/` に配る。
    pub case_inputs: BTreeMap<String, Vec<DeliveredInput>>,
    /// 期待 campaign lock（`lock_exec::expected_lock`）。実行（有料）経路は必須。None は mock など無課金の制御用。
    /// 渡すと、executor 投入・台帳更新より前に実体を照合し、`<out>/lock.json` に保存・再照合する。
    pub lock: Option<CampaignLock>,
    /// 全体台帳の置き場（測定 ID を跨ぐ既消費・予約・provider 枠）。None は単独測定（従来どおり）。
    pub global_ledger: Option<PathBuf>,
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
    /// `budget.max_parallel` の in-flight 上限に達したため今回は投入しなかった試行数（`waiting` にも数える）。
    pub parallel_deferred: u64,
    /// 残予算が予約を下回り、先行の予約が解けるまで投入を見送った試行数（`waiting` にも数える）。
    pub reserve_deferred: u64,
    /// A の試行で exit 0 だが成果物（workspace の変化）が無く failed にした累計（再開をまたいで記録から数える）。
    pub no_artifact: u64,
}

/// 最大超過の粒度（summary にも書く）。ADR 2026-10-16-campaign-budget-cap D4。
pub const OVERSHOOT_GRANULARITY: &str = "async executor: up to one poll interval of usage (one runner pass); sync executor: up to one whole trial unless the executor stops itself via TrialRequest.budget; arm cap: up to the last trial";

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
    Done(ExecResult, Vec<ObservedRun>),
    /// 外部の結果がまだ無い（待ち・通信断・投入結果不明）。台帳が追跡を続ける。
    Deferred,
    /// 確実な未投入の拒否（その試行だけ error）。
    Refused(String),
    /// 実行中に上限を超えたので cancel した（観測した使用量と reason）。
    Cancelled(ExecResult, &'static str, Vec<ObservedRun>),
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

/// 試行の初期木（repo + `materials/`）。ADR 2026-10-16-mlsys-pilot-wiring-fixes D5。
/// 試行の初期 KB も `kb/` として同じ木に入れる。A（harness）も B/C（Celeris task）も、この workspace の中の
/// `materials/` と `kb/` だけを初期資料として見る（通常運用 KB の path は試行に渡さない）。
fn build_seed(repo: &Path, mat: &Path, kb: &Path, seed: &Path) -> Result<(), String> {
    for (d, opt) in [("materials", "--materials-src"), ("kb", "--kb-src")] {
        if repo.join(d).exists() {
            return Err(format!(
                "repo snapshot already has {d}/; it would collide with {opt}"
            ));
        }
    }
    let _ = fs::remove_dir_all(seed);
    copy_tree(repo, seed)?;
    copy_tree(mat, &seed.join("materials"))?;
    copy_tree(kb, &seed.join("kb"))
}

/// 保存した snapshot。`empty` は初期木を持たない case 用の seed（lock が初期木ありの case と無い case を混ぜるときだけ）。
struct Snap {
    seed: PathBuf,
    seed_sha: String,
    kb_sha: String,
    empty: Option<(PathBuf, String)>,
}

/// 初回は snapshot を保存し、再開時は保存済み snapshot が壊れていないことを確かめる。
/// 戻り値は (試行の初期木（repo + materials/ + kb/）, kb_dir, 初期木の sha256, kb_sha256)。
fn prepare_snapshot(
    m: &CampaignManifest,
    opts: &RunOptions,
    lock: Option<&CampaignLock>,
) -> Result<Snap, String> {
    let snap = opts.out.join("snapshot");
    let (repo, mat, kb) = (snap.join("repo"), snap.join("materials"), snap.join("kb"));
    let seed = snap.join("workspace-seed");
    let empty = snap.join(lock_exec::EMPTY_SEED_DIR);
    let need_empty = lock.is_some_and(|l| !lock_exec::repo_free_cases(l).is_empty());
    let staged = snap.join(INPUTS_DIR);
    let meta_path = opts.out.join(SNAPSHOT_FILE);
    if meta_path.exists() {
        let v: Value = serde_json::from_slice(
            &fs::read(&meta_path).map_err(|e| format!("{}: {e}", meta_path.display()))?,
        )
        .map_err(|e| format!("snapshot.json: {e}"))?;
        let ks = hash_tree(&kb)?;
        let ss = hash_tree(&seed)?;
        let empty_sha = if need_empty {
            Some(hash_tree(&empty)?)
        } else {
            None
        };
        if v["repo_sha256"] != hash_tree(&repo)?
            || v["kb_sha256"] != ks
            || v["materials_sha256"] != hash_tree(&mat)?
            || v["workspace_seed_sha256"] != ss
            || empty_sha
                .as_ref()
                .is_some_and(|h| v["workspace_seed_empty_sha256"] != h.as_str())
        {
            return Err("saved snapshot no longer matches its recorded hash".into());
        }
        if !opts.case_inputs.is_empty() {
            inputs::verify_stage(&staged, &opts.case_inputs)?;
        }
        if let Some(l) = lock {
            lock_exec::verify_snapshot(l, &snap)?;
        }
        return Ok(Snap {
            seed,
            seed_sha: ss,
            kb_sha: ks,
            empty: empty_sha.map(|h| (empty, h)),
        });
    }
    if lock.is_some() {
        // snapshot.json の無い snapshot は途中で止まった残り。混ぜずに作り直す。
        let _ = fs::remove_dir_all(&snap);
    }
    if !opts.case_inputs.is_empty() {
        let src = opts.inputs_src.as_deref().ok_or(
            "selected cases declare external inputs; --inputs-src <dir> is required (ADR 2026-10-17-mlsys-pilot2-fixes D1)",
        )?;
        inputs::stage(src, &staged, &opts.case_inputs)?;
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
    build_seed(&repo, &mat, &kb, &seed)?;
    if need_empty {
        // 初期木を宣言しない case には repo を配らない（共通の --repo-src を誤配布しない）。
        let none = snap.join("repo-none");
        fs::create_dir_all(&none).map_err(|e| e.to_string())?;
        let r = build_seed(&none, &mat, &kb, &empty);
        let _ = fs::remove_dir_all(&none);
        r?;
    }
    if let Some(l) = lock {
        // 検査後にコピーした実体にも同じ期待値を当てる（検査後の差替えを見逃さない）。
        if let Err(e) = lock_exec::verify_snapshot(l, &snap) {
            let _ = fs::remove_dir_all(&snap);
            return Err(format!("copied snapshot fails the campaign lock: {e}"));
        }
    }
    let (rs, ms, ks) = (hash_tree(&repo)?, hash_tree(&mat)?, hash_tree(&kb)?);
    let ss = hash_tree(&seed)?;
    let empty_sha = if need_empty {
        Some(hash_tree(&empty)?)
    } else {
        None
    };
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
            "workspace_seed_sha256": ss,
            "workspace_seed_empty_sha256": empty_sha,
            "materials_pin_verified": verified,
            "inputs_sha256": inputs::unique(&opts.case_inputs)?
                .into_iter()
                .map(|x| (x.name, x.sha256))
                .collect::<BTreeMap<_, _>>(),
        }),
    )?;
    Ok(Snap {
        seed,
        seed_sha: ss,
        kb_sha: ks,
        empty: empty_sha.map(|h| (empty, h)),
    })
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
    // 前提が先。依存が無ければ id 昇順（validate 済みの manifest では Err にならない）。
    let order = n.problem_order().unwrap_or_else(|_| n.problems.clone());
    let mut v = Vec::new();
    for (pos, p) in order.iter().enumerate() {
        // 有効 arm だけ（未使用 arm は試行にも分母にも入れない）。C・B/C のみのときの B は subset のみ。
        let arms: Vec<ConfigId> = n.planned_configs(p);
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
        token_usage: None,
        cost_micro_usd: None,
        wall_secs: 0,
        reason: None,
        error: None,
        routing: None,
        observed: Vec::new(),
        harness: cfg.harness.clone(),
        harness_version: cfg.harness_version.clone(),
        model: cfg.model.clone(),
        model_version: cfg.model_version.clone(),
        policy_version: cfg.policy_version.clone(),
        workspace: String::new(),
        kb: String::new(),
        inputs: Vec::new(),
        workspace_sha256_initial: String::new(),
        kb_sha256_initial: String::new(),
        workspace_sha256_final: String::new(),
        kb_sha256_final: String::new(),
    }
}

/// cancel の結果。`Pending` は停止を確認できていない（台帳は cancel_pending のまま・予約も解除しない）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CancelOutcome {
    Stopped,
    Pending,
}

/// 未完了の台帳行を cancel（外部の task・子 task・クラスタ job ごと）する。
/// 先に cancel_pending を永続化し、cancel の成功と外部の終端の確認（`Executor::stopped`）が揃って初めて
/// cancelled（terminal）にする。cancel・照会・確認の失敗は隠さず台帳に残し、次回の run が続ける。
/// task_id がまだ無い行は冪等キーで照会し、投入済みなら同様に cancel する。
fn cancel_row(
    exec: &dyn Executor,
    ledger: &mut Ledger,
    row: &LedgerRow,
    reason: &str,
) -> Result<CancelOutcome, String> {
    let reason = match (row.state, row.reason.as_deref()) {
        (LedgerState::CancelPending, Some(first)) => first,
        _ => reason,
    };
    if row.state != LedgerState::CancelPending {
        ledger.mark_cancel_pending(&row.key, reason, None)?;
    }
    let t = match row.task_ref() {
        Some(t) => Some(t),
        None => match exec.lookup(&row.idempotency_key) {
            Ok(t) => t,
            Err(e) => {
                ledger.note_comm_failure(&row.key, &e.to_string())?;
                return Ok(CancelOutcome::Pending);
            }
        },
    };
    if let Some(t) = t {
        if let Err(e) = exec.cancel(&t, reason) {
            ledger.note_comm_failure(&row.key, &format!("cancel: {e}"))?;
            return Ok(CancelOutcome::Pending);
        }
        match exec.stopped(&t) {
            Ok(true) => {}
            Ok(false) => {
                ledger.note_comm_failure(&row.key, "cancel: external task not yet terminal")?;
                return Ok(CancelOutcome::Pending);
            }
            Err(e) => {
                ledger.note_comm_failure(&row.key, &format!("stopped: {e}"))?;
                return Ok(CancelOutcome::Pending);
            }
        }
    }
    ledger.mark_cancelled(&row.key, reason)?;
    Ok(CancelOutcome::Stopped)
}

/// 非同期 executor の in-flight（この campaign の未完了の台帳行のうち自分以外）が `max_parallel` に達しているか。
/// 既に台帳に行がある試行（投入済み・追跡中）は上限に関係なく追跡を続ける。
fn at_parallel_cap(ledger: &Ledger, key: &TrialKey, hash: &str, max_parallel: u32) -> bool {
    if ledger.get(key).is_some_and(|r| !r.state.is_terminal()) {
        return false;
    }
    let in_flight = ledger
        .unfinished()
        .into_iter()
        .filter(|r| r.key.manifest_sha256 == hash && r.key != *key)
        .count();
    in_flight >= max_parallel as usize
}

/// 非同期実行器の 1 試行を台帳経由で進める。外部の結果が出ていない（Wait・通信断・結果不明）なら `Deferred`
/// （workspace は消さず、再実行で同じ task を追う）。executor の失敗は `ErrKind` で扱いを分ける。
#[allow(clippy::too_many_arguments)]
fn drive_async(
    exec: &dyn Executor,
    ledger: &mut Ledger,
    key: &TrialKey,
    req: &TrialRequest,
    out: &Path,
    rel: &str,
    seeder: &Seeder,
    budget: &Budget,
    arm: (u64, u64),
    shared: Option<(&GlobalLedger, &AttemptId)>,
) -> Result<Driven, String> {
    let ws_rel = format!("{rel}/workspace");
    let idem = key.idempotency_key();
    let known = ledger.get(key).cloned();
    if known
        .as_ref()
        .is_some_and(|r| r.state == LedgerState::CancelPending)
    {
        // 停止の確認待ち。新しい仕事として扱わない（cancel の再試行は run の先頭で行う）。
        return Ok(Driven::Deferred);
    }
    let mut task = known.as_ref().and_then(LedgerRow::task_ref);
    if known.is_none() {
        // 投入『前』に planned を fsync する。ここで落ちても冪等キーで再開できる。
        ledger.plan(key, &ws_rel, exec.routing(req)?)?;
    }
    // 予約額（この試行の実効上限）も投入前に台帳へ残す。外部の終端を確認するまで解除しない。
    ledger.reserve(key, req.budget)?;
    if task.is_none() {
        let unknown = known
            .as_ref()
            .is_some_and(|r| r.state == LedgerState::SubmitUnknown);
        // planned のまま（task_id 無し）: 同一冪等キーで照会し、無ければ投入する。
        let existing = match exec.lookup(&idem) {
            Ok(t) => t,
            Err(e) if e.kind == ErrKind::Refused => return refuse(ledger, key, e),
            Err(e) => {
                ledger.note_comm_failure(key, &format!("lookup: {e}"))?;
                return Ok(Driven::Deferred);
            }
        };
        if existing.is_none() && unknown {
            // 投入結果が不明のまま照会しても見つからない: 再 submit せず、次回の照会に回す。
            ledger.note_comm_failure(key, "lookup: submit outcome still unknown")?;
            return Ok(Driven::Deferred);
        }
        if existing.is_none() {
            // 外部に何も無いので、workspace は初期 snapshot から作り直してよい。
            let _ = fs::remove_dir_all(out.join(rel));
            seeder.make(req.problem_id, req.workspace)?;
        } else if !req.workspace.exists() {
            seeder.make(req.problem_id, req.workspace)?;
        }
        let t = match existing {
            Some(t) => t,
            None => match exec.submit(req, &idem) {
                Ok(t) => t,
                Err(e) => match e.kind {
                    ErrKind::Refused => return refuse(ledger, key, e),
                    ErrKind::NotSent => {
                        ledger.note_comm_failure(key, &format!("submit: {e}"))?;
                        return Ok(Driven::Deferred);
                    }
                    ErrKind::Unknown | ErrKind::Interrupted => {
                        ledger.mark_submit_unknown(key, &format!("submit: {e}"))?;
                        return Ok(Driven::Deferred);
                    }
                },
            },
        };
        ledger.mark_submitted(key, &t)?;
        if let Some((g, id)) = shared {
            g.mark_submitted(id)?;
        }
        task = Some(t);
    }
    let t = task.ok_or_else(|| String::from("task ref missing"))?;
    // 投入済み: 以降の失敗は通信断。台帳の task・冪等キー・既観測 usage・開始時刻・予約を保ち、次回また追う。
    let fresh = match exec.track(&t) {
        Ok(f) => f,
        Err(e) => return interrupted(ledger, key, "track", e),
    };
    if fresh != t {
        ledger.mark_waiting(key, &fresh)?;
    }
    match exec.poll(&fresh) {
        Err(e) => interrupted(ledger, key, "poll", e),
        Ok(ExecStatus::Done(r)) => {
            let seen = match exec.observed(&fresh) {
                Ok(seen) => seen,
                Err(e) => return interrupted(ledger, key, "observed", ExecError::interrupted(e)),
            };
            ledger.note_comm_ok(key)?;
            Ok(Driven::Done(r, seen))
        }
        Ok(ExecStatus::Wait) => {
            // poll ごとに子 task・job を含む累積使用量を見て、上限超過なら cancel する。
            let usage = match exec.usage(&fresh) {
                Ok(u) => u,
                Err(e) => return interrupted(ledger, key, "usage", e),
            };
            ledger.note_comm_ok(key)?;
            if let Some(Usage { tokens, wall_secs }) = usage {
                ledger.record_usage(key, ObservedUsage { tokens, wall_secs })?;
                if let Some(reason) = breach_reason(
                    budget,
                    arm,
                    tokens.and_then(|u| u.count(budget.token_basis)),
                    wall_secs,
                ) {
                    if let Some(row) = ledger.get(key).cloned() {
                        cancel_row(exec, ledger, &row, reason)?;
                    }
                    let seen = exec.observed(&fresh).unwrap_or_default();
                    return Ok(Driven::Cancelled(
                        ExecResult {
                            outcome: Outcome::Cancelled,
                            tokens,
                            cost_micro_usd: None,
                            wall_secs,
                        },
                        reason,
                        seen,
                    ));
                }
            }
            if ledger.get(key).map(|r| r.state) != Some(LedgerState::Waiting) {
                ledger.mark_waiting(key, &fresh)?;
            }
            Ok(Driven::Deferred)
        }
    }
}

/// 確実な未投入で再試行しても通らない拒否: 外部に何も無いので台帳を閉じ、その試行だけ error にする。
fn refuse(ledger: &mut Ledger, key: &TrialKey, e: ExecError) -> Result<Driven, String> {
    ledger.mark_cancelled(key, REASON_EXECUTOR_ERROR)?;
    Ok(Driven::Refused(e.msg))
}

/// 投入済みの試行との通信断: 失敗を台帳に残し、結果は出さずに次回へ回す（消費 0 にも完了にもしない）。
fn interrupted(
    ledger: &mut Ledger,
    key: &TrialKey,
    what: &str,
    e: ExecError,
) -> Result<Driven, String> {
    ledger.note_comm_failure(key, &format!("{what}: {e}"))?;
    Ok(Driven::Deferred)
}

/// 系列台帳の固定位置は期待 lock の `series` 欄から取る（lock が無い副比較は拒否される）。
pub fn run(
    m: &CampaignManifest,
    input: &InputManifest,
    exec: &dyn Executor,
    opts: &RunOptions,
) -> Result<RunReport, String> {
    let pin = opts.lock.as_ref().and_then(|l| l.series.clone());
    run_with_series(m, input, exec, opts, pin.as_ref())
}

/// 副比較（B1/C1）は系列台帳の固定位置 `series` が必須（無ければ副作用の前に拒否）。主比較は使わない。
pub fn run_with_series(
    m: &CampaignManifest,
    input: &InputManifest,
    exec: &dyn Executor,
    opts: &RunOptions,
    series: Option<&crate::series::SeriesPin>,
) -> Result<RunReport, String> {
    m.check_against_input(input)?;
    // 共有承認の manifest は宣言した台帳でしか走らない（省略・別 path・結び付き不明の lock は台帳に触れる前に拒否）。
    m.check_shared_ledger(opts.global_ledger.as_deref())?;
    if let Some(l) = &opts.lock {
        l.check_shared_ledger(m)?;
    }
    // 副比較: 出力先が許可 root の中であること・系列台帳が読めること・同じ系列の他 run が動いていないことを、
    // 副作用（out の作成・executor・台帳）より前に確かめる。
    if m.is_side()
        && let Some(l) = &opts.lock
        && l.series.as_ref() != series
    {
        return Err(
            "campaign lock series pin differs from the run's series ledger; refusing".into(),
        );
    }
    let mut ledger_guard = self::series::begin(m, series, &opts.out)?;
    let prior = ledger_guard
        .as_ref()
        .map(|s| s.2.clone())
        .unwrap_or_default();
    let r = run_inner(m, input, exec, opts, &prior);
    // 成否にかかわらず、この out の消費（確定 + 予約）を固定の系列台帳へ追記する（out の実体があるときだけ）。
    if let Some((s, real, _)) = &mut ledger_guard {
        let rec = s.record(real, &m.budget);
        return match (r, rec) {
            (Ok(r), Ok(())) => Ok(r),
            (Err(e), _) | (Ok(_), Err(e)) => Err(e),
        };
    }
    r
}

fn run_inner(
    m: &CampaignManifest,
    input: &InputManifest,
    exec: &dyn Executor,
    opts: &RunOptions,
    prior: &budget::Spend,
) -> Result<RunReport, String> {
    let hash = m.compute_sha256()?;
    let out = &opts.out;
    // 全体台帳: 開始の登録と検証（取り分を記録し、横断上限に収まらない測定は何かする前に拒否する）。
    // 投入の可否は開始時の snapshot でなく、毎投入の `admit_shared`（最新の全行読取・検査・予約を 1 つの排他）で決める。
    let global = opts
        .global_ledger
        .as_deref()
        .map(GlobalLedger::open)
        .transpose()?;
    if let Some(g) = &global {
        g.admit_start(m)?;
    }
    // 副作用（out の作成・snapshot・台帳・executor）より前に、lock と現在の実体を照合する。
    let lock = opts.lock.as_ref();
    let start = match lock {
        Some(l) => {
            lock_exec::check_delivery(l, &opts.case_inputs)?;
            let sources = lock_exec::Sources {
                repo: opts.repo_src.as_deref(),
                materials: opts.materials_src.as_deref(),
                kb: opts.kb_src.as_deref(),
                inputs: opts.inputs_src.as_deref(),
            };
            Some((l, lock_exec::begin(m, l, out, sources)?))
        }
        None => None,
    };
    // B1/C1 は固定の系列台帳の既消費（他の out・hash の分）を総枠へ合算する。超過なら何も実行しない。
    if prior.tokens >= m.budget.max_tokens || prior.wall_secs >= m.budget.max_wall_secs {
        return Err(format!(
            "series {} already consumed {} tokens / {}s of the approved total {} tokens / {}s in the pinned series ledger (other outs/hashes); refusing to run",
            m.campaign_id,
            prior.tokens,
            prior.wall_secs,
            m.budget.max_tokens,
            m.budget.max_wall_secs
        ));
    }
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let meta_path = out.join(META_FILE);
    if meta_path.exists() {
        let v: Value = serde_json::from_slice(&fs::read(&meta_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("run-meta.json: {e}"))?;
        // 同じ測定 ID で有効 arm・比較組・set 宣言・有効設定を変えた再開は、変えた欄を名指しして拒否する。
        if let Some(prior) = v.get("plan_identity")
            && let Ok(prior) =
                serde_json::from_value::<crate::manifest::PlanIdentity>(prior.clone())
        {
            m.plan_identity()?.check_resume(&prior)?;
        }
        if v["campaign_sha256"] != hash {
            return Err("out dir belongs to a different campaign hash; use an empty dir".into());
        }
    }
    if let Some((l, lock_exec::Start::Fresh)) = start {
        lock_exec::save(out, l)?;
    }
    let snapshot = prepare_snapshot(m, opts, lock)?;
    let (repo, repo_sha, kb_sha) = (snapshot.seed, snapshot.seed_sha, snapshot.kb_sha);
    let repo_free = lock.map(lock_exec::repo_free_cases).unwrap_or_default();
    let empty_sha = snapshot.empty.as_ref().map(|(_, h)| h.clone());
    let seeder = Seeder {
        repo: &repo,
        empty: snapshot.empty.as_ref().map(|(p, _)| p.as_path()),
        repo_free: &repo_free,
        stage: out.join("snapshot").join(INPUTS_DIR),
        inputs: &opts.case_inputs,
    };
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
            "plan_identity": m.plan_identity()?,
            "input_sha256": input.sha256,
            "split": m.split,
            "executor": exec.name(),
            "evidence_kind": evidence_kind(exec.simulated()),
            "real_performance": !exec.simulated(),
            "locked": lock.is_some() || out.join(lock_exec::LOCK_FILE).exists(),
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
    publish_global(global.as_ref(), m, &hash, &records, &ledger)?;
    // trials.jsonl に記録済みなのに台帳が未完了のまま（記録直後の crash）は done に直す。
    let stale: Vec<TrialKey> = ledger
        .unfinished()
        .into_iter()
        .filter(|r| r.key.manifest_sha256 == hash && r.state != LedgerState::CancelPending)
        .filter(|r| done.contains(&(r.key.arm, r.key.problem_id.clone(), r.key.rep)))
        .map(|r| r.key.clone())
        .collect();
    for k in stale {
        ledger.mark_done(&k)?;
    }
    // 前回までに cancel を要求して停止を確認できていない行は、新しい仕事より先に続きを試す。
    for row in ledger
        .unfinished()
        .into_iter()
        .filter(|r| r.key.manifest_sha256 == hash && r.state == LedgerState::CancelPending)
        .cloned()
        .collect::<Vec<_>>()
    {
        cancel_row(exec, &mut ledger, &row, REASON_BUDGET_STOP)?;
    }
    // 前回の停止で精算できなかった共有予約を、ローカルの記録・台帳と照合して精算する（証明が無ければ持ち続ける）。
    if let Some(g) = &global {
        reconcile_shared(g, exec, m, &hash, &records, &ledger)?;
    }
    let mut rep = RunReport {
        resumed_skipped: records.len() as u64,
        tokens_total: records.iter().filter_map(|r| r.tokens).sum(),
        wall_secs_total: records.iter().map(|r| r.wall_secs).sum(),
        // 再開時も記録から数え直す（summary.json は run ごとに書き直すため、今回の run だけの件数にしない）。
        no_artifact: records
            .iter()
            .filter(|r| r.reason.as_deref() == Some(REASON_NO_ARTIFACT))
            .count() as u64,
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
        // 新規投入の前に『残予算 = 上限 − 確定消費 − in-flight の予約』が予約額以上かを見る（追跡中の行は予約済み）。
        let gate = if exec.is_async()
            && m.termination.stop_on_budget
            && blocked_by.is_none()
            && !rep.budget_stopped
            && ledger.get(&key).is_none_or(|r| r.reserved.is_none())
        {
            let s =
                budget::spend(&records, &ledger, &m.budget, &hash, Some(&key)).plus_prior(prior);
            Some(budget::admit(&m.budget, &s, cid))
        } else {
            None
        };
        // 共有台帳の予約（毎投入。stop_on_budget・同期/非同期に関係なく、executor を呼ぶ前に fsync する）。
        // 他の測定 ID の最新の消費・予約・provider 枠と、台帳に残る最小の承認上限はここで効く。
        let arm_now = arm_use.get(&cid).copied().unwrap_or_default();
        let local_cap = match gate {
            Some(Gate::Admit(c)) => c,
            _ => effective_cap(&m.budget, arm_now.0, arm_now.1),
        };
        let shared = if blocked_by.is_none()
            && !rep.budget_stopped
            && !arm_exhausted(&m.budget, arm_now)
            && !matches!(gate, Some(Gate::Exhausted | Gate::Defer))
            && !(exec.is_async() && at_parallel_cap(&ledger, &key, &hash, m.budget.max_parallel))
        {
            admit_shared(
                global.as_ref(),
                m,
                &hash,
                &key,
                ledger.get(&key),
                local_cap.max_tokens,
            )?
        } else {
            SharedGate::Go(None)
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
        } else if gate == Some(Gate::Exhausted) {
            // 何も in-flight でないのに残予算が予約を下回る: 投入せず打ち切る（有料実行 0 件）。
            rep.budget_stopped = true;
            rec.reason = Some(REASON_BUDGET_STOP.into());
            rep.budget_cancelled += 1;
        } else if let SharedGate::Exhausted(why) = &shared {
            // 共有の承認上限に収まらず、待っても空く予約が無い: 投入せず打ち切る（有料実行 0 件）。
            rep.budget_stopped = true;
            rec.reason = Some(REASON_BUDGET_STOP.into());
            rec.error = Some(clip_error(why));
            rep.budget_cancelled += 1;
        } else if matches!(shared, SharedGate::Defer) {
            // 共有の枠（tokens・試行数・provider 枠）が他の未精算予約で埋まっている: 解けるまで投入しない。
            rep.waiting += 1;
            rep.reserve_deferred += 1;
            continue;
        } else if gate == Some(Gate::Defer) {
            // 先行の予約（in-flight・停止未確認）が解けるまで投入しない。次の run が回収してから再判定する。
            rep.waiting += 1;
            rep.reserve_deferred += 1;
            continue;
        } else {
            let arm = arm_now;
            let mut cap = local_cap;
            let shared_id = match &shared {
                SharedGate::Go(Some((id, granted))) => {
                    // 実投入の上限は共有台帳で押さえた額を超えない（再開時は前回の予約額）。
                    cap.max_tokens = cap.max_tokens.min(*granted);
                    Some(id.clone())
                }
                _ => None,
            };
            let rel = format!("trials/{cid:?}/{pid}/rep{rep_no}");
            let ws = out.join(&rel).join("workspace");
            // 試行の KB は workspace の中（初期 snapshot の複製）。別 dir に置くと実行器が参照できない。
            let kbd = ws.join("kb");
            let seed_sha = if repo_free.contains(&pid) {
                empty_sha.clone().unwrap_or_default()
            } else {
                repo_sha.clone()
            };
            let is_async = exec.is_async();
            if !is_async {
                // 中断した試行の残骸があれば捨て、必ず初期 snapshot から作り直す。
                let _ = fs::remove_dir_all(out.join(&rel));
                seeder.make(&pid, &ws)?;
                rec.workspace_sha256_initial = hash_tree_excluding(&ws, &[INPUTS_DIR])?;
                rec.kb_sha256_initial = hash_tree(&kbd)?;
                if rec.workspace_sha256_initial != seed_sha || rec.kb_sha256_initial != kb_sha {
                    return Err(format!(
                        "trial dir {rel} does not start from the saved snapshot"
                    ));
                }
            } else {
                // 非同期: 作成・再利用は drive_async が台帳を見て決める（参照中は消さない）。
                rec.workspace_sha256_initial = seed_sha.clone();
                rec.kb_sha256_initial = kb_sha.clone();
            }
            rec.inputs = seeder.specs(&pid).to_vec();
            rec.workspace = format!("{rel}/workspace");
            rec.kb = format!("{rel}/workspace/kb");
            let timeout = m.termination.trial_timeout_secs;
            let req = TrialRequest {
                campaign_id: &m.campaign_id,
                manifest_sha256: &hash,
                config: cfg,
                problem_id: &pid,
                prompt: prompts.get(pid.as_str()).copied().unwrap_or(""),
                trial_no: 1,
                rep: rep_no,
                workspace: &ws,
                kb: &kbd,
                timeout_secs: timeout,
                budget: cap,
                variant: m.variant.as_ref(),
            };
            let mut cancelled_by: Option<&'static str> = None;
            let mut errored: Option<String> = None;
            let failed_result = || ExecResult {
                outcome: Outcome::Error,
                tokens: None,
                cost_micro_usd: None,
                wall_secs: 0,
            };
            // routing は投入前に決める。Err ならその試行だけ error（task は作らない）。
            let pinned = exec.routing(&req);
            if let Ok(v) = &pinned
                && cid != ConfigId::A
                && !v.is_null()
            {
                rec.routing = Some(v.clone());
            }
            let mut res = if let Err(e) = pinned {
                // 投入しない。記録の hash 検査のため、初期 snapshot のままの作業場所だけ用意する。
                if is_async {
                    seeder.make(&pid, &ws)?;
                }
                errored = Some(e);
                failed_result()
            } else if is_async && at_parallel_cap(&ledger, &key, &hash, m.budget.max_parallel) {
                // in-flight が max_parallel に達している: 投入しない。次の run が先の試行を回収し、
                // 予算・使用量を見てから出す（ADR 2026-10-16-mlsys-pilot-wiring-fixes D3）。
                rep.waiting += 1;
                rep.parallel_deferred += 1;
                continue;
            } else if is_async {
                match drive_async(
                    exec,
                    &mut ledger,
                    &key,
                    &req,
                    out,
                    &rel,
                    &seeder,
                    &m.budget,
                    arm,
                    global.as_ref().zip(shared_id.as_ref()),
                )? {
                    Driven::Refused(e) => {
                        errored = Some(e);
                        failed_result()
                    }
                    Driven::Done(r, seen) => {
                        rec.observed = seen;
                        r
                    }
                    Driven::Cancelled(r, why, seen) => {
                        cancelled_by = Some(why);
                        rec.observed = seen;
                        r
                    }
                    Driven::Deferred => {
                        // 外部待ち・通信断: 台帳が参照中なので workspace は消さず、再実行で同じ task を追う。
                        rep.waiting += 1;
                        continue;
                    }
                }
            } else {
                match exec.execute(&req) {
                    Err(e) => {
                        // 同期 executor の失敗は実行したかを証明しない: 共有予約は投入済みとして保守的に精算する。
                        if let (Some(g), Some(id)) = (&global, &shared_id) {
                            g.mark_submitted(id)?;
                        }
                        errored = Some(e);
                        failed_result()
                    }
                    Ok(ExecStatus::Done(r)) => r,
                    Ok(ExecStatus::Wait) => {
                        // 外部待ち: 記録を残さず、残骸も消して、再実行（resume）で改めて実行する。
                        // executor が実行しなかったと返したので、共有予約は消費 0 で解放する。
                        if let (Some(g), Some(id)) = (&global, &shared_id) {
                            g.settle(id, Terminal::NotSubmitted)?;
                        }
                        let _ = fs::remove_dir_all(out.join(&rel));
                        rep.waiting += 1;
                        continue;
                    }
                }
            };
            if let Some(e) = &errored {
                rec.reason = Some(REASON_EXECUTOR_ERROR.into());
                rec.error = Some(clip_error(e));
                rep.executor_errors += 1;
                // 投入前に失敗して作業場所が無いときも初期 snapshot から作る（hash の失敗で本当の原因を隠さない）。
                if !ws.exists() {
                    seeder.make(&pid, &ws)?;
                }
            }
            if res.wall_secs > timeout && res.outcome != Outcome::Timeout {
                res.outcome = Outcome::Timeout;
                rec.reason = Some("runner_timeout".into());
            }
            res.wall_secs = res.wall_secs.min(timeout);
            let ws_final = hash_tree(&ws)?;
            // A の成果物は試行の workspace にしか出ない。exit 0 でも何も変わっていなければ合格にしない（fail-closed）。
            // 配った入力（と、その展開）は成果物に数えない。
            if cid == ConfigId::A
                && res.outcome == Outcome::Passed
                && hash_tree_excluding(&ws, &[INPUTS_DIR])? == seed_sha
            {
                res.outcome = Outcome::Failed;
                rec.reason = Some(REASON_NO_ARTIFACT.into());
                rep.no_artifact += 1;
            }
            let counted = res.tokens.and_then(|u| u.count(m.budget.token_basis));
            // 同期 executor（と実行中に観測できなかった async）は完了後に上限を判定する。Timeout が優先。
            if cancelled_by.is_none() && errored.is_none() && res.outcome != Outcome::Timeout {
                cancelled_by = breach_reason(&m.budget, arm, counted, res.wall_secs);
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
            if let Some(t) = counted {
                rep.max_overshoot_tokens = rep
                    .max_overshoot_tokens
                    .max(t.saturating_sub(cap.max_tokens));
            }
            let charged = if errored.is_some() {
                0
            } else {
                m.budget.charged_tokens(counted)
            };
            let e = arm_use.entry(cid).or_default();
            e.0 += charged;
            e.1 += res.wall_secs;
            rep.tokens_charged_total += charged;
            rep.tokens_missing += u64::from(counted.is_none() && errored.is_none());
            rec.outcome = res.outcome;
            rec.passed = res.outcome == Outcome::Passed;
            rec.tokens = counted;
            rec.token_usage = res.tokens;
            rec.cost_micro_usd = res.cost_micro_usd;
            rec.wall_secs = res.wall_secs;
            rec.workspace_sha256_final = ws_final;
            rec.kb_sha256_final = hash_tree(&kbd)?;
            rep.executed += 1;
            rep.tokens_total += counted.unwrap_or(0);
            rep.wall_secs_total += res.wall_secs;
            rep.budget_stopped = over(&rep);
        }
        append(&trials_path, &rec)?;
        // cancel の停止確認待ち（cancel_pending）は done にしない。terminal にできるのは確認後だけ。
        if ledger
            .get(&key)
            .is_some_and(|r| !r.state.is_terminal() && r.state != LedgerState::CancelPending)
        {
            ledger.mark_done(&key)?;
        }
        records.push(rec);
        if let Some(g) = &global {
            settle_shared(g, m, &hash, &records, &ledger, &key)?;
        }
        publish_global(global.as_ref(), m, &hash, &records, &ledger)?;
    }
    // 打ち切り後に残った未完了の台帳行（前提待ちで今回の走査に来なかったもの）も cancel する。
    if rep.budget_stopped {
        for row in ledger.unfinished().into_iter().cloned().collect::<Vec<_>>() {
            if row.key.manifest_sha256 != hash
                || records.iter().any(|r| {
                    (r.config, &r.problem_id, r.rep)
                        == (row.key.arm, &row.key.problem_id, row.key.rep)
                })
            {
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
            if let Some(g) = &global {
                settle_shared(g, m, &hash, &records, &ledger, &row.key)?;
            }
        }
    }
    write_summary(m, &hash, &records, &rep, &ledger, out, exec.simulated())?;
    Ok(rep)
}

/// 共有台帳の投入許可の結果。
enum SharedGate {
    /// 投入してよい（共有台帳なしは None。ありは attempt と予約額）。
    Go(Option<(AttemptId, u64)>),
    /// 他の未精算予約が解けるまで投入しない。
    Defer,
    /// 承認上限に収まらず、待っても空かない。
    Exhausted(String),
}

/// 毎投入の共有予約。最新の全行を読み、最小の承認上限で tokens・試行数・provider 枠（1 試行 1 枠）を検査して
/// 予約を fsync する（1 つの排他）。同じ試行の未精算予約（同一冪等キー）があればそれを続ける。
/// ローカル台帳が投入済みかもしれないと言う試行（task がある・結果不明・追跡中）は拒否で見捨てず載せる。
fn admit_shared(
    g: Option<&GlobalLedger>,
    m: &CampaignManifest,
    hash: &str,
    key: &TrialKey,
    local: Option<&LedgerRow>,
    tokens: u64,
) -> Result<SharedGate, String> {
    let Some(g) = g else {
        return Ok(SharedGate::Go(None));
    };
    let adopt = local.is_some_and(|r| r.task_id.is_some() || r.state != LedgerState::Planned);
    let a = TrialAdmission {
        measurement_id: m.campaign_id.clone(),
        trial: key.idempotency_key(),
        manifest_sha256: hash.to_string(),
        caps: Caps::of(m),
        tokens,
        provider_slots: 1,
        adopt,
    };
    Ok(match g.admit_trial(&a)? {
        Admission::Granted { id, tokens, .. } => SharedGate::Go(Some((id, tokens))),
        Admission::Rejected(r) if r.outstanding => SharedGate::Defer,
        Admission::Rejected(r) => SharedGate::Exhausted(r.msg),
    })
}

/// 試行の記録から精算する消費。tokens 欠測は 0 にせず予約額と既観測の大きい方（保守的）。
fn shared_usage(
    rec: &TrialRecord,
    row: &AttemptRow,
    local: Option<&LedgerRow>,
    b: &Budget,
) -> crate::global_ledger::Usage {
    let seen = local.and_then(|r| r.observed_usage);
    let seen_tokens = seen
        .and_then(|o| o.tokens)
        .and_then(|u| u.count(b.token_basis))
        .unwrap_or(0);
    let tokens = match rec.tokens {
        Some(t) if rec.reason.as_deref() != Some(REASON_EXECUTOR_ERROR) => t.max(seen_tokens),
        _ => row.reserved_tokens.max(seen_tokens),
    };
    crate::global_ledger::Usage {
        tokens,
        wall_secs: rec.wall_secs.max(seen.map_or(0, |o| o.wall_secs)),
        ran: true,
    }
}

/// 1 試行の共有予約の精算。ローカルが終端（記録あり・台帳が終端か無し）のときだけ精算する。
/// - cancel の停止未確認（cancel_pending）・追跡中の行は精算しない（予約と枠を持ち続ける）。
/// - 実行しなかった記録（executor_error・budget_stop）で共有行が Reserved のまま、かつローカル台帳が投入を
///   示していない（行が無い・task 無しで閉じた）ときだけ消費 0 で解放する。
/// - それ以外は記録の消費（欠測は保守的）で精算する。同じ attempt の再精算は置換なので二重加算しない。
fn settle_shared(
    g: &GlobalLedger,
    m: &CampaignManifest,
    hash: &str,
    records: &[TrialRecord],
    ledger: &Ledger,
    key: &TrialKey,
) -> Result<(), String> {
    let idem = key.idempotency_key();
    let Some(row) = g
        .outstanding(&m.campaign_id)?
        .into_iter()
        .find(|r| r.id.trial == idem)
    else {
        return Ok(());
    };
    let Some(rec) = records.iter().rev().find(|r| {
        r.campaign_sha256 == hash
            && (r.config, &r.problem_id, r.rep) == (key.arm, &key.problem_id, key.rep)
    }) else {
        return Ok(());
    };
    let local = ledger.get(key);
    if local.is_some_and(|r| !r.state.is_terminal()) {
        return Ok(());
    }
    let not_run = rec.reason.as_deref() == Some(REASON_EXECUTOR_ERROR) || !rec.ran();
    let unsent = row.state == AttemptState::Reserved
        && local.is_none_or(|r| r.task_id.is_none() && r.state == LedgerState::Cancelled);
    let t = if not_run && unsent {
        Terminal::NotSubmitted
    } else {
        Terminal::Finished(shared_usage(rec, &row, local, &m.budget))
    };
    g.settle(&row.id, t)
}

/// 再開時の照合: この測定 ID の未精算の共有予約を、ローカルの記録・台帳と突き合わせる。
/// - 記録がある: `settle_shared`（停止未確認なら持ち続ける）。
/// - ローカル台帳に行がある: 次の走査が同じ冪等キーで続きを追う（ここでは触らない）。
/// - どちらも無い: 非同期は冪等キーで照会し、投入が無いと確かめたときだけ解放する（照会失敗・投入ありは保持）。
///   同期は実行中に止まった可能性を否定できないので、予約額を消費として保守的に精算する。
fn reconcile_shared(
    g: &GlobalLedger,
    exec: &dyn Executor,
    m: &CampaignManifest,
    hash: &str,
    records: &[TrialRecord],
    ledger: &Ledger,
) -> Result<(), String> {
    let mut keys: BTreeMap<String, TrialKey> = BTreeMap::new();
    for (arm, problem_id, rep) in plan(m) {
        let k = TrialKey {
            manifest_sha256: hash.to_string(),
            problem_id,
            arm,
            trial_no: 1,
            rep,
        };
        keys.insert(k.idempotency_key(), k);
    }
    for r in ledger.rows() {
        keys.insert(r.key.idempotency_key(), r.key.clone());
    }
    for row in g.outstanding(&m.campaign_id)? {
        if let Some(k) = keys.get(&row.id.trial) {
            if ledger.get(k).is_some_and(|r| !r.state.is_terminal()) {
                continue;
            }
            let recorded = records.iter().any(|r| {
                r.campaign_sha256 == hash
                    && (r.config, &r.problem_id, r.rep) == (k.arm, &k.problem_id, k.rep)
            });
            if recorded {
                settle_shared(g, m, hash, records, ledger, k)?;
                continue;
            }
            if ledger.get(k).is_some() {
                continue;
            }
        }
        if row.state != AttemptState::Reserved {
            if !exec.is_async() {
                g.settle(&row.id, Terminal::Finished(conservative(&row)))?;
            }
            continue;
        }
        if exec.is_async() {
            // 共有予約 → ローカル予約 → 投入の順なので、ローカル行が無ければ投入していない。照会でも確かめる。
            if let Ok(None) = exec.lookup(&row.id.trial) {
                g.settle(&row.id, Terminal::NotSubmitted)?;
            }
        } else {
            g.settle(&row.id, Terminal::Finished(conservative(&row)))?;
        }
    }
    Ok(())
}

/// 消費を確かめられない attempt の保守的な精算（予約額を使い切り、試行として数える）。
fn conservative(row: &AttemptRow) -> crate::global_ledger::Usage {
    crate::global_ledger::Usage {
        tokens: row.reserved_tokens,
        wall_secs: 0,
        ran: true,
    }
}

/// 自分の確定消費（in-flight の予約を除く）を全体台帳へ公開する。台帳は単調に持つので再集計で加算も減少もしない。
fn publish_global(
    g: Option<&crate::global_ledger::GlobalLedger>,
    m: &CampaignManifest,
    hash: &str,
    records: &[TrialRecord],
    ledger: &Ledger,
) -> Result<(), String> {
    let Some(g) = g else { return Ok(()) };
    let s = budget::spend(records, ledger, &m.budget, hash, None);
    // 投入を拒否された試行（executor_error）は動いていない。同期 executor の Err は精算側が保守的に試行として
    // 数える（台帳は確定値の大きい方を取る）ので、ここで数えなくても過小にならない。
    let trials = records
        .iter()
        .filter(|r| r.ran() && r.reason.as_deref() != Some(REASON_EXECUTOR_ERROR))
        .count() as u64;
    g.record(&crate::global_ledger::GlobalLedger::published(
        m,
        hash,
        crate::global_ledger::Consumed {
            tokens: s.tokens.saturating_sub(s.held_tokens),
            wall_secs: s.wall_secs.saturating_sub(s.held_wall_secs),
            trials,
        },
    ))
}

/// runner 自身の集計（checker は別実装で再計算して突き合わせる）。
fn write_summary(
    m: &CampaignManifest,
    hash: &str,
    records: &[TrialRecord],
    rep: &RunReport,
    ledger: &Ledger,
    out: &Path,
    simulated: bool,
) -> Result<(), String> {
    let held = budget::spend(records, ledger, &m.budget, hash, None);
    let mut cells = serde_json::Map::new();
    for c in m.active_arms() {
        // 分母 = 有効 arm の計画された試行総数（problems × repeats、C は subset × repeats）。
        let n = plan(m).iter().filter(|(k, _, _)| *k == c).count();
        let mut counts = [0u64; 6]; // passed failed error timeout cancelled blocked
        let mut seen = 0u64;
        // tokens の内訳の和（cache が欠測の試行は件数を別に数える）。
        let (mut tin, mut tout, mut tcr, mut tcc, mut usage_missing, mut cache_missing) =
            (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
        for r in records.iter().filter(|r| r.config == c && r.ran()) {
            match r.token_usage {
                Some(u) => {
                    tin += u.input;
                    tout += u.output;
                    tcr += u.cache_read.unwrap_or(0);
                    tcc += u.cache_creation.unwrap_or(0);
                    cache_missing +=
                        u64::from(u.cache_read.is_none() || u.cache_creation.is_none());
                }
                None => usage_missing += 1,
            }
        }
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
                   "timeout": counts[3], "cancelled": counts[4], "blocked": counts[5], "missing": (n as u64).saturating_sub(seen),
                   "tokens": {"input": tin, "output": tout, "cache_read": tcr, "cache_creation": tcc,
                              "usage_missing_trials": usage_missing, "cache_missing_trials": cache_missing}}),
        );
    }
    write_json(
        &out.join(SUMMARY_FILE),
        &json!({
            "schema": "celeris.campaign-summary/1",
            "campaign_sha256": hash,
            "evidence_kind": evidence_kind(simulated),
            "real_performance": !simulated,
            "records": records.len(),
            "budget_stopped": rep.budget_stopped,
            "budget": {
                "token_basis": m.budget.token_basis,
                "max_parallel": m.budget.max_parallel,
                "parallel_deferred": rep.parallel_deferred,
                "reserve_deferred": rep.reserve_deferred,
                "reserved_in_flight": {"tokens": held.held_tokens, "wall_secs": held.held_wall_secs, "rows": held.in_flight},
                "remaining_tokens": m.budget.max_tokens.saturating_sub(held.tokens),
                "overshoot_formula": budget::OVERSHOOT_FORMULA,
                "no_artifact": rep.no_artifact,
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
