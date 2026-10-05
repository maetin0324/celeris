# Phase 3 (p3-model-fix): model 段統合の修正 — feedback 試験の RoutingRecord literal

完了日: 2026-10-05（状態: 修正は integrate-model 段で適用する。この branch 単独では未適用）

## 原因

- escalation 葉（`f353b14f`、branch `celeris-wu/01M4577C9412HCDQEV1AFTT69C/escalation`）が `task_core::model_policy::RoutingRecord` に `escalation: Option<crate::retry_policy::EscalationAudit>` を足した。
- events 葉（`55d9988b`）が書いた `crates/task-core/src/model_router/feedback/tests.rs:15` の全欄 struct literal には、その欄が無い。
- 2 葉を統合すると `E0063 missing field 'escalation' in initializer of 'RoutingRecord'` で task-core の lib test が compile できず、全 check と test-parallel が連鎖で落ちる。

## 修正

- `crates/task-core/src/model_router/feedback/tests.rs` の `RoutingRecord { ... }` literal で `optimizer: None,` の次に `escalation: None,` を 1 行足す。
- 他のファイルは変えない。

## この branch で足さなかった理由

- 現 branch（`model-fix`、base `103d9f88`）は `RoutingRecord` に `escalation` 欄を持たない（`crates/task-core/src/model_policy.rs:527-548`）。
- ここで 1 行だけ足すと、この branch の task-core は E0063 とは逆の E0560（unknown field）で compile できなくなる。
- そのため修正は、escalation 葉を取り込んだ統合後の状態に対して当てる（integrate-model で適用する）。

## 証拠（統合後の状態を試行して確認し、後で戻した）

試行手順: `git merge --no-commit --no-ff celeris-wu/01M4577C9412HCDQEV1AFTT69C/escalation` → 修正前に `cargo test -p task-core --lib --no-run` → 修正を当てて検査 → `git merge --abort` と `git reset --hard HEAD`。

| 段階 | コマンド | 結果 |
| --- | --- | --- |
| 事前見積もり | `git merge-tree --write-tree HEAD <escalation>` | exit 0（衝突なし） |
| 修正前 | `cargo test -p task-core --lib --no-run` | `error[E0063]: missing field 'escalation' in initializer of 'RoutingRecord'`（`feedback/tests.rs:15`）、task-core lib test が compile できない |
| 修正後 | `cargo test -p task-core --lib feedback` | `test result: ok. 3 passed; 0 failed`、exit 0 |
| 修正後 | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0（`Finished dev profile`） |
| 後始末 | `git merge --abort && git reset --hard HEAD` | 作業ツリーは HEAD `103d9f88` と一致。`feedback/tests.rs` に `escalation` は無い |

注: `cargo test -p task-core --lib feedback` の 3 件は、feedback 系の `routing_outcome_reads_work_unit_checks_by_run`、`routing_reward_waits_for_review_and_supersedes_idempotently` などを含む。task-core 全体（1694 件の nextest）は統合後の段（integrate-model）で流す。

## 未解決事項

- integrate-model で、escalation 葉の統合後に `feedback/tests.rs` の上記 1 行を当てる。
- task-core・task-api・task-ops の nextest（1694 件 passed）は、その統合後の状態で確かめる。この branch 単独ではまだ確かめていない（この branch の HEAD は escalation 欄の無い状態で、この段の変更は進捗ファイルだけ）。

## 提案

- `RoutingRecord` の全欄 literal を grep で洗い出し（`grep -rn 'RoutingRecord {' crates`、12 箇所）、統合時に欄追加の影響をまとめて直せるようにする。
- 欄を持つ葉の範囲 check には、欄を使う他 crate の literal を含める（既存の教訓 `task-core の pub struct に欄を足す葉`）。
