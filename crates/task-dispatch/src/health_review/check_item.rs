//! ADR 2026-11-08-health-enforcement-loop D1.2・D2.2・D3.2・D3.4: WU の check と段の統合の検査の列の 1 項目の記録（開始・
//! 終了の event）と、列の最後に足す health の項目（`cmd` は [`HEALTH_CHECK_ID`]）。
//!
//! 両方の経路は `dispatcher::phase_integration::run_integration_checks` を通り、そこが検査の各項目を [`record_start`]・
//! [`record_finish`] で残し、最後に [`run_item`] で health を 1 回評価する（経路ごとに検査器を起こさない）。[`evaluate`] は
//! 経路共通の入口 [`gate::health_gate`] を呼び、扱い（[`HealthGateDecision`]）を列の 1 行（`pass`・`reason`）に写す。
//! 写しの規則はここだけに置く:
//!
//! - Pass・PolicyOnly: 合格（PolicyOnly は WU・段の統合では先へ進める。D3.1）。
//! - Repair: 不合格。reason は rule ごとの `health:<rule_id>:<hash>[,…]（evidence: …）` を `; ` で並べた文で、
//!   `task_core::classify_review_failure` が `RepairClass::Health` に分ける（D3.2）。class の数え方は
//!   [`repair_buckets`]（`health:<rule_id>`）。
//! - Error（Unavailable を含む）: 不合格。reason は `health_check_error: <理由>`（[`check_error`] が読む）。repair・replan に
//!   回さず、経路は task を Blocked にする（D3.4）。
//! - health だけの不合格（Repair）の往復（D4.3。[`limits::judge`]）と、修復の上限の使い切り（D4.4。WU の check は WU の
//!   retry の上限〈`execution_scheduler::retries_remain`〉、段の統合は統合の修復の上限）: WU の retry・統合の repair・
//!   replan に回さず、`health_findings_repeated:`・`health_repair_limit:` の質問で Blocked にする
//!   （[`stops_without_replan`]）。往復は木の root に `HealthFindingsRepeated` を追記する（受信箱の `health_repeat` 1 件）。
//!   往復の判定・root への追記・質問の文は [`health_repeat_stop`]、上限の使い切りの質問の文は [`limit_question`]
//!   の 1 か所で作り、task の review（`review_limits.rs`）も同じ関数を呼ぶ。
//! - NotApplicable: 列に足さない（event には理由を持つ証跡を残す）。
//!
//! 修復の後の再評価は、経路が新しい head で [`run_item`] を呼び直す（前の証跡を使わない。ここは何も覚えない）。
//!
//! この file は `dispatcher::phase_integration` の子 module として `#[path]` で読む（dispatcher の size の制約と、
//! `dispatcher` → `health_review` の依存を足さないため。`design_gate.rs` と同じ）。LLM は呼ばない。

use std::path::Path;

use super::super::*;
use super::ObservedIntegration;
use crate::health_review::gate::target_base;
use crate::health_review::gate::{
    self, BundledChecker, HealthGateDecision, HealthGateOutcome, HealthGateRequest,
};
use crate::health_review::input::HEALTH_INPUT_TIMEOUT;
use crate::health_review::limits::{self, HealthScope, RepeatVerdict};
use task_core::required_checks::health::{HEALTH_CHECK_ID, HealthEvidence, UnavailableReason};
use task_core::{HealthRepeatSite, RepoId};

/// D3.4: Error・Unavailable の項目の reason の先頭の語（`health_check_error: <理由>`）。
pub(in super::super) const HEALTH_CHECK_ERROR: &str = "health_check_error";
/// D4.3: 同じ指摘の往復で止めた質問の先頭の語（`health_findings_repeated: <位置>: …`）。
pub(in super::super) const HEALTH_FINDINGS_REPEATED: &str = "health_findings_repeated";
/// D4.4: health の修復の上限を使い切って止めた質問の先頭の語（`health_repair_limit: <位置>: …`。段の統合と task の
/// review の両方。[`repair_limit_question`]）。
pub(in super::super) const HEALTH_REPAIR_LIMIT: &str = "health_repair_limit";

