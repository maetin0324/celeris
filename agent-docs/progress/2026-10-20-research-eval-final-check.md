---
title: 研究評価接続 3/P0・統合 — 修正後の同一構成での無 LLM 通し確認と最小実 pilot の可否
tasks: [01M5BT8JYWTNTFXZHCYSFQHSEX]
status: partial
updated: 2026-10-20
---
# 研究評価接続 3/P0・統合

## 結論（状態の区別）
- **実装済み**: 研究評価接続 1（launcher 接続 adb335c6）・2（失敗原因保持 79b13d88 / c893c911）・4（human-op report 7223b59d）が main に入っている。本 task で足した実装は 1 件だけ（下記の flaky 試験の直し）。
- **無 LLM 検証済み**: 下記「無 LLM 通し」の試験。launcher → preflight → 通常 API 起動経路（所有 stub）→ 入力隔離 → 台帳 → 停止/再開 → 評価側観測 → 独立 grader → 原因別結果 → export を、実 CLI／実 launcher 入口で通す。
- **実 pilot 実施済み: なし**（理由は下記。worker は LLM CLI・本番 release 昇格・測定 daemon の起動をしない規則）。
- **未検証**: 最新 SHA での実 daemon preflight、実 LLM の A0/B0 試行、実 token の費用照合、B0 の cos run 観測、CoS を測定に含めるかの固定。

## 0. 同一構成の pin（取り直しできなかった部分を含む）
| 項目 | 現状（2026-10-20 時点、読取のみ） |
|---|---|
| code SHA | main `c893c911ff6f`（本 branch の base）。研究評価接続 1・2・4 を含む |
| release | `/local/celeris/state/releases/` に `c893c911ff6f` は**まだ無い**（delivery が `prepare.sh c893c911…` を実行中）。既存は `cfae4a9a2684`・`7223b59d37b7`・`cb0cd7b8e5d6` 等。**旧 `c9654bff5633` は消えており**（`check-main-pins.py` が `CELERIS_BIN does not exist` で非 0）、その preflight・旧 pilot は流用しない |
| manifest / 選択セット | 前回 pin（`01M58VFRBA6HG0SSF3CG5BKM91/artifacts/pinned/main/final.json`, campaign_sha256 `8ca0e198…`）は experiment_sets・`shared_ledger`・`approved_trials` の宣言が無い旧形式。新しい契約（docs/ops/experiment-sets.md）では `selected_sets=["main"]`・共有台帳宣言・`approved_trials` を持つ新 manifest（新 campaign_id）が要り、これを作ると campaign_sha256 も変わる |
| 共有台帳 | `global-ledger.jsonl` は host 上に**存在しない**（`find /local/celeris` で測定用のものは無し）。= 既消費 0 として新規に作られる。承認枠の既消費は無い |
| A（claude） | `claude` は `~/.local/bin/claude`、`bwrap` あり。前回承認値は harness_version 2.1.303・`A_ACCOUNT=claude_max_lab`。**現在の version・login 状態は未確認**（worker が LLM CLI を起動しない規則のため `claude --version` も実行していない） |
| input / grader / env pin | 前回 pin の `fixed/{cases,grader-private,grade-cases}`・`measure-src/main/*`・`cap-env.json` が実在（`check-main-pins.py` の NG は CELERIS_BIN の 1 件のみ）。release 差し替え後に hash を再計算する必要がある |

→ 「同一構成での取り直し preflight」は、(a) c893c911 の release が出来て昇格される（人: H1）、(b) 新 manifest・env-lock・lock の再生成、(c) 測定専用 daemon の起動、が前提で、いずれも本 run では行えない。**最新版の preflight は未取得**（旧結果を流用していない）。

## 1. 無 LLM 通し（本 run で実行）
- `cargo nextest run -p celeris-campaign --test launcher --test arm_set_cli_normal --test global_ledger_atomic --test human_ops_join`: 46 passed, 0 failed。
  - launcher 入口（`launcher.rs`）: 共有台帳宣言の強制、B/C のみは A 認証不要、variant hash の拒否、`grade` の評価側観測収集（trial identity・台帳 job への結び付け、改変・別 trial の拒否）。
  - `arm_set_cli_normal.rs`: 実 CLI で A/B・B/C セット、通常 `POST /api/v1/tasks` の所有 stub、固定証拠と共有台帳。
  - `global_ledger_atomic.rs`: 台帳の原子性・予約回復。`human_ops_join.rs`: 人の操作 raw と試行の join。
- 全体は `bash scripts/dev/test-parallel.sh`（下記の検査結果）。停止/再開は `a_program_pin_and_watch.rs`・`runner_shared_admission.rs`、独立 grader・原因別結果は `grade_failure_class.rs`・`grader_independent.rs`、export は `final_accounting.rs`・`human_ops_report.rs` が受け持つ。
- 本 run の実装修正: `a_program_pin_and_watch::watch_stop_and_restart_reattaches_without_resubmit_and_stops_at_the_cap` が全体並列で `one process per trial left 2 right 3` と落ちた（子 process の起動前に launches を数える競合）。launches が 3 件になるまで待つ出来事待ちに直した（時間待ち・負荷は足していない）。

