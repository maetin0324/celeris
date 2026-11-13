//! ADR 2026-11-11 dispatcher-state-ownership D1: 案件の停滞の見張りと CoS chat の起動の待ちの owner。
//!
//! - 持つ状態は 2 つ: 案件単位の停滞の判定の結果（[`StagnationWatch`]。ADR 2026-10-27-project-stagnation-inbox D3・D4）と、
//!   CoS chat の起動の状態（型は `L`。daemon の dispatcher では `cos_chat::launch::CosChatLaunch`。この module は
//!   `dispatcher` module を使わないので型引数で受ける）。
//! - field は private。`tree_units`（停滞の判定）と `cos_chat/launch`（起動の状態の差し込み・外し・tick の取り出しと戻し）の
//!   書き込みはこの struct の method を通す。
//! - 停滞の判定は store と releases_dir の事実から毎回作り直す（ここは結果の写しだけで、止まり始めた時刻を持たない）。
//!   判定をするかは純粋関数 `task_ops::stagnation::scan_due`、判定そのものは `task_ops::stagnation::detect` が決め、
//!   この struct は判定の時刻・成否・番号（generation）を持つ。番号は判定が成功するたびに 1 進み、閾値を差し替えても
//!   戻さない（CoS の取り込みが番号の変化だけで照合し直すので、通知の冪等性はこの番号で保つ。
//!   ADR 2026-10-29-stagnation-triage-notify-wiring D1・D3）。
//! - 壁時計など `ProbePorts` の値は持たず、method の引数で受ける（`now`）。

use task_core::pricing::BudgetRates;
use task_ops::stagnation::{StagnationFinding, StagnationPolicy};
use time::OffsetDateTime;

/// ADR 2026-10-27-project-stagnation-inbox D3・D4: 停滞の判定の状態。閾値と間隔（`configure_stagnation` で渡す）、
/// 最後の判定（snapshot の `stagnation`）と判定した時刻。
#[derive(Debug, Clone)]
struct StagnationWatch {
    policy: StagnationPolicy,
    scan_interval_secs: u64,
    findings: Vec<StagnationFinding>,
    checked_at: Option<OffsetDateTime>,
    /// 最後の判定が成功し、`findings` が今の状態か（ADR 2026-10-29-stagnation-triage-notify-wiring D1）。
    available: bool,
    /// 判定が成功するたびに 1 進む（同 D3）。
    generation: u64,
    /// ADR 2026-11-09-unscheduled-restart-hold D1: この process で `dispatch_ready` が 1 回でも終わったか（tick が立てる）。
    waits_known: bool,
    /// ADR 2026-11-06-budget-usd-caps D2: `[budget_rates]`（単価表に無い model の換算単価）。案件の日次上限と木の
    /// 上限の判定が使う。`set_budget_rates` で入る。
    budget_rates: BudgetRates,
}

impl Default for StagnationWatch {
    fn default() -> Self {
        Self {
            policy: StagnationPolicy::default(),
            scan_interval_secs: task_ops::stagnation::DEFAULT_SCAN_INTERVAL_SECS,
            findings: Vec::new(),
            checked_at: None,
            available: false,
            generation: 0,
            waits_known: false,
            budget_rates: BudgetRates::default(),
        }
    }
}

/// 案件の停滞の見張りと CoS chat の起動の待ちの owner（`L` は CoS chat の起動の状態の型）。
pub(crate) struct ProjectOrchestration<L> {
    /// CoS chat の起動の状態（store・設定・走っている run・slot の待ち・triage・案件の controller）。
    /// `None` は CoS chat を配線していない（verify も外す）。
    cos_chat_launch: Option<L>,
    /// 案件単位の停滞の判定の結果と、判定の時刻・成否・番号。
    stagnation: StagnationWatch,
}

impl<L> Default for ProjectOrchestration<L> {
    fn default() -> Self {
        Self {
            cos_chat_launch: None,
            stagnation: StagnationWatch::default(),
        }
    }
}

impl<L> ProjectOrchestration<L> {
    // --- CoS chat の起動の状態 ---

    /// CoS chat の起動の状態（配線していなければ `None`）。
    pub(crate) fn launch(&self) -> Option<&L> {
        self.cos_chat_launch.as_ref()
    }

    /// CoS chat の起動の状態を書き換える口（配線していなければ `None`）。
    pub(crate) fn launch_mut(&mut self) -> Option<&mut L> {
        self.cos_chat_launch.as_mut()
    }

    /// 起動の状態を差し込む（前の状態は捨てる）。
    pub(crate) fn install_launch(&mut self, launch: L) {
        self.cos_chat_launch = Some(launch);
    }