/// 統合を止める理由 `why` が replan で直らない health の止まり（D3.4 の Error・D4.3 の往復・D4.4 の上限の使い切り）か。
/// 当たれば `integration_gives_up_with` は replan せず Blocked（質問）にする（D4.4: health のために replan を足さない）。
pub(super) fn stops_without_replan(why: &str) -> bool {
    [
        HEALTH_CHECK_ERROR,
        HEALTH_FINDINGS_REPEATED,
        HEALTH_REPAIR_LIMIT,
    ]
    .iter()
    .any(|p| {
        why.strip_prefix(p)
            .is_some_and(|rest| rest.starts_with(':'))
    })
}

/// D4.3: 往復で止めた task の質問の文。木の root の受信箱の `health_repeat`（集合ごとに 1 件）とは別に、止めた task の
/// Blocked を人が comment・回答で再開する口になる。
fn repeated_question(site: &HealthRepeatSite, fingerprints: &[String]) -> String {
    format!(
        "{HEALTH_FINDINGS_REPEATED}: {}: 修復の後も同じ health の指摘が残りました（{}）。repair・retry・replan をせずに止めました（木の root の受信箱に 1 件上げています）。直し方を comment で書くと再開します。",
        site.label(),
        fingerprints.join(", ")
    )
}

/// D4.3: 範囲 `site` の今の health の評価が同じ指摘の往復（[`limits::judge`]。task の events を読む）なら、木の root に
/// `HealthFindingsRepeated` を追記し（`task_ops::inbox::health_repeat::record_findings_repeated`。集合ごとに受信箱 1 件）、
/// 止める質問の文を返す。往復でなければ `None`（今の repair・retry の判定のまま）。WU の check・段の統合・task の review
/// の 3 経路がこれだけを呼ぶ（止め方〈質問を run の結果にする・統合を止める・review を Blocked にする〉は経路が決める）。
pub(in crate::dispatcher) fn health_repeat_stop(
    store: &dyn task_core::TaskStore,
    task: &Task,
    site: HealthRepeatSite,
) -> Result<Option<String>, DispatchError> {
    let events = store.events_for(task.id)?;
    let scope = match &site {
        HealthRepeatSite::WorkUnit { key } => HealthScope::WorkUnit(key),
        HealthRepeatSite::Phase { key } => HealthScope::Phase(key),
        HealthRepeatSite::Review => HealthScope::Review,
    };
    let RepeatVerdict::Repeat { fingerprints } = limits::judge(&events, scope) else {
        return Ok(None);
    };
    let text = repeated_question(&site, &fingerprints);
    tracing::warn!(task_id = %task.id, site = %site.label(), ?fingerprints, "the same health findings came back after a repair; blocking instead of repairing again (ADR 2026-11-08-health-enforcement-loop D4.3)");
    task_ops::inbox::health_repeat::record_findings_repeated(store, task, site, &fingerprints)
        .map_err(ops_to_store)?;
    Ok(Some(text))
}

/// D4.4: 範囲 `site` の health の修復が上限を使い切って止めた質問の文（WU の check・段の統合・task の review で共通）。
/// `used` は使い切った上限の数え（[`repair_limit_question`]・[`retry_limit_question`] が作る）、`summary` は不合格の理由。
fn limit_question(site: &HealthRepeatSite, used: &str, summary: &[String]) -> String {
    format!(
        "{HEALTH_REPAIR_LIMIT}: {}: health の修復が上限に達しました（{used}）。repair も replan もせずに止めました。直し方を comment で書くと再開します。指摘: {}",
        site.label(),
        summary.join("; ")
    )
}

/// D4.4: 段の統合と task の review の修復の上限の使い切りの質問の文（[`limit_question`]）。`bucket` は health の不合格の
/// bucket（`health:<rule_id>` を `,` で並べたもの）、`all` は（修復の合計, `max_repairs`）、`same` は（bucket の修復の数,
/// `max_repairs_per_class`）。
pub(in crate::dispatcher) fn repair_limit_question(
    site: &HealthRepeatSite,
    bucket: &str,
    (all, max_repairs): (usize, u32),
    (same, per_class): (usize, u32),
    summary: &[String],
) -> String {
    let used = format!("修復の合計 {all}/{max_repairs} 回、{bucket} の修復 {same}/{per_class} 回");
    limit_question(site, &used, summary)
}

