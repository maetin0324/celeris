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
    executor::{Executor, TrialRequest},
    manifest::{CampaignManifest, ConfigId},
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
    pub tokens_total: u64,
    pub wall_secs_total: u64,
    pub budget_stopped: bool,
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

/// 実行すべき試行の一覧（problem 昇順 → A,B,C → 試行番号）。C は事前登録 subset だけ。
pub fn plan(m: &CampaignManifest) -> Vec<(ConfigId, String, u32)> {
    let n = m.normalized();
    let subset: BTreeSet<&str> = n.secondary.subset.iter().map(String::as_str).collect();
    let mut v = Vec::new();
    for p in &n.problems {
        for c in [ConfigId::A, ConfigId::B, ConfigId::C] {
            if c == ConfigId::C && !subset.contains(p.as_str()) {
                continue;
            }
            for t in 1..=m.termination.max_attempts_per_trial {
                v.push((c, p.clone(), t));
            }
        }
    }
    v
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
        .map(|r| (r.config, r.problem_id.clone(), r.trial_no))
        .collect();
    let mut rep = RunReport {
        resumed_skipped: records.len() as u64,
        tokens_total: records.iter().filter_map(|r| r.tokens).sum(),
        wall_secs_total: records.iter().map(|r| r.wall_secs).sum(),
        ..RunReport::default()
    };
    let over = |rep: &RunReport| {
        m.termination.stop_on_budget
            && (rep.tokens_total >= m.budget.max_tokens
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

    for (cid, pid, trial_no) in plan(m) {
        if done.contains(&(cid, pid.clone(), trial_no)) {
            continue;
        }
        safe_component(&pid)?;
        let cfg = cfgs.get(&cid).ok_or("config missing")?;
        let mut rec = TrialRecord {
            schema: TRIAL_SCHEMA.into(),
            campaign_sha256: hash.clone(),
            input_sha256: input.sha256.clone(),
            split: m.split,
            config: cid,
            problem_id: pid.clone(),
            trial_no,
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
        };
        if rep.budget_stopped {
            rec.reason = Some(REASON_BUDGET_STOP.into());
            rep.budget_cancelled += 1;
        } else {
            let rel = format!("trials/{cid:?}/{pid}/{trial_no}");
            let (ws, kbd) = (out.join(&rel).join("workspace"), out.join(&rel).join("kb"));
            // 中断した試行の残骸があれば捨て、必ず初期 snapshot から作り直す。
            let _ = fs::remove_dir_all(out.join(&rel));
            copy_tree(&repo, &ws)?;
            copy_tree(&kb, &kbd)?;
            rec.workspace = format!("{rel}/workspace");
            rec.kb = format!("{rel}/kb");
            rec.workspace_sha256_initial = hash_tree(&ws)?;
            rec.kb_sha256_initial = hash_tree(&kbd)?;
            if rec.workspace_sha256_initial != repo_sha || rec.kb_sha256_initial != kb_sha {
                return Err(format!(
                    "trial dir {rel} does not start from the saved snapshot"
                ));
            }
            let timeout = m.termination.trial_timeout_secs;
            let req = TrialRequest {
                config: cfg,
                problem_id: &pid,
                prompt: prompts.get(pid.as_str()).copied().unwrap_or(""),
                trial_no,
                workspace: &ws,
                kb: &kbd,
                timeout_secs: timeout,
            };
            let mut res = exec.execute(&req)?;
            if res.wall_secs > timeout && res.outcome != Outcome::Timeout {
                res.outcome = Outcome::Timeout;
                rec.reason = Some("runner_timeout".into());
            }
            res.wall_secs = res.wall_secs.min(timeout);
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
        records.push(rec);
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
        let n = plan(m)
            .iter()
            .filter(|(k, _, t)| *k == c && *t == 1)
            .count();
        let mut counts = [0u64; 5]; // passed failed error timeout cancelled
        let mut seen = 0u64;
        for r in records.iter().filter(|r| r.config == c && r.trial_no == 1) {
            seen += 1;
            counts[match r.outcome {
                Outcome::Passed => 0,
                Outcome::Failed => 1,
                Outcome::Error => 2,
                Outcome::Timeout => 3,
                Outcome::Cancelled => 4,
            }] += 1;
        }
        cells.insert(
            format!("{c:?}"),
            json!({"n": n, "passed": counts[0], "failed": counts[1], "error": counts[2],
                   "timeout": counts[3], "cancelled": counts[4], "missing": (n as u64).saturating_sub(seen)}),
        );
    }
    write_json(
        &out.join(SUMMARY_FILE),
        &json!({
            "schema": "celeris.campaign-summary/1",
            "campaign_sha256": hash,
            "records": records.len(),
            "budget_stopped": rep.budget_stopped,
            "cells": Value::Object(cells),
        }),
    )
}