    /// 起動の状態を外す（verify の起動時。reconcile・fallback・root-done の処理も止まる）。
    pub(crate) fn remove_launch(&mut self) {
        self.cos_chat_launch = None;
    }

    /// tick の間だけ起動の状態を取り出す（`restore_launch` で戻す。取り出している間は `launch` が `None`）。
    pub(crate) fn take_launch(&mut self) -> Option<L> {
        self.cos_chat_launch.take()
    }

    /// `take_launch` で取り出した状態を戻す。
    pub(crate) fn restore_launch(&mut self, launch: L) {
        self.cos_chat_launch = Some(launch);
    }

    // --- 停滞の見張り ---

    /// 停滞の閾値と判定の間隔を差し替える。次の判定から効く（間隔の待ちと前の結果は捨てる）。
    /// 番号・メモリの待ちが揃ったという事実・換算単価は保つ（取り込み側が照合済みの番号と食い違うように、番号は戻さない）。
    pub(crate) fn configure_stagnation(
        &mut self,
        policy: StagnationPolicy,
        scan_interval_secs: u64,
    ) {
        let previous = std::mem::take(&mut self.stagnation);
        self.stagnation = StagnationWatch {
            policy,
            scan_interval_secs,
            generation: previous.generation,
            waits_known: previous.waits_known,
            budget_rates: previous.budget_rates,
            ..StagnationWatch::default()
        };
    }

    /// ADR 2026-11-09-unscheduled-restart-hold D1: メモリの待ちが揃った（起動後に `dispatch_ready` が 1 回終わった）。
    pub(crate) fn mark_waits_known(&mut self) {
        self.stagnation.waits_known = true;
    }

    /// ADR 2026-11-06-budget-usd-caps D2: `[budget_rates]` の換算単価を入れる。
    pub(crate) fn set_budget_rates(&mut self, rates: BudgetRates) {
        self.stagnation.budget_rates = rates;
    }

    /// `[budget_rates]` の換算単価。
    pub(crate) fn budget_rates(&self) -> &BudgetRates {
        &self.stagnation.budget_rates
    }

    /// 最後の停滞の判定の結果（判定が失敗した後は前の結果のまま。今の状態かは `stagnation_available`）。
    pub(crate) fn stagnation_findings(&self) -> &[StagnationFinding] {
        &self.stagnation.findings
    }

    /// 停滞の閾値。
    pub(crate) fn stagnation_policy(&self) -> StagnationPolicy {
        self.stagnation.policy
    }

    /// 最後の判定が成功し、結果が今の状態として使えるか。
    pub(crate) fn stagnation_available(&self) -> bool {
        self.stagnation.available
    }

    /// 判定が成功するたびに 1 進む番号。
    pub(crate) fn stagnation_generation(&self) -> u64 {
        self.stagnation.generation
    }

    /// 停滞の判定を始めるか決め、始めるなら判定の時刻を `now` にして結果を「今の状態でない」にし、閾値を返す。
    /// 判定の時期でない・起動後にメモリの待ちが揃っていないなら `None`（状態は変えない）。判定が失敗したら
    /// `record_stagnation` を呼ばないので、前の結果は今の状態として使われない（ADR 2026-10-29-stagnation-triage-notify-wiring D2）。
    pub(crate) fn begin_stagnation_scan(
        &mut self,
        now: OffsetDateTime,
    ) -> Option<StagnationPolicy> {
        let s = &self.stagnation;
        if !task_ops::stagnation::scan_due(s.checked_at, s.scan_interval_secs, s.waits_known, now) {
            return None;
        }
        self.stagnation.checked_at = Some(now);
        self.stagnation.available = false;
        Some(self.stagnation.policy)
    }

    /// 成功した判定の結果を入れ、番号を 1 進める。
    pub(crate) fn record_stagnation(&mut self, findings: Vec<StagnationFinding>) {
        self.stagnation.findings = findings;
        self.stagnation.available = true;
        self.stagnation.generation += 1;
    }

    /// 試験: 次の判定の時期を決める時刻を置く（遠い未来にして tick の判定を止める）。
    #[cfg(test)]
    pub(crate) fn set_stagnation_checked_at_for_test(&mut self, at: OffsetDateTime) {
        self.stagnation.checked_at = Some(at);
    }