/// D4.4: WU の check の health だけの不合格が WU の retry の上限（`max_retries`。retry の余地の判定は
/// `execution_scheduler::retries_remain`）を使い切った質問の文（[`limit_question`]）。`retries` は WU の retry の回数。
fn retry_limit_question(
    site: &HealthRepeatSite,
    retries: u32,
    max_retries: u32,
    summary: &[String],
) -> String {
    limit_question(
        site,
        &format!("WU の retry {retries}/{max_retries} 回"),
        summary,
    )
}

/// WU の check の列 `(cmd, pass)` の不合格がすべて health の項目か（不合格が無ければ `false`）。health でない失敗が混ざる
/// 不合格は往復で止めず、今どおり WU の retry にする。
fn only_health_fails<'a>(rows: impl IntoIterator<Item = (&'a str, bool)>) -> bool {
    let mut failing = rows.into_iter().filter(|(_, pass)| !pass).peekable();
    failing.peek().is_some() && failing.all(|(cmd, _)| cmd == HEALTH_CHECK_ID)
}

/// health を評価する 1 つの repo の作業ツリーと範囲（経路が組み立てる。D1.2 の表）。
#[derive(Debug, Clone)]
pub(in super::super) struct HealthTarget {
    repo_id: RepoId,
    /// git と検査器を実行する作業ツリー（WU は WU の作業ツリー、段の統合は統合後の Task の作業ツリー）。
    worktree: PathBuf,
    /// 取り込みが無いときの base（WU は `wu.base_commit`、段の統合は段の基点。無ければ統合の前の HEAD）。`Err` は読めなかった
    /// 理由（評価せず Unavailable の Error にする。合格にしない）。実際の base は [`target_base::health_base`] が head と
    /// target から決める。
    start: Result<String, String>,
    /// review 前の同期と同じ target（[`Dispatcher::health_review_target`]）。`Ok(None)` は案件の repo の参照が無い作業場所。
    target: Result<Option<String>, String>,
    /// WU は WU の HEAD、段の統合は統合の HEAD（証跡には解決した sha が残る）。
    head: String,
    /// release に同梱した検査器（経路の外側で読んだ値）。
    bundled: Option<BundledChecker>,
}

impl HealthTarget {
    /// 段の統合の対象の head を統合の HEAD にする（merge の後に決まる。空なら `HEAD`）。
    pub(super) fn at_head(self, head: &str) -> Self {
        let head = if head.is_empty() { "HEAD" } else { head };
        Self {
            head: head.to_string(),
            ..self
        }
    }
}

/// 検査の列に足す health の 1 行と、event に残す証跡。
#[derive(Debug, Clone, PartialEq, Eq)]
struct HealthItem {
    /// 列に足すか（NotApplicable は `false`。event には `evidence` を残す）。
    counted: bool,
    pass: bool,
    /// Pass は `Some(0)`、Repair は `Some(1)`、Error と NotApplicable は `None`。
    exit: Option<i32>,
    reason: String,
    evidence: HealthEvidence,
}

/// D1.2: `target` を経路共通の入口で評価し、列の 1 行に写す（blocking。git と検査器の子 process を起こす）。base は
/// [`target_base::health_base`] が決める。merge が衝突したときは base = T の 2 回目も評価し、悪い方を項目にする
/// （D3.2。Error > Fail > PolicyOnly > Pass、同じなら 1 回目）。
fn evaluate(target: &HealthTarget) -> HealthItem {
    let resolved = target
        .start
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|start| {
            target_base::health_base(&target.worktree, &target.head, start, &target.target)
        });
    let base = match resolved {
        Ok(base) => base,
        Err(why) => {
            return item_of(HealthGateOutcome::unavailable(
                target.repo_id,
                "",
                &target.head,
                UnavailableReason::FactsUnreadable(why),
            ));
        }
    };
    let first = evaluate_at(target, &base.sha);
    let Some(t) = base.against_target.filter(|_| first.counted) else {
        return first;
    };
    let second = evaluate_at(target, &t);
    if badness(&second) > badness(&first) {
        second
    } else {
        first
    }
}

