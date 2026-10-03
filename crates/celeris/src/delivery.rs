//! ADR-0051: 部署レビュアーのマージ可否判定 → merge → release/verify。
//! 判断は既存の独立したReviewer run。ここは永続状態を決定的に進めるだけで promote は呼ばない。
use crate::config::Config;
use std::{
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use task_core::{
    Delivery, DeliveryState as State, Event, Message, MessageId, MessageRole, RepairClass, Status,
    StoreError, TaskStore, WorkUnitKind, WorkUnitStatus,
};
use task_ops::changes::{git, git_with_env};
use time::OffsetDateTime;

/// ADR-0051 Phase 106追記: push の待ち時間。ローカルの merge/rev-parse より長め
/// （ssh 越しの origin を想定。`BatchMode=yes` で対話的な認証には絶対に落ちない）。
const PUSH_TIMEOUT: Duration = Duration::from_secs(120);
/// 1度だけ再試行したがなお失敗した `push_error` の目印。通知文では外して見せる
/// （[`push_error_display`]）。この目印が付いていたら以後は触らない。
const PUSH_RETRIED_PREFIX: &str = "[retried] ";
const MERGE_BASE_FAILURE: &str = "[merge-base] ";
/// 承認 SHA の早送り（`git merge --ff-only`）の待ち時間。差分の全ファイルを作業ツリーに書くので、
/// NFS 上の自己リポジトリで数百ファイルの差分だと 20 秒では足りない（2026-09-30: 406 ファイルの
/// 配送が途中で kill され、`index.lock` と書きかけの作業ツリーが残った）。
const MERGE_TIMEOUT: Duration = Duration::from_secs(600);

/// 配送で局所修復できる失敗だけを分類する。gate の他の step は従来の Reopen に残す。
fn classify_delivery_failure(
    state: State,
    detail: &str,
    failed_step: Option<&str>,
) -> Option<RepairClass> {
    if state != State::Blocked {
        return None;
    }
    if detail.starts_with(MERGE_BASE_FAILURE) {
        return Some(RepairClass::MergeBase);
    }
    if detail.starts_with("リリース準備に失敗") && failed_step == Some("cargo-fmt-check") {
        return Some(RepairClass::Format);
    }
    None
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String, String> {
    git_text_within(repo, args, Duration::from_secs(20))
}
fn git_text_within(repo: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let out = git(repo, args, timeout).ok_or("git を起動できません")?;
    if out.ok {
        Ok(out.stdout.trim().into())
    } else if out.timed_out {
        Err(format!(
            "git {} timed out after {}s (killed)",
            args.join(" "),
            timeout.as_secs()
        ))
    } else {
        Err(out.stderr.chars().take(1000).collect())
    }
}
fn sha(repo: &Path, reference: &str) -> Result<String, String> {
    git_text(repo, &["rev-parse", "--verify", reference])
}

fn push_error_display(err: &str) -> &str {
    err.strip_prefix(PUSH_RETRIED_PREFIX).unwrap_or(err)
}

/// stderr の末尾 `max` バイト（文字境界を守る）。
fn tail_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// origin が既にこのブランチと同じかそれより先か（省略できるか）。判定できなければ
/// （リモート追跡ブランチが無い等）省略しない。
fn push_already_up_to_date(repo: &Path, remote: &str, branch: &str) -> bool {
    git_text(
        repo,
        &[
            "rev-list",
            "--count",
            &format!("{remote}/{branch}..{branch}"),
        ],
    )
    .map(|s| s.trim() == "0")
    .unwrap_or(false)
}

/// ADR-0051 Phase 106: `merge_reviewed` が成功した直後、release.sh を起こす前に呼ぶ純粋な git 呼び出し。
/// 非対話（`git()` が既定で `GIT_TERMINAL_PROMPT=0` を設定。ここでは `GIT_SSH_COMMAND` に
/// `-o BatchMode=yes` を足す）。force push はしない。
fn push_merged(repo: &Path, remote: &str, branch: &str, timeout: Duration) -> Result<(), String> {
    if push_already_up_to_date(repo, remote, branch) {
        return Ok(());
    }
    let ssh_command = match std::env::var("GIT_SSH_COMMAND") {
        Ok(existing) if !existing.trim().is_empty() => format!("{existing} -o BatchMode=yes"),
        _ => "ssh -o BatchMode=yes".to_string(),
    };
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    match git_with_env(
        repo,
        &["push", remote, &refspec],
        &[("GIT_SSH_COMMAND", ssh_command.as_str())],
        timeout,
    ) {
        Some(o) if o.ok => Ok(()),
        Some(o) => Err(tail_bytes(o.stderr.trim(), 500)),
        None => Err("git を起動できません".into()),
    }
}

/// push が失敗して（まだ再試行の目印が付いていなければ）1度だけ再試行する。release の準備を
/// 止めないので、これは `advance` の状態遷移そのものではなく独立した CAS 保存。
fn maybe_retry_push(
    store: &dyn TaskStore,
    config: &Config,
    old: &Delivery,
    now: OffsetDateTime,
) -> Result<Delivery, StoreError> {
    if !config.selfdeploy.push || old.release.is_none() || old.pushed_at.is_some() {
        return Ok(old.clone());
    }
    let Some(err) = &old.push_error else {
        return Ok(old.clone());
    };
    if err.starts_with(PUSH_RETRIED_PREFIX) {
        return Ok(old.clone());
    }
    let mut next = old.clone();
    match push_merged(
        &config.selfdeploy.repo,
        &config.selfdeploy.push_remote,
        &old.default_branch,
        PUSH_TIMEOUT,
    ) {
        Ok(()) => {
            next.pushed_at = Some(now);
            next.push_error = None;
        }
        Err(e) => next.push_error = Some(format!("{PUSH_RETRIED_PREFIX}{e}")),
    }
    Ok(if store.delivery_save(Some(old), &next)? {
        next
    } else {
        old.clone()
    })
}
pub fn tick(store: &dyn TaskStore, config: &Config, now: OffsetDateTime) -> Result<(), StoreError> {
    for delivery in store.delivery_list()? {
        // ADR-0079 D6（Phase R1c）: 木の子 task は main に取り込まない（`task_ops::delivery::begin` が行を
        // 作らない。旧い行が残っていても merge / release に進めない保険）。
        if store
            .get(delivery.task_id)?
            .is_some_and(|t| task_core::tree::is_tree_child(&t))
        {
            continue;
        }
        if config
            .selfdeploy
            .delivery_projects
            .contains(&delivery.project_id.to_string())
        {
            advance(store, config, &delivery, now)?;
        }
    }
    Ok(())
}

fn validate_candidate(repo: &Path, d: &Delivery) -> Result<(), String> {
    if sha(repo, &format!("refs/heads/{}", d.branch))? != d.head
        || sha(repo, &format!("refs/heads/{}", d.default_branch))? != d.base
    {
        return Err(
            "対象コミットまたは既定ブランチが変わりました。更新して再レビューが必要です".into(),
        );
    }
    if let Err(stderr) = git_text(repo, &["merge-base", "--is-ancestor", &d.base, &d.head]) {
        return Err(format!(
            "git merge-base --is-ancestor {} {} failed; stderr: {stderr}",
            d.base, d.head
        ));
    }
    Ok(())
}

/// 承認した SHA だけを早送り。git 自身が無関係な未コミット変更を保持する。
fn merge_reviewed(repo: &Path, d: &Delivery) -> Result<(), String> {
    validate_candidate(repo, d)?;
    if task_ops::changes::current_branch(repo).as_deref() != Some(d.default_branch.as_str()) {
        return Err("自己リポジトリが既定ブランチをチェックアウトしていません".into());
    }
    // `--no-progress`: 進捗行（"Updating files: ..."）が stderr を埋めて本当の失敗理由を隠すのを防ぐ。
    git_text_within(
        repo,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "merge",
            "--ff-only",
            "--no-autostash",
            "--no-progress",
            &d.head,
        ],
        MERGE_TIMEOUT,
    )?;
    if sha(repo, "HEAD")? != d.head {
        return Err("取り込み後のSHAが一致しません".into());
    }
    Ok(())
}

fn make_repair(
    store: &dyn TaskStore,
    config: &Config,
    d: &Delivery,
    class: RepairClass,
    now: OffsetDateTime,
) -> Result<Option<String>, StoreError> {
    let units = store.work_units_for(d.task_id)?;
    let repairs: Vec<_> = units
        .iter()
        .filter(|u| u.kind == WorkUnitKind::Repair)
        .collect();
    let same_class = repairs
        .iter()
        .filter(|u| {
            u.spec
                .title
                .starts_with(&format!("repair ({}):", class.bucket()))
        })
        .count();
    if repairs.len() as u32 >= config.execution.max_repairs
        || same_class as u32 >= config.execution.max_repairs_per_class
    {
        return Ok(None);
    }
    let branch_head = sha(&config.selfdeploy.repo, &format!("refs/heads/{}", d.branch))
        .unwrap_or_else(|_| d.head.clone());
    let main_head = sha(
        &config.selfdeploy.repo,
        &format!("refs/heads/{}", d.default_branch),
    )
    .unwrap_or_else(|_| d.base.clone());
    let output = if class == RepairClass::Format {
        let log = config
            .selfdeploy
            .releases_dir
            .join(d.release.as_deref().unwrap_or(""))
            .join(".gate-cargo-fmt-check.log");
        std::fs::read_to_string(log)
            .map(|s| tail_bytes(&s, 4000))
            .unwrap_or_default()
    } else {
        tail_bytes(&d.detail, 4000)
    };
    let details = vec![
        format!("対象ブランチ {}: {}", d.branch, branch_head),
        format!("既定ブランチ {}: {}", d.default_branch, main_head),
        format!("失敗した検査の出力:\n{output}"),
    ];
    let (max_turns, max_wall_secs) = class.budget();
    let n = repairs.len() + 1;
    let key = format!("repair-{n}");
    let spec = task_core::WorkUnitSpec {
        key: key.clone(),
        kind: WorkUnitKind::Repair,
        title: format!("repair ({}): 配送の局所修復", class.bucket()),
        objective: task_core::build_repair_objective(class, &details, "配送", "", None, None),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: Default::default(),
        harness: None,
        features: None,
        budget: Some(task_core::WorkUnitBudget {
            max_turns: Some(max_turns),
            max_wall_secs: Some(max_wall_secs),
        }),
        outputs: vec![],
        phase: None,
    };
    let stamp = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| StoreError::Invalid(e.to_string()))?;
    let active = store.execution_plan_active(d.task_id)?;
    let (new_plan, rows, event) = if let Some(plan) = active {
        let row = task_core::WorkUnitRow::new(
            task_core::new_id(),
            d.task_id.to_string(),
            plan.id,
            units.iter().map(|u| u.seq).max().unwrap_or(0) + 1,
            spec,
            WorkUnitStatus::Ready,
            stamp,
        );
        let ev = Event::WorkUnitTransitioned {
            work_unit_id: row.id.clone(),
            key: key.clone(),
            from: WorkUnitStatus::Pending,
            to: WorkUnitStatus::Ready,
            reason: "delivery_repair".into(),
            run_id: None,
        };
        (None, vec![row], ev)
    } else {
        let task = store
            .get(d.task_id)?
            .ok_or_else(|| StoreError::Invalid("task missing".into()))?;
        let plan_id = task_core::new_id();
        let main = task_core::WorkUnitSpec {
            key: "main".into(),
            kind: WorkUnitKind::Implement,
            title: task.title,
            objective: task.objective,
            depends_on: vec![],
            done_when: vec![],
            checks: vec![],
            context: Default::default(),
            harness: None,
            features: None,
            budget: None,
            outputs: vec![],
            phase: None,
        };
        let main_row = task_core::WorkUnitRow::new(
            task_core::new_id(),
            d.task_id.to_string(),
            plan_id.clone(),
            0,
            main.clone(),
            WorkUnitStatus::Done,
            stamp.clone(),
        );
        let repair_row = task_core::WorkUnitRow::new(
            task_core::new_id(),
            d.task_id.to_string(),
            plan_id.clone(),
            1,
            spec.clone(),
            WorkUnitStatus::Ready,
            stamp.clone(),
        );
        let plan_spec = task_core::ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA.into(),
            rationale: "delivery repair: 暗黙の WorkUnit を実体化".into(),
            work_units: vec![main, spec],
            phases: Vec::new(),
            children: Vec::new(),
        };
        let plan = task_core::ExecutionPlanRow {
            id: plan_id.clone(),
            task_id: d.task_id.to_string(),
            version: 1,
            origin: task_core::PlanOrigin::Repair,
            planner_run_id: None,
            status: task_core::PlanStatus::Active,
            spec: plan_spec.clone(),
            created_at: stamp,
            superseded_at: None,
        };
        let ev = Event::ExecutionPlanned {
            plan_id,
            version: 1,
            origin: task_core::PlanOrigin::Repair,
            supersedes: None,
            reason: Some("delivery_repair".into()),
            plan: Box::new(plan_spec),
        };
        (Some(plan), vec![main_row, repair_row], ev)
    };
    store.delivery_repair_apply(d.task_id, vec![event], new_plan, rows)?;
    Ok(Some(key))
}

