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

## 追記（attempt 2, 読取のみの再確認）
- release `c893c911ff6f` は**出来ている**（`/local/celeris/state/releases/c893c911ff6f`、built_at 2026-10-20T05:58:53Z、schema_version 88、`bin/celeris` sha256 `f44d35502bdf251a5be0e6a197cc2344155e075b0960f1396adb5de600e51a94`）。ただし**昇格は人の操作（H1）**で、本 branch HEAD（ab11d799）はこの SHA より 11 commit 新しい（c893c911 は祖先）。測定は c893c911 の release を pin するか、HEAD の release を作り直して pin するかを人が決める。
- `/local/celeris/data/measure/main/{daemon.toml,state}` は旧 mlsys-main-1 のまま（`CELERIS_BIN` が消えた c9654bff5633）。新 manifest・env-lock・lock の再生成と測定 daemon の起動は本番 host 操作（`/local` 配下の書換・daemon 起動）で worker の禁止範囲のため未実施。よって**最新構成の preflight・実 pilot・raw 人操作の記録は未取得**。手順は上記「3」。
- 次の一手（人）: 上記 3 の 1〜4 を実行して preflight（無課金）を取る → 5〜6 で A0/B0 K4×1 の 2 試行 pilot。

## 検査結果
- `bash scripts/dev/test-parallel.sh`: exit 0（6198 passed, 0 failed, 15 ignored。初回は上記 flaky 1 件で exit 100、修正後に再実行）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 追記（attempt 3）: 実 `celeris` daemon での無 LLM 通し（最新 code）
同一構成（この branch の HEAD `8fe6c59a453d`。main `a1f33d76` + 本 task の試験修正）の実 binary で、通常 launcher の全段を通した。
LLM・userns・本番 host・cluster は使っていない（A は claude stub、B は実 `claude_code` adapter に非 LLM の program を command として渡す、能力検査は実 `cap-worker`）。

- 入口: `LAUNCHER_REAL_EVIDENCE=<dir> LAUNCHER_REAL_RELEASE=$(git rev-parse --short=12 HEAD) cargo test -p celeris-campaign --test launcher -- --ignored --exact real_daemon_main_set_runs_through_the_launcher_without_llm` → 1 passed（42.9 s）。証跡は run の成果物 `real-daemon-rehearsal/`（`steps.json`＝各段の exit と出力、`root/`＝測定 root 一式）。
- pin: daemon `target/debug/celeris` sha256 `cf42f63e…c7cf809`（`/proc/<pid>/exe` が同じ実体）、`celeris-campaign` sha256 `c817bbc9…21281a3`、release `8fe6c59a453d`（一時 releases_dir の外＝unmanaged で起動）、campaign `research-eval-main-rehearsal-1`（`experiment_sets.s`＝A/B、`selected_sets`、`approved_trials 12`、`shared_ledger` 宣言、K4×1 rep）、campaign_sha256 `ffdc0b5e…7cfbe6`。input/grader/env は `preflight_setup` の固定（試験 case・grader-private・cap-env）。
- 段ごとの結果: prepare 0 / start 0（`GET /api/v1/config` の identity.pid = launcher 所有 pid、release 一致）/ **preflight 0（`ok true`、`paid_calls_made 0`、`missing` 空。checked: manifest・versions・input-manifest・case-inputs・graders・case-time・inputs・capability・candidates・auth-env・snapshot-src・measure-env・env-check・trial-state・lock・budget）** / human-op record ×2（A0・B0、watch の前）0 / watch 0（settled、台帳 done 2、B は通常 `POST /api/v1/tasks` で実 dispatcher・実 worker 起動）/ stop 0 → start 0 → watch 0（再開: 台帳・投入数・A 起動数とも不変）/ collect 0（実験 case なし 2 件 skip）/ grade: 採点 2 件（`failure_class` model_failure ×2）、**check が非 0** / export 非 0（同じ check）/ 測定 DB の写し → `celerisctl campaign export` 0 / `human-op report` 0。
- 通しで見つけて直したもの（本 commit）:
  1. `human-op record --manifest <file>` が manifest の `sha256` 欄（実 manifest には無く空）を identity にして必ず拒否していた → runner・台帳・export と同じ `compute_sha256()` に（試験 `human_ops_cli::manifest_path_identity_uses_the_campaign_hash`）。
  2. runbook どおり pilot 開始時（初回 watch の前）に `<OUT>/human-ops.jsonl` を書くと、watch が「out has data but no lock.json」で拒否していた → lock の新規判定で `human-ops.jsonl` を runner の data に数えない（`lock_exec::tests`）。
  3. launcher が `REPO_SRC` を必須にして常に `--repo-src` を渡すため、K4 だけの最小 pilot が preflight の lock 検査で拒否されていた → 任意化（K2 を選べば従来どおり CLI が要求）。`docs/ops/measure-run-final.md` §2 を更新。