fn evaluate_at(target: &HealthTarget, base: &str) -> HealthItem {
    item_of(gate::health_gate(&HealthGateRequest {
        repo_id: target.repo_id,
        worktree: &target.worktree,
        base,
        head: &target.head,
        integrated_by_celeris: true,
        bundled: target.bundled.as_ref(),
        timeout: HEALTH_INPUT_TIMEOUT,
    }))
}

/// 2 回の評価の悪さの順（D3.2）: Error 3、Repair 2、PolicyOnly 1、Pass・NotApplicable 0。
fn badness(item: &HealthItem) -> u8 {
    match (item.pass, item.exit) {
        (false, None) => 3,
        (false, Some(_)) => 2,
        (true, _) if item.reason != "health: pass" && item.counted => 1,
        _ => 0,
    }
}

fn item_of(outcome: HealthGateOutcome) -> HealthItem {
    let evidence = outcome.evidence().clone();
    let why = evidence.reason.clone().unwrap_or_default();
    let (counted, pass, exit, reason) = match outcome.decision() {
        HealthGateDecision::NotApplicable => {
            (false, true, None, format!("health: not applicable: {why}"))
        }
        HealthGateDecision::Pass => (true, true, Some(0), "health: pass".to_string()),
        HealthGateDecision::PolicyOnly => {
            let why = if why.is_empty() { "policy_only" } else { &why };
            (true, true, Some(0), format!("health: {why}"))
        }
        HealthGateDecision::Repair(repairs) => {
            let reasons: Vec<&str> = repairs.iter().map(|r| r.reason.as_str()).collect();
            (true, false, Some(1), reasons.join("; "))
        }
        HealthGateDecision::Error { reason } => {
            (true, false, None, format!("{HEALTH_CHECK_ERROR}: {reason}"))
        }
    };
    HealthItem {
        counted,
        pass,
        exit,
        reason,
        evidence,
    }
}

/// 列の 1 行が health の Error（D3.4）なら、その reason（`health_check_error: <理由>`）。
fn check_error<'a>(cmd: &str, pass: bool, reason: &'a str) -> Option<&'a str> {
    let rest = reason.strip_prefix(HEALTH_CHECK_ERROR)?;
    (cmd == HEALTH_CHECK_ID && !pass && rest.starts_with(':')).then_some(reason)
}

/// 段の統合の記録の行 `(cmd, pass, reason)` に health の Error（D3.4）があれば、その reason。
pub(super) fn health_error(rows: &[(String, bool, String)]) -> Option<String> {
    rows.iter()
        .find_map(|(cmd, pass, reason)| check_error(cmd, *pass, reason))
        .map(str::to_string)
}

/// D3.2・D4.1: health の Repair の reason（[`evaluate`] が作る）から、`max_repairs_per_class` を数える rule ごとの
/// bucket（`health:<rule_id>`、重複なし・順は reason のまま）。health の Repair でなければ空。
fn repair_buckets(reason: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in reason.split("; ") {
        let Some(item) = part.strip_prefix(task_core::execution::HEALTH_REPAIR_REASON_PREFIX)
        else {
            continue;
        };
        let first = item.split([',', '（']).next().unwrap_or_default();
        if let Some((rule_id, _hash)) = first.rsplit_once(':') {
            let bucket = format!("health:{rule_id}");
            if !out.contains(&bucket) {
                out.push(bucket);
            }
        }
    }
    out
}

/// 段の統合と task の review の修復の bucket（repair WU の title の `repair (<bucket>)`）と、その bucket の既にある修復の数。
/// health の修復は rule ごと（`health:<rule_id>` を `,` で並べる）に数え、数は rule ごとの最大（D3.2・D4.1）。他の class は
/// class の bucket。
pub(in crate::dispatcher) fn repair_count(
    class: task_core::RepairClass,
    summary: &[String],
    repairs: &[&task_core::WorkUnitRow],
) -> (String, usize) {
    let buckets: Vec<String> = match class {
        task_core::RepairClass::Health => summary.iter().flat_map(|r| repair_buckets(r)).collect(),
        other => vec![other.bucket().to_string()],
    };
    let count = |bucket: &String| {
        let title_has = |t: &str| t.split(',').any(|b| b == bucket);
        repairs
            .iter()
            .filter(|u| Dispatcher::repair_bucket_of_title(&u.spec.title).is_some_and(title_has))
            .count()
    };
    let most = buckets.iter().map(count).max().unwrap_or(0);
    (buckets.join(","), most)
}

