---
title: 全体棚卸しの debt の draft 起票
tasks: [01M6ZDF99MKHFN57PN0H7G6Z9Z]
status: done
updated: 2026-11-09
---
# 全体棚卸しの debt の draft 起票

## 変更

- `phase 1: file health debt drafts from inventory blocking` — `crates/task-ops/src/health_debt.rs`（関数 `file_health_debts`、入力 `HealthDebtInput`、結果 `FiledHealthDebts`、`debt_key`、定数 `DEBT_KEY_PREFIX`・`DEBT_OPERATION`・`DEBT_LABEL`・`MAX_DEBTS_PER_FIRE = 3`）と試験 `health_debt/tests.rs`（`health_inventory_*` 6 件）を足し、`lib.rs` に mod 宣言を 1 行（と doc 1 行）足した。
- 起票の対象は `HealthEvidence.blocking` の fingerprint だけ。verdict が `Fail` のときだけ起票する（`Pass`・`PolicyOnly`・`Error`・`NotApplicable` は何もしない）。severity・delta・exception・候補の判定は Rust に無い（正本は base 側の検査器の fail 級の判定で、gate の judge が `blocking` に入れる）。
- `HealthBlockingDetail` は題・objective の文面（path・rule_id・message）に使うだけ。`blocking` に無い fingerprint の detail は読まない。
- 冪等キーは `health-debt:<repo_id>:<fingerprint>:<head_sha>`。既存の debt は `SqliteStore::idempotency_keys_with_prefix("health-debt:<repo_id>:<fingerprint>:")` で引き、終端でない task があれば起票せず `existing_skipped` に数える。
- 作成は既存の経路だけを使う: `add::build_task_with_roles`（`POST /tasks` と同じ組み立て）と `SqliteStore::create_task_with`（冪等キーの照合 `idempotency_lookup_tx` と記録 `idempotency_record_tx` を task と同じ transaction で書く。project controller の root と同じ形）。新しい作成経路・store の method は足していない。
- task は `draft`・`created_by = daemon`・`provenance = System`・label `health-debt`・案件と repo を指定。親・依存・`stages_hint` は持たない。受け入れ条件は reviewer の 1 件（「検査器の report にその fingerprint の fail 級の finding が無く、変更が objective の path に限られている」）。objective はその finding の path だけを挙げ、「path の外を変えない・計画・子 task・ほかの debt のまとめを作らない」と書く。

## 実装前 brief

1. 根本原因: 棚卸しの評価（`HealthEvidence`）から debt の提案を作る口が無い。runner（unit runner）が使う起票の関数が要る。
2. 責務 owner: 地図の「定期実行（cron job）・受信箱の片付け規則・日次整理・統括 CoS の定期見直し」の行（task-core::cron / task-ops / task-dispatch）。task の作成の組み立ては task-ops::add（`POST /tasks` と共有）。表と冪等キーの読み書きは task-core の store（既存の method を使うだけ）。
3. 既存の再利用先: `add::build_task_with_roles`、`SqliteStore::create_task_with`・`idempotency_lookup(_tx)`・`idempotency_record_tx`・`idempotency_keys_with_prefix`、`Status::is_terminal`、`SpecProvenance::system`。
4. 選択肢: (a) store に起票専用の transaction method を足す（store の size.responsibility-growth が悪化し、作成経路が増える。不採用）、(b) task-api の `create_task_op` を呼ぶ（task-ops から API 層へ依存が逆向きになる。不採用）、(c) task-ops で既存の組み立てと `create_task_with` を使う（採用）。
5. 新しい状態・分岐・fallback・port: 無し。冪等キーの行（既存の表 `task_idempotency_keys`）を 1 件の debt に 1 行足すだけ。
6. 撤去対象: 無し（新しい機能で、置き換える旧い仕組みが無い）。
7. 不変条件と試験:
   - 同じ repo と fingerprint の終端でない debt があれば、同じ head でも別の head でも 2 件目を作らない（`health_inventory_does_not_file_same_debt_twice`）。
   - 起票は draft の 1 件だけで、親・子・依存・段の名指しを持たず、objective は自分の finding の path だけを挙げる（`health_inventory_files_drafts_only`）。
   - 1 回の発火の起票は 3 件まで、残りは `limit_remaining`、枠を超える入力は 3 に切り詰め、枠 0 なら何も作らない（`health_inventory_caps_filing_at_three`）。
   - 起票の対象は `blocking` だけ。detail にだけある fingerprint は起票せず、`blocking` にあって detail の無いものは `missing_detail` に数える（`health_inventory_files_only_checker_blocking`）。
   - verdict が Fail でなければ task も冪等キーも作らない（`health_inventory_files_nothing_unless_fail`）。
   - debt が終端になった後は別の head で起票し直す。同じ head のキーは使用済みなので作らない。接頭辞の同じ別の fingerprint（`fp-a:x`）の task は `fp-a` を塞がない（`health_inventory_refiles_after_debt_task_closed`）。
   - 既存の試験は作成の経路（`add`）と冪等キーの store（`idempotency`）を別々に見るだけで、この契約を検出できないので専用の試験を足した。