- **未解決（実 pilot の前に要る）**: 実 daemon の B の reviewer run に resolved routing が記録されず、check が `no resolved routing recorded (fell back to the global config?)` で拒否する（export も同じ理由で非 0）。今回の構成（fake source・route_defaults）由来か Celeris 本体の欠落かは未切り分け。B の reviewer run が 12 回（stub の verdict 数と実 criterion 数の不一致で review が繰り返された可能性）、watch の `tokens_total` が A の 105 だけで B の usage が数えられていない点も未確認。いずれも実 pilot の B 結果の有効性に直結するので、別 task で切り分ける（提案）。

## 枠と台帳の読取（attempt 3、読取のみ）
- `/local/celeris/data/measure/` は `main` だけで、共有台帳（`*ledger*`）は無い。旧 pin（`01M58VFRBA6HG0SSF3CG5BKM91/artifacts/pinned/main/final.json`）に `shared_ledger`・`approved_trials` の宣言は無い（grep 0 件）。= 承認枠（試行 12・3,000,000 tokens）の共有台帳上の既消費は 0、旧 pilot-2（488k）は別台帳で未算入。
- 最小実 pilot（A0/B0 × K4 × 1 rep、上限 600,000 tokens）は既存承認の内側で、追加承認は要らない。ただし上の「未解決」の check 拒否が残る間は B の結果が無効になるため、**実 pilot は未実施**（worker は LLM CLI を起動しない規則でもある）。raw 人操作の記録は、上の通しで watch の前から `<OUT>/human-ops.jsonl` に記録→report まで通ることを確認した（実 pilot での記録は未実施）。

## 状態区分（attempt 3 時点）
- 実装済: 研究評価接続 1・2・4（main）、本 task の修正 3 件＋flaky 試験の直し。
- 無 LLM 検証済: 実 `celeris` daemon（最新 code）での launcher 全段（上記）。preflight は最新 code で取り直した（旧 c9654bff5633 の結果は流用していない）。
- 実 pilot 実施済: なし。
- 未検証: B reviewer run の routing 記録（check 拒否）、B の usage 集計、本番 release（c893c911 / a1f33d76）の昇格後の実 preflight、実 LLM の A0/B0、7 日使用率の読取。

## 検査結果（attempt 3）
- `bash scripts/dev/test-parallel.sh`: exit 0（6212 passed, 0 failed, 17 ignored）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 後続 task 01M5C13NRT（B reviewer の routing 記録）— 途中経過
- **原因（特定済み）**: 測定 config の fake source/route_defaults ではなく、Celeris 本体の欠落。`pick_reviewer`（crates/task-dispatch/src/dispatcher/review_spawn.rs）が task の `routing_override` を渡さずに `select_route` を呼んでいたため、reviewer run の `route_decision.routing_resolved` が null になっていた（worker run は dispatch_run.rs で記録済み）。証拠: 通し記録 01M5BT8JYW/artifacts/real-daemon-rehearsal/steps.json で grade/export が reviewer run だけを `no resolved routing recorded` で拒否し、worker run は拒否されていない。修正: reviewer も override 付きの `select_route_with` で選び、`routing_resolved` を記録する（commit 30f45688）。
- **受け入れ試験**: `cargo test -p celeris-campaign --test launcher -- --ignored --exact real_daemon_main_set_runs_through_the_launcher_without_llm` → exit 0（1 passed, 42.7s）。試験の既定 release `rehearsal0001` が manifest の 12-hex 検査で落ちていたため、未指定時は checkout の HEAD sha12 を使うように直した（commit 3f1bd73d）。
- **watch の tokens_total に B が入らない理由**: 旧通しの watch は `tokens_total:105, tokens_missing:1`。runner.rs は B の case の `counted` が None のとき tokens_total に足さず tokens_missing を数える（runner.rs:1172/1182）。つまり B の usage は 0 として数えられたのではなく「未取得」として扱われている。celeris executor が B の usage を取れていない経路の特定は未了（次 run で確認）。
- **B reviewer run が 12 回の理由**: 未確認（仮説: stub reviewer が verdict を 1 件しか返さず criterion 数と合わないため review が不成立→再試行）。次 run で events を見て確定する。