/// 検査の列の 1 項目の位置（0 始まり）・列の長さ・cmd。
pub(super) struct ItemAt<'a> {
    pub(super) index: u32,
    pub(super) total: u32,
    pub(super) cmd: &'a str,
}

/// 検査の列の 1 項目の結果（終了の event に残す）。`health` は health の項目の証跡（他の項目は `None`）。
pub(super) struct ItemEnd {
    pub(super) pass: bool,
    pub(super) exit: Option<i32>,
    pub(super) timed_out: bool,
    pub(super) duration_ms: u64,
    pub(super) health: Option<Box<HealthEvidence>>,
}

/// 項目の開始を event に残す（葉の WU の check〈`observed.run_id` が `Some`〉は `WorkUnitCheckStarted`、段の統合は
/// `IntegrationCheckStarted`）。書けなくても検査は続ける（見るための記録で、判定には使わない）。
pub(super) fn record_start(
    store: &dyn task_core::TaskStore,
    task_id: TaskId,
    observed: &ObservedIntegration,
    at: &ItemAt<'_>,
    log_path: String,
) {
    let (work_unit_id, key) = (observed.work_unit_id.clone(), observed.key.clone());
    let (index, total, cmd) = (at.index, at.total, at.cmd.to_string());
    let started_at = rfc3339(OffsetDateTime::now_utc());
    let started = match &observed.run_id {
        Some(run_id) => Event::WorkUnitCheckStarted {
            work_unit_id,
            key,
            run_id: run_id.clone(),
            index,
            total,
            cmd,
            log_path,
            started_at,
        },
        None => Event::IntegrationCheckStarted {
            work_unit_id,
            key,
            index,
            total,
            cmd,
            log_path,
            started_at,
        },
    };
    if let Err(e) = store.append_event(task_id, &started) {
        tracing::warn!(%task_id, work_unit = %observed.key, error = %e, "could not record the integration check start");
    }
}

/// 項目の終了を event に残す（[`record_start`] と同じ event の組。health の項目は `health` 欄に証跡を持つ）。
pub(super) fn record_finish(
    store: &dyn task_core::TaskStore,
    task_id: TaskId,
    observed: &ObservedIntegration,
    at: &ItemAt<'_>,
    end: ItemEnd,
) {
    let (work_unit_id, key) = (observed.work_unit_id.clone(), observed.key.clone());
    let (index, total, cmd) = (at.index, at.total, at.cmd.to_string());
    let ItemEnd {
        pass,
        exit,
        timed_out,
        duration_ms,
        health,
    } = end;
    let finished = match &observed.run_id {
        Some(run_id) => Event::WorkUnitCheckFinished {
            work_unit_id,
            key,
            run_id: run_id.clone(),
            index,
            total,
            cmd,
            pass,
            exit,
            timed_out,
            duration_ms,
            health,
        },
        None => Event::IntegrationCheckFinished {
            work_unit_id,
            key,
            index,
            total,
            cmd,
            pass,
            exit,
            timed_out,
            duration_ms,
            health,
        },
    };
    if let Err(e) = store.append_event(task_id, &finished) {
        tracing::warn!(%task_id, work_unit = %observed.key, error = %e, "could not record the integration check finish");
    }
}

