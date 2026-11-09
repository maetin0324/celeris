---
title: 健全性v2 G3 loop-points entry — health の経路共通の入口 health_gate と結果の分類（D1.3・D3.1・D3.2）
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 loop-points entry — health の経路共通の入口 health_gate と結果の分類（D1.3・D3.1・D3.2）

完了日: 2026-11-09

WorkUnit `entry`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1、unit loop-points の子）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D1.2・D1.3・D3.1・D3.2 のうち、全経路が呼ぶ 1 つの入口と結果の分類を `crates/task-dispatch/src/health_review/gate.rs` に置いた。経路への配線（review・WU の check・段の統合・再利用・同期・修復の後・delivery）は後続の unit（integration-path・review-path・reuse-sync・delivery-path）が行う。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 入口の名前・型・使い方（後段の unit 向け）
module は `task_dispatch::health_review::gate`（`pub mod gate`）。経路はこの入口だけを呼び、検査器の起動を自分で書かない。

```rust
pub struct HealthGateRequest<'a> {
    pub repo_id: RepoId,
    pub worktree: &'a Path,            // git を実行する作業ツリー
    pub base: &'a str,                 // sha か ref（証跡には解決した sha）
    pub head: &'a str,                 // sha か ref
    pub integrated_by_celeris: bool,   // ssh の remote workspace は false
    pub bundled: Option<&'a BundledChecker>, // 同梱の検査器（経路の外側から引数で渡す）
    pub timeout: Duration,             // 事実の読み取り＋評価の全体。経路は HEALTH_INPUT_TIMEOUT を渡す（D4.2）
}

pub fn health_gate(request: &HealthGateRequest<'_>) -> HealthGateOutcome;

pub enum HealthGateOutcome {
    NotApplicable { reason: NotApplicableReason, evidence: HealthEvidence }, // verdict NotApplicable
    Unavailable { reason: UnavailableReason, evidence: HealthEvidence },     // verdict Error
    Evaluated(HealthRun),                                                    // Pass・PolicyOnly・Fail・Error
}
pub struct HealthRun { pub evidence: HealthEvidence, pub failures: Vec<HealthFailure> }
pub struct HealthFailure { pub rule_id: String, pub fingerprint: String, pub paths: Vec<String> }

impl HealthGateOutcome {
    pub fn evidence(&self) -> &HealthEvidence;          // どの場合も証跡がある
    pub fn item(&self) -> Option<&HealthEvidence>;      // 必須検査の列に足す項目（NotApplicable は None）
    pub fn decision(&self) -> HealthGateDecision;       // 経路の扱い
}

pub enum HealthGateDecision {
    NotApplicable,              // 項目を足さない
    Pass,                       // 合格
    PolicyOnly,                 // WU・段の統合は合格扱い、review は G2 の policy_review_gate
    Repair(Vec<HealthRepair>),  // 統合を止めて既存の repair へ（rule ごと、rule_id の順）
    Error { reason: String },   // 合格にも repair にもしない（D3.4 の Blocked と受信箱）
}

pub struct HealthRepair { pub rule_id: String, pub reason: String, pub paths: Vec<String> }
impl HealthRepair { pub fn bucket(&self) -> String } // "health:<rule_id>"
pub fn health_repairs(failures: &[HealthFailure]) -> Vec<HealthRepair>;
```

- 使い方: 経路は `HealthGateRequest` を作って `health_gate` を 1 回呼び、`item()` を必須検査の列（`PhaseCheckResult.health`・`WorkUnitCheckFinished.health`・`IntegrationCheckFinished.health`。cmd は `HEALTH_CHECK_ID`）に足し、`decision()` で進むか止めるかを決める。項目の `pass` は `decision() == Pass`（WU・段の統合では `PolicyOnly` も）だけ。
- `HealthRepair.reason` は `health:<fingerprint>[,…]（evidence: <path>, …）`。report/1 の schema で fingerprint は `<rule_id>:<hash>` なので、項目は `health:<rule_id>:<hash>`。review は `FailedCheck { check: Command { cmd: HEALTH_CHECK_ID, .. }, reason, repair_hint }` の reason と、`repair_hint` の class に `bucket()` を使う。WU・段の統合は既存の repair の失敗の詳細に reason を、class の数え方に `bucket()` を使う。
- 同梱の検査器: 経路の中で `bundled_checker()`（process の OnceLock）を読まない。daemon・delivery の外側で 1 回読み、`HealthGateRequest.bundled` に渡す（`crates/celeris/src/delivery.rs` の `tick` と同じ形）。
- 内部の順: `repo_facts`（git は `git_stdout(worktree, …, deadline)`）→ `applicability` → `run_health_check`（private。経路は直接呼べない）。