    /// 試験: 停滞の判定の結果を直に置く。
    #[cfg(test)]
    pub(crate) fn set_stagnation_findings_for_test(&mut self, findings: Vec<StagnationFinding>) {
        self.stagnation.findings = findings;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).expect("rfc3339")
    }

    /// 不変条件（ADR 2026-10-29-stagnation-triage-notify-wiring D1・D3、ADR 2026-11-09-unscheduled-restart-hold D1）:
    /// メモリの待ちが揃う前は判定しない。判定が成功するたびに番号が 1 進み、閾値を差し替えても番号・待ちの印・
    /// 換算単価を戻さない。判定を始めた後に成功しなければ結果は今の状態でない。
    /// 既存の試験で検出できない理由: dispatcher の停滞の試験は store と tick を通して findings を見るだけで、
    /// 閾値の差し替えで番号が 0 に戻っても（CoS の取り込みが照合し直さず通知を重ねても）findings は同じなので通る。
    #[test]
    fn stagnation_generation_survives_reconfigure_and_scan_waits_for_waits() {
        let mut o: ProjectOrchestration<()> = ProjectOrchestration::default();
        let t0 = at("2026-11-13T00:00:00Z");
        assert!(
            o.begin_stagnation_scan(t0).is_none(),
            "waits are not known yet"
        );
        o.mark_waits_known();
        assert!(o.begin_stagnation_scan(t0).is_some());
        assert!(!o.stagnation_available());
        o.record_stagnation(Vec::new());
        assert!(o.stagnation_available());
        assert_eq!(o.stagnation_generation(), 1);
        assert!(
            o.begin_stagnation_scan(t0).is_none(),
            "the interval has not passed"
        );
        o.set_budget_rates(BudgetRates::default());
        o.configure_stagnation(StagnationPolicy::default(), 0);
        assert_eq!(o.stagnation_generation(), 1, "generation is not reset");
        assert!(!o.stagnation_available(), "old findings are not current");
        assert!(o.begin_stagnation_scan(t0).is_some(), "waits_known is kept");
        assert!(!o.stagnation_available());
        o.record_stagnation(Vec::new());
        assert_eq!(o.stagnation_generation(), 2);
    }

    /// 不変条件: tick の間に取り出した起動の状態は戻すまで `launch` に見えず、戻すと同じ値が見える（CoS chat の起動の
    /// 待ちを tick の間に失わない）。
    /// 既存の試験で検出できない理由: CoS chat の試験は tick の前後の状態だけを見るので、取り出しと戻しが owner の
    /// method を通らなくても通る。この試験は owner の method の契約を直接固定する。
    #[test]
    fn launch_take_and_restore_keep_the_state() {
        let mut o: ProjectOrchestration<u32> = ProjectOrchestration::default();
        assert!(o.launch().is_none());
        o.install_launch(7);
        let taken = o.take_launch().expect("installed");
        assert!(o.launch().is_none());
        o.restore_launch(taken);
        assert_eq!(o.launch(), Some(&7));
        if let Some(l) = o.launch_mut() {
            *l = 8;
        }
        assert_eq!(o.launch(), Some(&8));
        o.remove_launch();
        assert!(o.launch().is_none());
    }
}

#[cfg(test)]
mod structure_guard_tests {
    /// 不変条件（ADR 2026-11-11 dispatcher-state-ownership D1・D2）: `ProjectOrchestration` は表の 2 field だけを private に持ち、
    /// `Dispatcher` を受けず `dispatcher` module を使わない。`StagnationWatch` の field も private。
    /// 既存の試験で検出できない理由: 停滞と CoS chat の試験は振る舞いだけを見るので、field を pub にしても、method が
    /// Dispatcher の可変参照を受けても、`dispatcher` の型を直に使っても通る。
    #[test]
    fn structure_guard_project_orchestration() {
        let src = include_str!("project_orchestration.rs");
        let src = &src[..src
            .find("mod tests {")
            .expect("the test module is in the source")];
        let fields_of = |head: &str| {
            let start = src.find(head).expect("the struct is defined");
            let body = &src[start..];
            let body = &body[..body.find("\n}\n").expect("the struct body ends")];
            let mut fields = Vec::new();
            for line in body.lines().skip(1) {
                let t = line.trim_start();
                if t.starts_with("///") || t.is_empty() {
                    continue;
                }
                assert!(!t.starts_with("pub"), "a field is not private: {line}");
                if let Some((name, _)) = t.split_once(": ") {
                    fields.push(name.to_string());
                }
            }
            fields
        };
        assert_eq!(
            fields_of("pub(crate) struct ProjectOrchestration<L> {"),
            ["cos_chat_launch", "stagnation"]
        );
        assert_eq!(fields_of("struct StagnationWatch {").len(), 8);
        let owner = ["Dis", "patcher"].concat();
        for bad in [
            format!("&mut {owner}"),
            format!("&{owner}"),
            format!("impl {owner}"),
            format!("crate::{}", owner.to_lowercase()),
            format!("super::{}", owner.to_lowercase()),
        ] {
            assert!(
                !src.contains(&bad),
                "project_orchestration.rs must not contain {bad:?}"
            );
        }
    }
}