/// health の項目を 1 回評価し、開始・終了の event（`health` 欄に証跡）を残す。列に足す行（`(pass, reason)`）を返す
/// （対象が無い・NotApplicable なら `None`）。評価の thread が落ちたら合格にせず Error にする（D3.4）。
pub(super) async fn run_item(
    store: &dyn task_core::TaskStore,
    task_id: TaskId,
    observed: &ObservedIntegration,
    target: Option<HealthTarget>,
    (index, total): (u32, u32),
) -> Option<(bool, String)> {
    let target = target?;
    let at = ItemAt {
        index,
        total,
        cmd: HEALTH_CHECK_ID,
    };
    record_start(store, task_id, observed, &at, String::new());
    let clock = std::time::Instant::now();
    let item = tokio::task::spawn_blocking(move || evaluate(&target))
        .await
        .map_err(|e| format!("{HEALTH_CHECK_ERROR}: health evaluation task failed: {e}"));
    let end = ItemEnd {
        pass: item.as_ref().is_ok_and(|i| i.pass),
        exit: item.as_ref().ok().and_then(|i| i.exit),
        timed_out: false,
        duration_ms: u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX),
        health: item.as_ref().ok().map(|i| Box::new(i.evidence.clone())),
    };
    record_finish(store, task_id, observed, &at, end);
    match item {
        Ok(item) => item.counted.then_some((item.pass, item.reason)),
        Err(reason) => Some((false, reason)),
    }
}

/// 段の統合の検査の結果 `results`（`run_integration_checks` の戻り。`checks` の後に health の行が 1 行あり得る）を、
/// 統合の記録の行 `(cmd, pass, reason)` として `rows` に入れる。検査が無ければ auto_resolve の行を残して health の行だけ足す。
pub(super) fn put_rows(
    rows: &mut Vec<(String, bool, String)>,
    checks: &[task_core::WorkUnitCheck],
    results: Vec<(bool, String)>,
) {
    if !checks.is_empty() {
        rows.clear();
    }
    let cmds = checks
        .iter()
        .map(|c| c.cmd.as_str())
        .chain([HEALTH_CHECK_ID]);
    rows.extend(
        cmds.zip(results)
            .map(|(c, (pass, reason))| (c.to_string(), pass, reason)),
    );
}

/// ADR 2026-11-08-health-enforcement-loop D1.2: 段 `phase` の基点 P。その段の葉・子の unit のうち最も前（`seq`）の
/// unit が切られた commit（`base_commit`）。段の unit は段の始まりの Task のブランチから切られるので、統合と修復を
/// 繰り返しても同じ基点になる。どの unit も基点を持たなければ `None`。health の base は [`target_base::health_base`] が決める。
fn phase_base_commit(units: &[task_core::WorkUnitRow], phase: &str) -> Option<String> {
    units
        .iter()
        .filter(|u| {
            u.phase.as_deref() == Some(phase)
                && !matches!(
                    u.kind,
                    task_core::WorkUnitKind::Integrate | task_core::WorkUnitKind::Repair
                )
        })
        .filter_map(|u| u.base_commit.clone().map(|base| (u.seq, base)))
        .min_by_key(|(seq, _)| *seq)
        .map(|(_, base)| base)
}

/// health の証跡の repo の id。案件の repo の参照（`task.repos` の同じ名前）があればその id。参照の無い作業場所
/// （task の `repos` が空の単一の作業場所）は固定の id（全桁 0）にする（作業場所は 1 つなので取り違えない）。
fn health_repo_id(repo_id: Option<RepoId>) -> RepoId {
    repo_id.unwrap_or(RepoId(ulid::Ulid::nil()))
}

/// D1.2: 段の統合の health の対象。統合後の作業ツリー（検査と同じ先頭の git の repo `repo_items[0]`）で、取り込みが無い
/// ときの base `start`（段の基点〈[`phase_base_commit`]〉。無ければ merge の前の HEAD）と review target `target`
/// （[`Dispatcher::health_review_target`]）を merge の前に読む。実際の base は head が決まった後（[`HealthTarget::at_head`]
/// の統合の HEAD）に [`target_base::health_base`] が決める。git の repo が無ければ `None`。
fn phase_target<T>(
    start: Option<String>,
    repo_items: &[(PathBuf, Option<RepoId>, T)],
    target: Result<Option<String>, String>,
) -> Option<HealthTarget> {
    let (dir, repo_id, _) = repo_items.first()?;
    let start = match start {
        Some(start) => Ok(start),
        None => head_commit(dir),
    };
    Some(HealthTarget {
        repo_id: health_repo_id(*repo_id),
        worktree: dir.clone(),
        start,
        target,
        head: "HEAD".to_string(),
        bundled: gate::bundled_checker().cloned(),
    })
}

