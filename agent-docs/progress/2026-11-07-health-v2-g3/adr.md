---
title: 健全性v2 G3 adr — ADR health-enforcement-loop
tasks: [01M6TYVJSWEY3ZE50S02ETYDNM]
status: done
updated: 2026-11-08
completed: 2026-11-08
---
# 健全性v2 G3 adr — ADR health-enforcement-loop

完了日: 2026-11-08

WorkUnit `adr`（G3 の ADR: 適用判定・証跡の結び付け・修復・上限・CI 入口・棚卸し・P-9・write_set）。基準は `$CELERIS_WU_BASE`。コードは変えていない。

## 変更
- 件名「G3 adr: ADR health-enforcement-loop と task・unit の進捗」の commit — [ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md)（状態: 提案）、task の進捗 [2026-11-07-health-v2-g3.md](../2026-11-07-health-v2-g3.md)、この file を追加した。

## 決めたこと（要約）
- (1) 適用判定: `task_core::required_checks::health::applicability`（純粋関数）。入力は Git 管理か・Celeris が統合する作業ツリーか・base の検査器・同梱の検査器だけ。全経路（direct の review・WU の check・段の統合・review の再利用・review 前の同期・統合の修復の後・delivery）の入口の関数を表にした。base に検査器が無い repo は Celeris の release に同梱した `<release>/repo-health/` を使う（profiles は写さず、repo に profile が無ければ組み込み generic）。remote workspace は Celeris が統合しないので `NotApplicable(remote_workspace)` にし、CI の入口を使う。
- (2) 証跡: `HealthEvidence` を既存の `PhaseCheckResult` の任意欄にし、`RequiredChecksEvaluated`・`WorkUnitCheckFinished`・`IntegrationCheckFinished` に残す。新しい Event の種類は作らない。`head_tree`・`checker.sha256`・`policy_sha256`・`report_sha256` は git と report の既存欄から Celeris 側で計算する。`definition_digest` は domain v3 で health の項目と検査器の版を含める。再利用は同じ `(repo, base_sha, head_tree, policy, checker)` の Pass だけ。
- (3) 修復: report の fail 級の finding（Fail）だけが統合を止めて既存の repair に回る。PolicyOnly は G2 の policy の関門。Error は repair に回さず Blocked と受信箱。reviewer の blocking の finding は差分の path と決定的な anchor が無ければ advisory に下げる。
- (4) 上限: 新しい欄は作らず、既存の `max_repairs`・`max_repairs_per_class`（class `health:<rule_id>`）・`max_replans`・段の統合の自動修復・`IntegrationRepair`・木と案件の費用の上限で数える。検査器の 1 回は 600 秒。同じ fingerprint の集合の往復は木の root の受信箱に 1 件。
- (5) CI: `scripts/dev/repo-health/ci.sh` は同じ検査器の薄い入口で policy の file。Celeris は CI の結果を証跡にせず、外部の手動 push は強制できない。branch 保護は repo の管理者が設定する。
- (6) 棚卸し: cron の種類 `health_inventory`。既定では job を作らない。前回の HEAD を base にした差分検査、`(head_tree, policy, checker)` が同じ repo は省略、記録は追記だけの表 `health_inventory_runs`、起票は fail 級だけを draft で冪等キー `health-debt:<repo_id>:<fingerprint>:<head_sha>`、1 回 3 件まで。
- (7) P-9（決定 p9-scope）: 案 A。`cascade_after_transition_tx` に reopen の枝を足し、`dependency_failed` で cancel され人が触っていない後続を `Trigger::DependencyReopened` で Ready に戻す。戻せない後続は受信箱で知らせる。
- (8) write_set: 13 行の path と理由・使う unit を表にした。slices.json は G3 の branch で変えない。

## 目的文との違い
- (1) の「repo に検査器が無いときに使う検査器の所在」は、同梱の検査器のために `scripts/selfdeploy/release.sh` と `crates/celeris/src/daemon/` の変更が要る。どちらも B0 の G3 の write_set の外なので (8) に足した。
- (8) の例示（`health_review/`・`review.rs`・`cron_jobs`・`docs/guides`・`docs/api/v1`・web の生成型）に加え、`required_checks`・`model/events.rs`・`cron.rs`・store・受信箱・`docs/architecture-map.md` が要ると判断して足した。

## 証拠（WU の作業ツリーで、commit の前に実行。基準は `$CELERIS_WU_BASE`）
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (411 files)`）
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）
- `python3 scripts/dev/check-human-text.py` → exit 0（0 findings）
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict=pass、doc-only、new=0・worsened=0）
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、出力は ADR と進捗の 3 file だけ。
- 受け入れ条件 0: ADR を `### (` で分けた 8 節のすべてに「- 決定」の行と「採らなかった」の行があることを python で確かめた（8 節とも True）。
- コードを変えていないので、`test-affected.sh`・clippy・fmt は close-out で task の提出時の HEAD に対して走らせる。

## 未解決
- 後続 unit は未着手。ADR の決定は実装の時に食い違いが出うる（特に D2.3 の `policy_sha256` の入力と、D6.5 の表の列）。close-out で付記に書く。
- remote workspace の review の流れ（`spawn_review` は remote workspace の task で早く返る）は、ADR を書く時点で読んだ範囲の確認だけ。remote の task が delivery に入らないことは `DeliverySkipReason` と ADR-0079 D5 の記述から判断した。loop-points で試験にする。

## 提案
- なし（task の進捗の提案節に D1.5 の重複の計測を書いた）。