## 変更
- 件名「G3 loop-points entry: 経路共通の入口 health_gate と結果の分類・Fail の repair の材料」の commit
  - `crates/task-dispatch/src/health_review/gate.rs`
    - 上の型と `health_gate`・`health_repairs` を足した。
    - `run_health_check` を private にし、引数 `bundled: Option<&BundledChecker>` を足した。同梱の検査器は引数だけから取り、`bundled_checker()` を読まない（渡されなければ `checker_missing`、版が違えば `checker_mismatch` の Error）。戻り値は `HealthRun`（証跡と Fail の finding）。
    - Fail の finding: 同じ検査器の `is_fail_grade` が選んだ fingerprint（`evidence.blocking`、50 件まで）を report の `findings` から引き、rule_id と evidence の path を持たせた。report に無い fingerprint と、fail 級の finding の無い exit 1 は `report_invalid` の Error（修復の対象が無いので repair に回さない。fail closed）。
    - PolicyOnly の判定は既存の `run_health_check` の判定（同じ検査器の `health.engine.policy_review_files`。`check-health.sh` と同じ）をそのまま使い、写しを作っていない。
  - `crates/task-dispatch/src/health_review/gate_tests.rs`: `run_health_check` の新しい引数に合わせた（同梱の試験は `Some(configured)` を渡す）。子 module `entry`（`entry_tests.rs`）を `#[path]` で読む。
  - `crates/task-dispatch/src/health_review/entry_tests.rs`（新規）: 試験 `health_enforce_entry_` 8 件。
- 件名「G3 loop-points entry: 進捗」の commit: この file。

## 試験（守る不変条件と、既存の試験で検出できない理由）
既存の `health_evidence_gate_*` は `run_health_check` の証跡の値を見るだけで、経路が見る分類と扱いを通らない。下の試験は入口を通し、分類の取り違え（Error・Unavailable を合格や repair にする、NotApplicable で項目を足す）を検出する。
- `health_enforce_entry_pass`: base の検査器の exit 0 は Evaluated・Pass、項目を足す。
- `health_enforce_entry_policy_only`: policy の file だけの変更は PolicyOnly で、Pass でも repair でもない。
- `health_enforce_entry_fail_routes_to_repair`: fail 級（error・new か worsened・例外なし）だけが rule ごとの repair になり、reason `health:<rule_id>:<hash>,…（evidence: …）` と bucket `health:<rule_id>` を持つ。warning・existing・例外つきは入らない。
- `health_enforce_entry_repairs_group_by_rule`: `health_repairs` が入力の順に依らず rule_id・fingerprint の順にまとめ、path を重複なく並べる。
- `health_enforce_entry_error_is_not_pass`: exit 2（error・unknown・unsupported）・report 無し・壊れた report・時間切れ・fail 級の無い exit 1 は Error で、合格にも repair にもならず、項目は verdict Error。
- `health_enforce_entry_not_applicable`: remote workspace（作業ツリーを読まない）と Git 管理でない dir は NotApplicable で、項目を足さず理由を証跡に残す。
- `health_enforce_entry_unavailable_is_not_pass`: base にも同梱にも検査器が無い repo（`checker_missing`）と、base を読めない repo（`facts_unreadable:`）は Unavailable で、項目は verdict Error、扱いは Error。
- `health_enforce_entry_non_celeris_repo_uses_bundled_checker`: base に検査器が無く test 台本が check-health を呼ばない repo では、引数で渡した同梱の検査器を選び、同じ判定（fail 級 → repair）をする。渡さなければ Unavailable。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch --lib health_review::gate` → exit 0（17 passed: `health_enforce_entry_` 8 件、`health_evidence_gate_` 9 件）
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。3615 passed、11 skipped。check-health は verdict=pass・fail_grade=0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更 path は `crates/task-dispatch/src/health_review/{gate,gate_tests,entry_tests}.rs` とこの file だけ

## 未解決
- 試験名の prefix: ADR の「後続 unit の path・試験名の prefix」は loop-points を `health_gate_path_` と書くが、task の計画は `health_enforce_`（この unit は `health_enforce_entry_`）を指定した。計画に従った。ADR の付記で揃えるのは unit record か closeout。
- `RepairClass` は `Copy` で `bucket()` が `&'static str` を返すので、`health:<rule_id>` を持つ variant はまだ無い。この unit は bucket の文字列（`HealthRepair::bucket`）だけを作った。`RepairClass` に足すか `ReviewRepairHint.class` で運ぶかは review-path・integration-path が決める（`crates/task-core/src/execution.rs` はこの unit の範囲の外）。
- 1 回の Fail に複数の rule があると `HealthRepair` は rule ごとに複数になる。今の `classify_review_failure` は class の混在を Substantive にするので、review-path は 1 件の `FailedCheck` にまとめるか rule ごとに分けるかを決める必要がある。
- 並行する unit facts は `repo_facts` の読み取りの失敗を Unavailable にする。入口は `repo_facts` を呼ぶだけなので、signature が変わらなければ入口の変更は要らない。

## 提案
- 無し。