/// `dir` の HEAD の commit の sha。読めなければ理由（評価せず Unavailable にする）。
fn head_commit(dir: &Path) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .output()
        .map_err(|e| format!("統合の前の HEAD を読めない: {e}"))?;
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !sha.is_empty() {
        Ok(sha)
    } else {
        Err("統合の前の HEAD を読めない".to_string())
    }
}

impl Dispatcher {
    /// D1.2: WU の check の health の対象（取り込みが無いときの base は `wu.base_commit`、target は [`Self::health_review_target`]、
    /// head は WU の作業ツリー `worktree` の HEAD）。基点の無い WU（Task の作業ツリーで走る repair など）は `None`（段の統合で評価する）。repo の id は作業場所の先頭の git の
    /// repo の参照。
    pub(in super::super) fn work_unit_health(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
        worktree: &Path,
    ) -> Option<HealthTarget> {
        let base = wu.base_commit.clone()?;
        let ws = self.task_workspaces_for(task);
        let repo_id = ws.as_ref().and_then(|ws| {
            let repo = ws.repos.iter().find(|r| r.is_git())?;
            let reference = task.repos.iter().find(|r| r.name == repo.name)?;
            Some(reference.repo_id)
        });
        let target = match &ws {
            Some(ws) => self.health_review_target(task, ws),
            None => Ok(None),
        };
        Some(HealthTarget {
            repo_id: health_repo_id(repo_id),
            worktree: worktree.to_path_buf(),
            start: Ok(base),
            target,
            head: "HEAD".to_string(),
            bundled: gate::bundled_checker().cloned(),
        })
    }

    /// D1.2: WU の check と段の統合の health の review target。作業場所 `ws` の先頭の git の repo の、review
    /// 前の同期と同じ target（[`Dispatcher::review_target_ref`]: 木の子は親の branch、ほかは repo の既定 branch）。案件の
    /// repo の参照が無い作業場所（review 前の同期の対象でない）は `Ok(None)`。参照があるのに target を解決できない・
    /// 読めないときは `Err`（health を合格にしない）。
    fn health_review_target(
        &self,
        task: &Task,
        ws: &task_worker::TaskWorkspaces,
    ) -> Result<Option<String>, String> {
        let Some(repo) = ws.repos.iter().find(|r| r.is_git()) else {
            return Ok(None);
        };
        let Some(reference) = task.repos.iter().find(|r| r.name == repo.name) else {
            return Ok(None);
        };
        match self.review_target_ref(task, reference.repo_id, &repo.source) {
            Ok(Some(target)) => Ok(Some(target)),
            Ok(None) => Err(format!(
                "repo {} の review target を解決できない",
                repo.name
            )),
            Err(e) => Err(format!(
                "repo {} の review target を読めない: {e}",
                repo.name
            )),
        }
    }

    /// D1.2: 段 `phase` の統合の health の対象（[`phase_target`]）。統合の作業場所 `ws`・merge の前の `repo_items` で、段の
    /// 基点（[`phase_base_commit`]）と review target（[`Self::health_review_target`]）を merge の前に読む。
    pub(super) fn phase_health<T>(
        &self,
        task: &Task,
        ws: &task_worker::TaskWorkspaces,
        units: &[task_core::WorkUnitRow],
        phase: &str,
        repo_items: &[(PathBuf, Option<RepoId>, T)],
    ) -> Option<HealthTarget> {
        let target = self.health_review_target(task, ws);
        phase_target(phase_base_commit(units, phase), repo_items, target)
    }