fn advance(
    store: &dyn TaskStore,
    config: &Config,
    old: &Delivery,
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    // ADR-0051 Phase 106追記: mergeが既に済んでいてpushが未成功・未再試行なら、状態遷移とは独立に
    // 1度だけ再試行する（`old` を最新の永続状態に差し替えてから通常の遷移へ進む）。
    let retried = maybe_retry_push(store, config, old, now)?;
    let old = &retried;
    let mut d = old.clone();
    let repo = &config.selfdeploy.repo;
    if old.state == State::MergeQueued {
        if !store
            .get(old.task_id)?
            .is_some_and(|t| t.status == Status::Done)
        {
            // 委譲した子や集約待ちを含め、元の仕事のdoneを待つ。
            return Ok(());
        }
        if let Some(marker) =
            task_ops::workspace::read_marker(&config.workspace_root.join(old.task_id.to_string()))
            && marker
                .repos
                .iter()
                .any(|r| task_ops::changes::is_dirty(Path::new(&r.dir)) != Some(false))
        {
            d.state = State::Blocked;
            d.detail = format!(
                "{MERGE_BASE_FAILURE}実装の作業ツリーに未コミットの変更があります。コミット後に再レビューしてください"
            );
            store.delivery_save(Some(old), &d)?;
            return Ok(());
        }
    }
    match old.state {
        State::Reviewing => {
            if store
                .get(old.task_id)?
                .is_some_and(|t| t.status.is_terminal())
            {
                d.state = State::Blocked;
                d.detail = "レビュアーのマージ判定が記録されず終了しました。部署内で実行結果を確認してください".into();
                store.delivery_save(Some(old), &d)?;
            }
        }
        State::MergeQueued => {
            d.state = State::Merging;
            if !store.delivery_save(Some(old), &d)? {
                return Ok(());
            }
            let claimed = d.clone();
            match merge_reviewed(repo, &d) {
                Ok(()) => {
                    d.state = State::Preparing;
                    d.release = Some(d.head[..12].into());
                    d.detail =
                        "部署内レビュー合格・マージ済み。ビルドとリリース検証を実行中です".into();
                    // ADR-0051 Phase 106: release.sh（start_prepare）を起こす前にoriginへpush。
                    // 失敗してもrelease準備は止めない（下のstart_prepareはこの結果を待たない）。
                    if config.selfdeploy.push {
                        match push_merged(
                            repo,
                            &config.selfdeploy.push_remote,
                            &d.default_branch,
                            PUSH_TIMEOUT,
                        ) {
                            Ok(()) => d.pushed_at = Some(now),
                            Err(e) => d.push_error = Some(e),
                        }
                    }
                }
                Err(e) => {
                    d.state = State::Blocked;
                    d.detail = format!("{MERGE_BASE_FAILURE}{e}");
                }
            }
            if !store.delivery_save(Some(&claimed), &d)? {
                return Ok(());
            }
            if d.state == State::Preparing {
                let mut integration = task_core::TaskIntegration::new(
                    d.task_id,
                    Some(d.repo_id),
                    &d.repo,
                    task_core::IntegrationMethod::Merge,
                    task_core::IntegrationState::Done,
                    now,
                )
                .with_detail(format!(
                    "部署のレビュアーの承認により {} を取り込み。worktreeとブランチは証跡として保持",
                    d.head
                ));
                integration.merged_at = Some(now);
                store.integration_put(&integration)?;
                start_prepare(store, config, &d)?;
            }
        }
        State::Merging => {
            // 再起動を跨いだ外部操作は自動で再実行しない。
            d.state = State::Blocked;
            d.detail = "取り込み処理が中断しました。gitと承認SHAの照合が必要です".into();
            store.delivery_save(Some(old), &d)?;
        }
        State::Preparing => {
            let dir = preparation_dir(config, old);
            if let Some(result) = read_json(&dir.join("result.json")) {
                let rel = config
                    .selfdeploy
                    .releases_dir
                    .join(old.release.as_deref().unwrap_or(""));
                let verified = read_json(&rel.join("verify.json")).is_some_and(|v| v["ok"] == true);
                let built = read_json(&rel.join("gate.json")).is_some_and(|v| v["ok"] == true);
                let same = read_json(&rel.join("manifest.json"))
                    .is_some_and(|v| v["sha"].as_str() == Some(&old.head));
                if result["ok"] == true && built && verified && same {
                    d.state = State::Ready;
                    d.detail = "部署内レビュー、マージ、ビルド、検証が完了しました。リリース画面のデプロイ操作を待っています（本番は未変更）".into();
                } else {
                    d.state = State::Blocked;
                    d.detail = format!(
                        "リリース準備に失敗しました。ログ: {}",
                        dir.join("prepare.log").display()
                    );
                }
                store.delivery_save(Some(old), &d)?;
            } else if !old.prepare_pid.is_some_and(crate::instance::pid_alive) {
                d.state = State::Blocked;
                d.detail = format!(
                    "リリース準備が中断しました。ログ: {}",
                    dir.join("prepare.log").display()
                );
                store.delivery_save(Some(old), &d)?;
            }
        }
        State::Ready | State::Blocked => {
            if old.notification.is_some() {
                return Ok(());
            }
            // repair WU とそのレビューが終わったら、新しい ref で配送を再検証する。
            let latest_repair = store
                .work_units_for(old.task_id)?
                .into_iter()
                .filter(|u| u.kind == WorkUnitKind::Repair)
                .max_by_key(|u| u.seq);
            let repaired_and_not_requeued = latest_repair
                .as_ref()
                .is_some_and(|u| u.status == WorkUnitStatus::Done)
                && !store.comments_for(old.task_id)?.iter().any(|c| {
                    latest_repair
                        .as_ref()
                        .is_some_and(|u| c.body == format!("[delivery-repair-requeued] {}", u.key))
                });
            if old.state == State::Blocked
                && old.decision == Some(true)
                && store
                    .get(old.task_id)?
                    .is_some_and(|t| t.status == Status::Done)
                && repaired_and_not_requeued
            {
                d.base = sha(repo, &format!("refs/heads/{}", d.default_branch))
                    .unwrap_or_else(|_| old.base.clone());
                d.head = sha(repo, &format!("refs/heads/{}", d.branch))
                    .unwrap_or_else(|_| old.head.clone());
                d.state = State::MergeQueued;
                d.release = None;
                d.prepare_pid = None;
                d.detail = "局所修復後の配送を再検証します".into();
                if store.delivery_save(Some(old), &d)?
                    && let Some(u) = latest_repair
                {
                    store.comment_add(
                        &task_core::TaskComment {
                            id: task_core::CommentId::new(),
                            task_id: old.task_id,
                            author_kind: task_core::CommentAuthorKind::System,
                            author: None,
                            run_id: None,
                            created_at: now,
                            body: format!("[delivery-repair-requeued] {}", u.key),
                        },
                        None,
                    )?;
                }
                return Ok(());
            }
            let needs_user =
                old.state == State::Ready || old.detail.trim_start().starts_with("[needs-human]");
            let node = if needs_user {
                store
                    .org_list()?
                    .into_iter()
                    .find(|n| n.kind == task_core::OrgKind::Secretary)
            } else {
                store.org_get(&old.department)?
            };
            let Some(node) = node else { return Ok(()) };
            // 技術的な差し戻しは通常のworker再試行が先。最終失敗/準備失敗だけを部署に戻す。
            if !needs_user
                && store
                    .get(old.task_id)?
                    .is_some_and(|t| !t.status.is_terminal())
            {
                return Ok(());
            }
            // マージ/ビルドの技術的な不備は実装担当へ一度だけ戻す。コメントと再開は同一transaction。
            const REPAIR: &str = "[delivery-repair]";
            if !needs_user
                && old.decision == Some(true)
                && store
                    .get(old.task_id)?
                    .is_some_and(|t| t.status == Status::Done)
            {
                let rel = config
                    .selfdeploy
                    .releases_dir
                    .join(old.release.as_deref().unwrap_or(""));
                let gate = read_json(&rel.join("gate.json"));
                let failed_step = gate
                    .as_ref()
                    .filter(|v| v["ok"] == false)
                    .and_then(|v| v["failed_step"].as_str());
                if let Some(class) = classify_delivery_failure(old.state, &old.detail, failed_step)
                {
                    // ADR 2026-10-02-parallel-integration-auto-resolve D2: merge_base 系は局所修復を作る前に定型衝突の自動解消を試す。
                    if class == RepairClass::MergeBase {
                        match auto_resolve::attempt(store, config, old)? {
                            auto_resolve::Outcome::Resolved {
                                base,
                                head,
                                actions,
                            } => {
                                auto_resolve::requeue(store, old, base, head, &actions, now)?;
                                return Ok(());
                            }
                            auto_resolve::Outcome::Fallback(fallback) => {
                                if auto_resolve::fall_back(store, config, old, &fallback, now)? {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    if let Some(key) = make_repair(store, config, old, class, now)? {
                        store.comment_add(&task_core::TaskComment {
                            id: task_core::CommentId::new(), task_id: old.task_id,
                            author_kind: task_core::CommentAuthorKind::System, author: None,
                            run_id: None, created_at: now,
                            body: format!("{REPAIR} {key} class={}。対象ブランチと既定ブランチ、失敗した検査だけを渡して局所修復します。", class.bucket()),
                        }, None)?;
                    } else {
                        let repairs = store.work_units_for(old.task_id)?;
                        let total = repairs
                            .iter()
                            .filter(|u| u.kind == WorkUnitKind::Repair)
                            .count();
                        let same_class = repairs
                            .iter()
                            .filter(|u| {
                                u.kind == WorkUnitKind::Repair
                                    && u.spec
                                        .title
                                        .starts_with(&format!("repair ({}):", class.bucket()))
                            })
                            .count();
                        let request = auto_resolve::limit_request(
                            config,
                            old,
                            format!(
                                "配送の局所修復が上限に達しました: {} (全体 {total}/{} 回、同分類 {same_class}/{} 回)",
                                class.bucket(),
                                config.execution.max_repairs,
                                config.execution.max_repairs_per_class
                            ),
                        );
                        auto_resolve::record_request(store, old, &request)?;
                    }
                    return Ok(());
                }
            }
            if !needs_user
                && old.decision == Some(true)
                && store
                    .get(old.task_id)?
                    .is_some_and(|t| t.status == Status::Done)
                && !store.comments_for(old.task_id)?.iter().any(|c| {
                    c.author_kind == task_core::CommentAuthorKind::System
                        && c.body.starts_with(REPAIR)
                })
            {
                store.comment_add(&task_core::TaskComment {
                    id:task_core::CommentId::new(),task_id:old.task_id,author_kind:task_core::CommentAuthorKind::System,author:None,run_id:None,created_at:now,
                    body:format!("{REPAIR} 部署内の取り込み・リリース検証で差し戻し。元の実装依頼を保持し、次の問題を修正してコミットと検証まで行ってください。必要なら自分のworktreeに現在の既定ブランチを取り込んでください。元のチェックアウトや本番サービスは変更せず、デプロイも実行しないでください。\n{}",old.detail)
                },Some((task_core::Trigger::Reopen,Vec::new())))?;
                return Ok(());
            }
            // review run由来の固定MessageIdで、追記後に落ちても重複しない。
            let id = MessageId(ulid::Ulid::from(
                u128::from(d.review_run.parse::<ulid::Ulid>().unwrap_or(d.task_id.0))
                    ^ 0x64656c6976657279u128,
            ));
            if !store
                .message_list(&node.id, Some(d.project_id), 1000)?
                .iter()
                .any(|m| m.id == id)
            {
                let title = store.get(d.task_id)?.map(|t| t.title).unwrap_or_default();
                let label = if d.state == State::Ready {
                    "デプロイ準備完了"
                } else {
                    "取り込み・リリース準備の確認が必要"
                };
                let release = d
                    .release
                    .as_ref()
                    .map(|s| format!("\n[リリース {s} を確認してデプロイ](/releases#release-{s})"))
                    .unwrap_or_default();
                // ADR-0051 Phase 106追記: origin へのpush状況を1行足す。push=falseで一度も
                // 試みていなければ何も足さない。
                let push_line = if let Some(head) = d.pushed_at.is_some().then(|| d.head.clone()) {
                    format!("\norigin へ push 済み: {head}")
                } else if let Some(err) = d.push_error.as_deref().map(push_error_display) {
                    format!("\n**push に失敗**: {err}。人が `git push` してください")
                } else {
                    String::new()
                };
                store.message_append(&Message { id, node_id: node.id, project_id: Some(d.project_id), role: MessageRole::Node, text: format!("【{label}・自動引き渡し結果】{title}\n{}\n[タスクと部署レビュー](/tasks/{}?tab=changes){}{}", d.detail, d.task_id, release, push_line), run_id: None, task_id: Some(d.task_id), metadata: None, created_at: now })?;
            }
            d.notification = Some(id);
            store.delivery_save(Some(old), &d)?;
        }
    }
    Ok(())
}
fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
fn preparation_dir(config: &Config, d: &Delivery) -> std::path::PathBuf {
    config
        .selfdeploy
        .releases_dir
        .join(".deliveries")
        .join(d.task_id.to_string())
        .join(&d.head)
}
fn start_prepare(store: &dyn TaskStore, config: &Config, old: &Delivery) -> Result<(), StoreError> {
    let dir = preparation_dir(config, old);
    let spawn = || -> std::io::Result<std::process::Child> {
        std::fs::create_dir_all(&dir)?;
        let log = std::fs::File::create(dir.join("prepare.log"))?;
        let mut cmd = Command::new("bash");
        cmd.arg(
            config
                .selfdeploy
                .releases_dir
                .parent()
                .unwrap_or(Path::new("."))
                .join("current/scripts/prepare.sh"),
        )
        .arg(&old.head)
        .arg(&dir)
        .env("SD_REPO", &config.selfdeploy.repo)
        .env(
            "CELERIS_STATE_DIR",
            config
                .selfdeploy
                .releases_dir
                .parent()
                .unwrap_or(Path::new(".")),
        )
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
        if let Some(path) = &config.source_path {
            cmd.env("CELERIS_CONFIG", path);
        }
        cmd.spawn()
    };
    let mut next = old.clone();
    match spawn() {
        Ok(mut child) => {
            next.prepare_pid = Some(child.id());
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => {
            next.state = State::Blocked;
            next.detail = format!("リリース準備を起動できません: {e}");
        }
    }
    store.delivery_save(Some(old), &next)?;
    Ok(())
}

mod auto_resolve;

#[cfg(test)]
#[path = "delivery/tests.rs"]
mod tests;