## 2. 枠の確認（読取のみ）と最小 pilot の範囲
- 既存の承認（`docs/ops/measure-run-final.md` 冒頭、人の決定 2026-10-16）: 試行 12 以内・tokens 全体 3,000,000・per_case 300,000・per_arm 1,300,000・`token_basis=all`・claude 7 日使用率 45% 以内・codex 21%・fable 不使用。HPC job の投入枠は K2/K4 とも cluster 投入を伴わないので追加枠なし（新規 HPC 枠は求めない）。
- 共有台帳は未作成＝この承認の既消費は台帳上 0。ただし旧 pilot（pilot-2 A のみ 488k）が別台帳で消費済みで、これが 3M に算入されるかは台帳に載っていないため**不明**。
- **提案する最小実 pilot**（既存枠の内側。再承認・枠増額は不要）: 主比較 A0/B0 × K4（analysis。K2 より軽い）× 1 rep = **2 試行**、上限は per_case 300,000 × 2 = **600,000 tokens 以内**。旧 pilot 488k を全額算入しても 1,088,000 < 3,000,000、試行数 2 + 旧 1〜2 ≪ 12。K2 を足す場合でも 4 試行・1,200,000 tokens で枠内。
- 不明部分（人の確認が要る最小）: ① 現時点の claude 7 日使用率 U（45% 以内か。`GET /api/v1/llm/accounts` の通常読取で人が見る）、② `claude_max_lab` が login 済みか、③ 旧 pilot の消費を台帳へ算入するか。①②が OK なら追加承認は要らない。

## 3. 人が実行する手順（本番 host・実 LLM。worker は実行しない）
1. c893c911 の release 完了後、人が `promote` で昇格し sha12 を控える（H1）。
2. 新 campaign_id（例 `mlsys-main-2`）の manifest を作る: `experiment_sets.main`（arms A,B）・`selected_sets=["main"]`・`approved_trials=12`・`shared_ledger=<絶対 path>`・固定欄 `role_models` 全 sonnet・`initial_info`=材料 hash。問題は K4 のみ（pilot 用）。`celeris-campaign validate` → `dry-run`（`llm_calls_made=0`、`total_trials=2`）。
3. `offline-lock` で env-lock/3・lock を再生成し、launcher.env の `CELERIS_BIN`・`RELEASE`・`GLOBAL_LEDGER`・`MEASUREMENT_ID` を更新（本番 launcher.env への追記は人）。`python3 scripts/campaign/check-main-pins.py --root <root>` が 0 になることを確認。
4. `sh scripts/campaign/measure-launcher.sh prepare|start|preflight --env <env>`（preflight は無課金。`paid_calls_made 0`、`missing` 空を確認。**これが最新版の preflight**）。
5. **pilot 開始の前に** raw 人操作の記録を開始する（`docs/ops/eval-human-ops-runbook.md` の `celeris-campaign human-op` で `<OUT>/human-ops.jsonl` へ追記。試行の identity に紐付け）。その後に `watch`。
6. `watch` 終端 → `collect` → `grade` → `export`。`final_accounting` で consumption を台帳・daemon の usage と照合。human-ops の join は `human_ops_join` / `human_ops_report` が出す。
7. 不合格行には `failure_class`（model_failure / env_failure / undetermined）が必須。旧 `grades.jsonl` は再採点。

## 4. CoS の扱い（受け入れ 4）
- 測定範囲は **CoS を含めない**。pilot は `POST /api/v1/tasks` から直接投入し、CoS chat run は呼ばない（task 木に紐付かない。ADR 2026-10-19-experiment-sets 付記）。したがって「全 role Sonnet／完全な総費用」とは主張しない。主張できるのは task 木に紐付く worker・reviewer・planner run の model と、その tokens までで、B0 の `role_models.cos` は宣言のみで未観測。
- CoS を測定に含めるには chat run の試行帰属（trial identity ↔ chat run id）と実 model/harness・消費の結び付けが要る。これは新機構で本 task の範囲外。必要なら別 task。

## 5. 未解決・提案
- 最新 release（c893c911）の昇格後に、上の 3〜6 を人が実行すれば最小 pilot を回収できる。成功基準は 2 試行の終端、`failure_class` 付き grade、台帳と usage の照合一致、human-ops の join。
- 提案: 旧 pilot の消費（488k）を共有台帳へ取り込む手順を 1 つ決める（取り込まないと 3M の残が過大に見える）。

## 検査結果
- `bash scripts/dev/test-parallel.sh`: exit 0（6198 passed, 0 failed, 15 ignored。初回は上記 flaky 1 件で exit 100、修正後に再実行）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