## ADR (6) との違い

ADR 本文は変えていない（G3 closeout が付記する）。

- D6.6 は「fail 級の finding（severity error、delta new か worsened、候補でない）だけ」と書くが、Rust ではこの判定をしない。`HealthEvidence.blocking` が検査器の fail 級の判定の結果で、それだけを対象にする（plan の objective の指示。判定の正本を 2 つにしない）。結果は同じ集合になる。D6.7 の「候補の finding は起票しない」も、候補が `blocking` に入らないことで満たす。
- repo id は入力の別の欄でなく `HealthEvidence.repo_id` から取る（同じ値を 2 か所で受けない）。案件に無い repo は `OpsError::Validation`。
- 結果の型に `missing_detail`（`blocking` にあるのに detail が無い、または detail に path が無い数）を足した。objective を path に限れないので起票しない。表 `health_inventory_runs` にはこの数の列が無いので、runner は `CronJobRun.detail` に書く（unit runner への引き継ぎ）。
- 同じ head のキーを持つ debt が終端になった後、同じ head で再び評価されても起票しない（冪等キーは使用済み。`existing_skipped` に数える）。D6.4 により同じ head は通常省略されるので、起票し直すのは head が変わり、その fingerprint が再び new か worsened になったときだけ。
- 1 回の発火の上限は、runner が渡す残りの枠（`remaining`）で数え、`MAX_DEBTS_PER_FIRE` を超える値は切り詰める。発火をまたぐ数え方は runner が持つ。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-ops health_inventory_` → exit 0、6 passed。`cargo nextest list -p task-ops health_inventory_` の件数 6（6 以上）。
- plan の範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` と plan の正規表現）→ 範囲外の path 無し。変更は `crates/task-ops/src/health_debt.rs`・`health_debt/tests.rs`・`lib.rs`・この file だけ。
- plan の grep の check（`health-debt:`・`blocking`・進捗の file）→ exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（542 秒）。対象は celeris・celeris-campaign・celeris-mcp・celerisctl・llm-proxy・task-api・task-dispatch・task-ops・task-worker と web。冒頭の check-health は verdict pass・exit 0・fail_grade 0・new 1・worsened 0・existing 717（new の 1 件は `dup.contract` の候補の warning で、`required_checks/health.rs` と `route_select/availability.rs` の `UnavailableReason`。どちらもこの unit で変えていない）。`reviewable=false` は `$CELERIS_WU_BASE`（WU の範囲の基点の commit）が HEAD の祖先でないため。cargo nextest 6472 passed・23 skipped、doc test 合格、web の unit 試験 104 file 711 passed、script bundle の SKIP 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check`・`sh scripts/dev/check-doc-links.sh`・`git diff --check` → exit 0。

## 本番への移行

この unit は関数を足すだけで、cron を有効化しない。config・表の変更は無い。本番の操作はしない。有効化の人の手順は inventory の record unit が記録する。

## 未解決

- `file_health_debts` を呼ぶのは unit runner（まだ無い）。runner は発火の残りの枠を repo の順に渡し、`FiledHealthDebts` の `task_ids`・`existing_skipped`・`limit_remaining` を `health_inventory_runs` の行に、`missing_detail` を `CronJobRun.detail` に写す。

## 提案

- G3 closeout が ADR (6) D6.6 に「Rust は `blocking` を対象にするだけで fail 級の判定をし直さない」「`missing_detail`」「同じ head のキーは debt の終端後も再利用しない」を付記する。