## 後続 task 01M5C13NRT — 残り 2 点の確認（run 01M5C1WG）
- **B reviewer run が 12 回の理由（確定）**: 旧通しの測定 DB の写し（01M5BT8JYW/artifacts/real-daemon-rehearsal/root/measure-db-copy.sqlite3、読取のみ）の `runs`・`events` で、B task `01M5C0A1ZEA1` は worker 3 回 × reviewer 4 回 = 12。各 worker 試行の後、reviewer run 3 回が `harness_error`（class infra、outcome `requeue: adapter: claude exited without artifacts/result.json`）、4 回目が `failed`（`reviewer infra failure ×3`）→ `review_fail` で ready に戻り、3 回目の試行の後 task は failed。つまり verdict 数の不一致ではなく、B の stub（実 claude_code adapter の command）が reviewer run の成果物（adapter が要求する `artifacts/result.json`）を書かなかったことによる infra 失敗の 3 回再試行 × worker 再試行 3 回。Celeris 本体の再試行規則（reviewer infra ×3、worker attempt 上限 3）どおりで、本体の不備ではないので直さない。測定構成の試験 stub 側の問題で、現行 HEAD の受け入れ試験は exit 0 で通る。
- **watch の tokens_total に B の usage が入らない理由（確定）**: 同じ DB で B の worker run 3 本は `usage_json`（input 1000 / output 50）を持つが、reviewer run 12 本はすべて `usage: null`（harness_error/failed の run は usage を記録しない）。B task 自体が failed で終わったため、runner.rs:1139 の `res.tokens` が None になり、`counted` None → `tokens_missing += 1`（runner.rs:1172、`missing_tokens_policy` は「cap 判定では per_case.max_tokens を課金、0 とは数えない」）。つまり B の usage は 0 として隠れたのではなく「未取得」として明示されており、集計規則どおり。直さない（B が review を通れば tokens は数えられる。実 pilot では実 reviewer が result.json/review.json を書く）。
- **検査（この run）**: `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。受け入れ試験は前 run（3f1bd73d 以降）で exit 0（1 passed）。
- **提案**: reviewer run の usage を harness_error/failed でも（adapter が取れた分は）記録すると、実 pilot の失敗試行の token 消費を台帳に正しく載せられる。別 task で検討。

## 再開（2026-10-21, attempt 1 of 再開 run）: 新 campaign `mlsys-main-3` の再 freeze と最新 release の preflight
人の決定（2026-10-21）: A0/B0 × K4 の 2 試行、`per_case.max_tokens`=1,500,000（`token_basis=all`）、この pilot 用に tokens 全体 3,000,000、fable 不使用。campaign sha256 の承認が出るまで有料呼び出しはしない（本 run は `paid_calls_made 0`、worker は LLM CLI を起動していない）。

### 実装（本 run の変更）
- main `641f334b6d64`（01M5C8KF8Y の修正入り）を取り込み（衝突 3 件は main 側を採用）。コード差分はこの 2 点のみ:
  - `preflight::APPROVED_PER_CASE_TOKENS` 300,000→1,500,000、`APPROVED_PER_ARM_TOKENS` 1,300,000→1,500,000（上の決定。旧値だと preflight が `exceeds the approved` で拒否した。全体 3,000,000・並列 1・token_basis=all は不変）。`docs/ops/measure-run-final.md` 冒頭に決定を追記。
  - `launcher.rs` の実 daemon 通し試験の task-template から `repos`・scratch repo を除去（D2 で template の `repos` は拒否されるため、main の試験が `auth-env` 拒否で落ちていた）。

### 同一構成の pin（成果物 `artifacts/mlsys-main-3/`、測定 root は `/local/celeris/data/measure/main-3/{state,out}`）
| 項目 | 値 |
|---|---|
| code SHA | main `641f334b6d64`（本番稼働中の release。`promoted.json` あり）。B の `harness_version`=`641f334b6d64`。campaign binary は本 branch の HEAD（main + 上記 2 点）から build、sha256 `01fedb88…562f1` |
| daemon | `/local/celeris/state/releases/641f334b6d64/bin/celeris` sha256 `441e24a3…c14`。起動中の `/proc/<pid>/exe` と一致（launcher が照合）、release 識別 `641f334b6d64`、測定専用 config（port 7733、db/ws/kb は `/local/celeris/data/measure/main-3/state`） |
| A | claude `2.1.303 (Claude Code)`、`A_ACCOUNT=claude_max_lab`（login 済み・7 日使用率 0.22 ＜ 45%、5 時間 0.07、2026-10-21T06:02Z の通常読取） |
| manifest | `pinned/final.json`（campaign_id `mlsys-main-3`、`selected_sets=[main]`、arm A/B、K4 のみ、`approved_trials` 2、`shared_ledger`=`root/global-ledger`、`role_models` 全 `claude-sonnet-5-5`、budget: 全体 3,000,000 / per_case 1,500,000 / per_arm 1,500,000 / token_basis all）。file sha256 `3db5a3b7…7f84f`、**campaign_sha256 `043d5f52e4da26785395d3096ef8ea9c00a641dd84916d14d6999287da7becb0`**（validate exit 0、dry-run `llm_calls_made 0`・`total_trials 2`・within_budget） |
| input | `pinned/final-input-manifest.json`（prompt を `materials/<case>/case.json` 参照に直し、`manifest_hash` を作り直した。内部 hash `62871a59…0b98`。旧 `6fa8f0ec…` と別物。K2 problem も入っているが選択は K4 のみ）。固定資産 `src/{materials,kb,inputs}` は旧 mlsys-main-1 の固定値の複写（materials_sha256 `dab3ca05…800d`、kb `741f2414…2fe0702`） |
| grader / env | `pinned/fixed/{cases,grader-private,grade-cases}`（旧固定の複写）、`pinned/cap-env.json`（A と worker が同一 toolchain）、`launcher.env` / `daemon.toml`（sha256 は `pins.sha256`） |
| lock | `offline-lock`（`paid_calls_made 0`）: lock.json `b353b4ee…108ef`、env-lock.json `3f0bc1d2…9806f`、モデル候補は本番 `GET /api/v1/llm/models` の保存（sha256 `1f060a51…bf19`）。`$OUT` へ複写済み |
| 最新 preflight | `artifacts/mlsys-main-3/preflight.json`: `ok true`・`paid_calls_made 0`・`missing` 空・campaign_sha256 一致・B probe の daemon release `641f334b6d64`（pid・instance_id 付き）。checked は manifest・versions・input-manifest・case-inputs・graders・case-time・inputs・capability・candidates・auth-env・snapshot-src・measure-env・env-check・trial-state・lock・budget。旧 c9654bff5633 / 302e28550103 の preflight は流用していない |

### 無 LLM 通し（最新 HEAD、実 `celeris` daemon）
`LAUNCHER_REAL_EVIDENCE=… cargo test -p celeris-campaign --test launcher -- --ignored --exact real_daemon_main_set_runs_through_the_launcher_without_llm` → 1 passed（42.7s）。`real-daemon-rehearsal/steps.json` の全段が exit 0: prepare / start / preflight（ok・paid 0）/ human-op record ×2（watch 前）/ watch / stop→start→watch（再開で二重投入なし）/ collect / grade（check を含む）/ export / `celerisctl campaign export` / `human-op report`。前回 attempt で非 0 だった grade・export の check（B reviewer の routing 欠落）は 641f334b に入った修正で解消している。

### 枠の確認
- 枠は人の決定どおり: 3,000,000 tokens（旧 pilot 488k を保守的に算入した残 1,152,306 に 1,847,694 を追加）、per_case 1,500,000、2 試行、HPC 投入なし（K4 は cluster を使わない）、fable 不使用。共有台帳 `root/global-ledger` は新規（既消費 0、旧 main-2 台帳は別）。7 日使用率は上記 22%。

### 状態区分
- 実装済: 上記 2 点＋ 641f334b の修正群。
- 無 LLM 検証済: 実 daemon 通し（上）、最新 release の preflight。
- 実 pilot 実施済: **なし**（campaign sha256 の承認待ち。承認前は watch を起動しない）。
- 未検証: 実 LLM の A0/B0、実 token の台帳照合、実 pilot の raw 人操作（手順は下）。CoS は測定に含めない（前記 §4 のまま）。

### 承認後に人が行う手順
```sh
E=/local/celeris/data/workspaces/01M5BT8JYWTNTFXZHCYSFQHSEX/artifacts/mlsys-main-3/root/launcher.env; L=scripts/campaign/measure-launcher.sh
sh $L start --env $E          # 止まっていれば（preflight 後に stop 済み）
sh $L preflight --env $E      # 無課金で再確認
# watch の前に raw 人操作を開始（docs/ops/eval-human-ops-runbook.md、<OUT>/human-ops.jsonl）
sh $L watch --env $E > $(dirname /local/celeris/data/measure/main-3/out)/runs/watch-1.json
sh $L grade --env $E; sh $L export --env $E      # RUNS は OUT の外
```