    /// WU の check の結果の health の行を扱う。列の最後の余りの行（`run_integration_checks` が `checks` の後に足す）に
    /// health の項目の check を付ける。その行が Error（D3.4）か、不合格が health だけで WU `wu` の範囲の同じ指摘の往復
    /// （D4.3。木の root に追記する）か、不合格が health だけで WU の retry の上限を使い切った（D4.4。
    /// `execution_scheduler::retries_remain` が偽。上限は `execution_scheduler::decide` に渡る `task.budget.max_retries`）
    /// なら、run の結果を質問（`health_check_error: <理由>`・`health_findings_repeated: …`・`health_repair_limit: …`。
    /// worker が `Done` で返した usage は落とさない）にすり替え、検査の記録を空にする（WU の repair〈retry〉にも、retry の
    /// 使い切りの後の WU の Failed〈replan〉にもしない）。health でない失敗が混ざる不合格は今どおり retry・replan に進む。
    pub(in super::super) fn settle_health_row(
        &self,
        task: &Task,
        wu: &Option<task_core::WorkUnitRow>,
        result: Result<RunOutcome, AdapterError>,
        mut check_run: WorkUnitCheckRun,
    ) -> Result<(Result<RunOutcome, AdapterError>, WorkUnitCheckRun), DispatchError> {
        if check_run.results.len() > check_run.checks.len() {
            check_run.checks.push(task_core::WorkUnitCheck {
                cmd: HEALTH_CHECK_ID.to_string(),
                expect_exit: 0,
                scope: false,
            });
        }
        let rows = || check_run.checks.iter().zip(&check_run.results);
        let error = rows().find_map(|(c, (pass, reason))| check_error(&c.cmd, *pass, reason));
        let repeat = match (error, wu) {
            (Some(text), _) => Some(text.to_string()),
            (None, Some(wu))
                if only_health_fails(rows().map(|(c, (p, _))| (c.cmd.as_str(), *p))) =>
            {
                let site = HealthRepeatSite::WorkUnit {
                    key: wu.key.clone(),
                };
                let max_retries = task.budget.max_retries;
                match health_repeat_stop(self.store.as_ref(), task, site.clone())? {
                    Some(text) => Some(text),
                    None if !crate::execution_scheduler::retries_remain(wu, max_retries) => {
                        let summary: Vec<String> = rows()
                            .filter(|(_, (pass, _))| !pass)
                            .map(|(_, (_, reason))| reason.clone())
                            .collect();
                        tracing::warn!(task_id = %task.id, work_unit = %wu.key, retries = wu.retries, max_retries, "a health-only work unit check failure used up the work unit retries; blocking instead of failing into a replan (ADR 2026-11-08-health-enforcement-loop D4.4)");
                        Some(retry_limit_question(
                            &site,
                            wu.retries,
                            max_retries,
                            &summary,
                        ))
                    }
                    None => None,
                }
            }
            (None, _) => None,
        };
        let Some(text) = repeat else {
            return Ok((result, check_run));
        };
        let usage = match &result {
            Ok(RunOutcome {
                terminal: Terminal::Done { usage, .. },
                ..
            }) => *usage,
            _ => None,
        };
        let question = Ok(RunOutcome {
            terminal: Terminal::Question { text, usage },
            exit_code: None,
        });
        let cleared = WorkUnitCheckRun {
            checks: Vec::new(),
            results: Vec::new(),
            cwd: check_run.cwd,
        };
        Ok((question, cleared))
    }

    /// D4.3・D4.4: 段 `phase` の統合の health だけの不合格（`class` が Health）を repair の前に止めるか。同じ指摘の往復なら
    /// 木の root に追記して往復の質問を、修復の上限を使い切った（`used` が `Some((bucket, 統合の修復の数, bucket の修復の
    /// 数))`）なら上限の質問を返す。どちらも [`stops_without_replan`] に当たり、統合は repair も replan もせず Blocked で
    /// 止まる。health でない不合格と、上限の内の往復でない指摘は `None`（今の repair・replan の判定のまま）。
    pub(super) fn phase_health_stop(
        &self,
        task: &Task,
        phase: &str,
        class: task_core::RepairClass,
        used: Option<(&str, usize, usize)>,
        summary: &[String],
    ) -> Result<Option<String>, DispatchError> {
        if class != task_core::RepairClass::Health {
            return Ok(None);
        }
        let site = HealthRepeatSite::Phase {
            key: phase.to_string(),
        };
        if let Some(text) = health_repeat_stop(self.store.as_ref(), task, site.clone())? {
            return Ok(Some(text));
        }
        let limit = &self.config.execution;
        Ok(used.map(|(bucket, all, same)| {
            let (all, same) = (
                (all, limit.max_repairs),
                (same, limit.max_repairs_per_class),
            );
            repair_limit_question(&site, bucket, all, same, summary)
        }))
    }
}

#[cfg(test)]
#[path = "check_item_tests.rs"]
mod tests;
