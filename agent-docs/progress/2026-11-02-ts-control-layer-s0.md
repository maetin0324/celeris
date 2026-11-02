---
title: S0 — 最新 main の棚卸し・Rust/TS 境界 ADR・edit→activate の baseline と S1〜S6 の実装計画
tasks: [01M6DCPKPJZZ9F9J4QJ47FH8KK]
status: done
updated: 2026-11-02
---
# S0 — 棚卸し・境界 ADR・baseline と S1〜S6 の実装計画

完了日: 2026-11-02（S0 の task 01M6DCPKPJZZ9F9J4QJ47FH8KK。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- S0 は棚卸し・baseline・境界 ADR・実装計画の 4 つを揃えた。コードは計測 script 1 本だけを足し、crates/・web/ は変えていない。
- 親 task は S0 だけでは done にならない。S1〜S6 の子 task（S5 と S6 は 1 つの子 task の中で S5→S6 の順）の実装と受け入れが残る。
- 最小 PoC は「lane 規則表の当て方」（`task-core/src/model_policy.rs` の `RULES` を上から当てる部分）。理由と比較 fixture は §3。
- TS の置き場は repo 直下の `control-ts/`、bundle と manifest は `control-ts/dist/` に作り、配備先は state dir の `script-bundles/<bundle_sha256>/` にする（この計画で決める。ADR への付記は S1 で行う。提案 1）。

## 1. S0 の WorkUnit と記録
| WorkUnit | 記録 | commit（件名） |
|---|---|---|
| inventory | [inventory.md](2026-11-02-ts-control-layer-s0/inventory.md) | phase 1: S0 棚卸し表（TS 移行／Rust 維持／判定のみ切出し）と S4 候補を inventory.md に書く |
| baseline-tool | [baseline-tool.md](2026-11-02-ts-control-layer-s0/baseline-tool.md)、[baseline.md](2026-11-02-ts-control-layer-s0/baseline.md) | phase 1: edit→activate の計測 script と小さな方針変更（policy）の baseline |
| baseline-cos | [baseline-cos.md](2026-11-02-ts-control-layer-s0/baseline-cos.md)、`baseline/cos*.jsonl` | phase 1: CoS のまとまった制御変更（cos）の baseline と 2 scenario の段別比較 |
| adr | [adr.md](2026-11-02-ts-control-layer-s0/adr.md)、[境界 ADR](../adr/2026-11-02-ts-control-layer-boundary.md) | phase 1: Rust/TS 境界 ADR（Host API・generation・版固定・数値目標と停止基準） |
| s1-s6-plan | このファイル | phase 1: S1〜S6 の実装計画と S0 の進捗まとめ |

## 2. S1〜S6 の実装計画
共通の前提（全段）:
- 基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。Rust の検査は毎段 `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`。
  全体試験は release gate だけ（ADR 2026-10-20-full-tests-at-release-only）。
- TS 側は `control-ts/` で、pnpm は `corepack pnpm@12.6.0` で版を固定する（`control-ts/package.json` の `packageManager` も `pnpm@12.6.0`。`web/` と同じ）。
  依存は offline の store から入れる: `cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile`。
- 言語移行の差分に挙動・方針の変更を混ぜない。既存の安全検査（`plan_conforms`・`job_wait_extend`・`review_rework` の basis 検査、human check の 403、`policy_version` の 422）は Rust の現在の場所から動かさない。
- 旧 `[model_routing]`・tier binding・`routing_shadow` の表を復活させない。比較の名前は `compare`（ADR (d)）。
- IEEE Access・K2・K4 の測定環境の config に `[script]` を足さない（ADR (g)）。

### S1: 埋め込み runtime・最小 Host API・独立 TS artifact の経路（依存: S0）
作るもの:
- 新 crate `crates/script-runtime/`（package `celeris-script`、`deno_core = "=0.413.0"`）。workspace の `Cargo.toml` の `members` に足す。
  - `src/lib.rs` — 公開は `ScriptRuntimeHandle`・DTO・`ScriptError` だけ（`deno_core` の型を出さない）
  - `src/actor.rs` — 専用 `std::thread` が `JsRuntime` を生成・所有・破棄し、current-thread の tokio runtime で event loop を回す ScriptRuntime actor
  - `src/handle.rs` — bounded mpsc（容量 64）・`oneshot`・`request_id`（ULID）・`deadline`・`try_send` 満杯で `Busy`
  - `src/watchdog.rs` — deadline 超過で `IsolateHandle::terminate_execution`
  - `src/ops.rs` — `#[op2]` の native op（最小: `op_host_version`、試験用の async op 1 本）と capability・scope の照合
  - `src/manifest.rs` — `celeris.script-manifest/1` の読み込みと `bundle_sha256`・`host_api`・`schema_sha256` の検証
  - `src/loader.rs` — bundle 自身と `celeris:host` 以外の import を拒否する module loader
  - `src/bootstrap.js` — 静的 bootstrap（`Math.random`・`Date.now` の扱い、`celeris:host` の結線）。業務コードは入れない
  - `src/error.rs` — `ScriptError::{Exception, Timeout, Busy, InvalidOutput, Denied, Terminated, GenerationGone}`
  - `src/bin/script-schema.rs` — DTO から JSON Schema を出す（`schemars`）
  - `tests/runtime.rs` — sync・async・exception・cancel・timeout（同期無限ループ）・busy queue・restart・権限拒否の統合試験
- `docs/api/script-host/host-v1.schema.json`（生成物を commit）
- TS の置き場 `control-ts/`: `package.json`（`typescript` 7.0.2、`esbuild` 0.28.2、`vitest` 5.0.0 を固定）・`pnpm-lock.yaml`・`tsconfig.json`・
  `src/host.d.ts`（schema からの生成物）・`src/domains/`（業務コード）・`scripts/gen-host-types.mjs`（`web/scripts/gen-types.mjs` と同じ方式）・
  `scripts/build-bundle.mjs`（esbuild で単一 ESM、Node 組み込みの import で失敗、`dist/bundle.mjs` と `dist/manifest.json` を書く）
- 事前 build の `v8` archive: repo の外 `/local/celeris/data/scratch/rusty-v8/` に置き、`RUSTY_V8_ARCHIVE` で指す（人が置く。§5 未解決 1）。

触る既存 file: `Cargo.toml`・`Cargo.lock`、`crates/celeris/Cargo.toml` と daemon の起動（`crates/celeris/src/daemon/` の中で handle を作り、全領域 `rust` なら isolate を起こさない）、
`docs/architecture-map.md`（`crates/script-runtime/` の行）、`scripts/dev/test-affected.sh`（`control-ts/*` と `docs/api/script-host/*` の変更で TS 側の検査を走らせる）、
`agent-docs/adr/2026-11-02-ts-control-layer-boundary.md`（付記: TS の置き場・`Math.random` の扱い・isolate 再利用の結論）。
範囲 check には最初から `Cargo.toml`・`Cargo.lock`・`crates/script-runtime/`・`crates/celeris/`・`control-ts/`・`docs/architecture-map.md`・`docs/api/script-host/`・`scripts/dev/test-affected.sh`・ADR を含める（adr.md の提案と同じ）。

受け入れの検証コマンド:
```sh
cargo check -p celeris-script && cargo tree -d -p celeris-script            # rustc 1.98.1 で build できるか、版の衝突
cargo nextest run -p celeris-script --test runtime                           # 8 種の統合試験
cargo tree -i deno_core --workspace -e normal --prefix none | awk '{print $1}' | sort -u   # celeris-script・celeris・deno_core だけ
cargo run -q -p celeris-script --bin script-schema > "$TMPDIR/h.json" && cmp "$TMPDIR/h.json" docs/api/script-host/host-v1.schema.json
cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile \
  && corepack pnpm@12.6.0 run gen:types && git diff --exit-code src/host.d.ts \
  && corepack pnpm@12.6.0 run typecheck && corepack pnpm@12.6.0 run test && corepack pnpm@12.6.0 run bundle
# TS artifact の更新で Rust build が起きないこと: bundle を作り直した後に
cargo build -p celeris -v 2>&1 | grep -c Compiling                           # 0
bash scripts/dev/test-affected.sh && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check
```
数値（ADR (f) の許容 overhead）: 判定 1 回 p50 ≤ 1 ms・p99 ≤ 5 ms、RSS 増分 isolate 1 個 ≤ 100 MiB、`celeris` の cold release build の増分 ≤ 60 秒、
`task-dispatch`・`task-api` の test-build の増分 ≤ 2 秒。超えたら S2 へ進まない。

### S2: 純粋ロジックの同等実装と差分検証（依存: S1）
最初の領域は §3 の PoC（lane 規則表の当て方）。続けて inventory §2.1・§2.2 の順（`llm_route::route_priority` → `execution_gate` の得点 → `route_select/{policy,lane,balance}` の順位付け）で 1 領域ずつ足す。`direct_route::evaluate` は Rust 維持（inventory §2.3）。

作るもの:
- `crates/task-core/src/model_policy.rs` の分割（言語移行と別 commit、挙動は不変）: `match_rule(&TaskFeatures) -> RuleMatch { rule_id, lane, condition }` と、
  それを受けて `LaneCeiling::clamp` を当てる `decide_with(features, ceiling, RuleMatch)`。`ModelPolicy::decide` は両者の合成にする
- `crates/task-dispatch/src/script_policy.rs` — trait `PolicyScript`（DTO だけを使う。`deno_core` に依存しない）
- `crates/celeris/src/daemon/script_policy.rs` — `PolicyScript` を `ScriptRuntimeHandle` で実装
- `crates/task-core/migrations/NNNN_script_domains.sql` — `script_generations`・`script_domain_modes`（ADR (d)。mode は `rust`・`compare` だけ S2 で使う）
- event kind `script_compare_recorded`（`crates/task-core/src/model/events.rs`）
- `control-ts/src/domains/lane_rules.ts`、`control-ts/test/lane_rules.test.ts`
- 比較 fixture `control-ts/fixtures/lane_rules/all-features.jsonl` と、それを Rust で出す `crates/task-core/tests/script_fixtures.rs`（§3）
- 比較 runner `crates/celeris/tests/script_compare_lane_rules.rs`（本物の deno_core host で fixture を全件流し、Rust 参照と一致を確かめる）

触る既存 file: `crates/task-core/src/model_policy.rs`、`crates/task-dispatch/src/dispatcher/dispatch_run.rs`（1379 行付近の `decide_for_task` の呼び出しで `compare` の記録を足す）、
`crates/task-core/src/model/events.rs`、`docs/api/script-host/host-v1.schema.json`、`docs/architecture-map.md`。
`crates/task-dispatch/src/dispatcher/work_units.rs` の 2019 行の呼び出しは S2 では Rust のまま（mode `ts` に上げる S5 で切り替える）。

受け入れの検証コマンド:
```sh
cargo nextest run -p task-core -E 'test(model_policy) | binary(script_fixtures)'
cargo run -q -p task-core --example dump-lane-fixtures > "$TMPDIR/f.jsonl" && cmp "$TMPDIR/f.jsonl" control-ts/fixtures/lane_rules/all-features.jsonl
cd control-ts && corepack pnpm@12.6.0 run typecheck && corepack pnpm@12.6.0 run test -- lane_rules && corepack pnpm@12.6.0 run compare -- lane_rules
cargo nextest run -p celeris --test script_compare_lane_rules              # 本物の host で 19,683 件一致、mode=compare の event 記録、rust への切り戻し
bash scripts/dev/test-affected.sh && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check
```
受け入れ: fixture 全件一致、安全回帰 0（天井越え・人の明示の上書きが出ない）、比較ログ（`script_compare_recorded`）と `rust` への切り戻しの試験、
`dispatch_run.rs` の本物の呼び出し経路で 1 領域。

### S3: TS 変更専用の高速検証経路（依存: S2。S4 と並行可）
作るもの:
- `scripts/dev/ts-verify.sh` — 変更を「TS だけ」「Host API（schema・`host.d.ts`・`crates/script-runtime/src/ops.rs`）」「Rust 基盤」に分け、TS だけなら
  型検査 → 対象の単体 → Rust/TS 比較 fixture → 安全シナリオ → 事前 build 済みの host での統合、を cargo を起動せずに走らせる
- 事前 build の host `script-host-runner`（`crates/script-runtime/src/bin/script-host-runner.rs`。bundle と fixture を受けて実行するだけ）と、その cache key
  （host の source の hash・`Cargo.lock`・features・rustc の版・`host-v1.schema.json` の hash）。key が変われば runner を作り直す
- Rust oracle の golden（S2 の fixture）を同じ key で無効化する
- 意図的破壊の試験 `scripts/dev/test-ts-verify.sh`（規則を 1 件壊した bundle を検出すること）

触る既存 file: `scripts/dev/test-affected.sh`（TS だけの変更では `ts-verify.sh` に委ね、Host API・Rust の変更では従来どおり cargo 側も走らせる）、
`scripts/dev/edit-activate-baseline.sh`（scenario `policy-ts`・`cos-ts` を足し、同じ編集課題を TS 経路で測る）。
CI 側の選択規則と grader は `scripts/dev/` の Rust 側の check が持ち、`control-ts/` からは変えられない形にする（S3 の試験で固定）。

受け入れの検証コマンド:
```sh
bash scripts/dev/ts-verify.sh --list                                         # 分類の結果
bash scripts/dev/ts-verify.sh                                                # TS だけの変更で exit 0、cargo の起動 0 回（記録の link_count 0）
bash scripts/dev/test-ts-verify.sh                                           # 意図的破壊を検出して exit 1 を期待する試験が exit 0
bash scripts/dev/edit-activate-baseline.sh --scenario policy-ts --mode warm --reps 20 --out agent-docs/progress/<S3 の進捗>/policy-ts.jsonl
bash scripts/dev/edit-activate-baseline.sh --validate agent-docs/progress/<S3 の進捗>/policy-ts.jsonl
bash scripts/dev/test-affected.sh && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check
```
数値: TS だけの変更の edit→activate を warm p50 ≤ 30 秒・p95 ≤ 60 秒、cold p95 ≤ 120 秒（ADR (f)）。baseline（policy warm 221.0 秒・cos 155.7 秒）と段ごとに並べる。

### S4: CoS のまとまった非同期経路の同等移行（依存: S2）
対象は inventory §4 の A「受信箱一次対応の digest」（`crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` の `digest_message`）。
起点（triage run の終了）→ Rust の読み出し（`cos_triage_run_report`・`chat_run_get`）→ TS の digest → Rust の書き込み（`chat_assistant_message_add_once`、冪等 key `COS_TRIAGE_DIGEST_KEY_PREFIX`+run_id）。
`running`→`pending` の戻しと書き込みは Rust に残す。続けて同じ形で B（定期見直しの入力文）に広げる。

作るもの:
- 前段の Rust 変更（言語移行と別 commit）: `CosTriageItemReport` に `Serialize`・`Deserialize`・`JsonSchema`（`crates/task-core/src/chat/triage.rs` 524 行）
- `control-ts/src/domains/cos_digest.ts`、`control-ts/test/cos_digest.test.ts`
- 記録済み入力の replay fixture `control-ts/fixtures/cos_digest/*.json`（`crates/task-dispatch/src/dispatcher/tests/cos_chat_triage_inbox_thread.rs` の 11 件の入力と期待を Rust から書き出す）
- `crates/task-dispatch/src/dispatcher/tests/cos_digest_script.rs` — event の順序・再配送・retry・cancel・再起動・`policy_version` の変化で、冪等 key により message が 1 件に限られること

触る既存 file: `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs`（`digest_one` で `PolicyScript` を呼ぶ）、`crates/task-core/src/chat/triage.rs`、`docs/api/script-host/host-v1.schema.json`。
LLM は呼ばない（digest は記録済みの run の結果だけを読む）。

受け入れの検証コマンド:
```sh
cargo nextest run -p task-dispatch -E 'test(/cos_chat_triage_inbox_thread|cos_digest_script/)'
cd control-ts && corepack pnpm@12.6.0 run typecheck && corepack pnpm@12.6.0 run test -- cos_digest && corepack pnpm@12.6.0 run compare -- cos_digest
bash scripts/dev/ts-verify.sh                                                # S3 が先に入っていれば TS 経路でも通す
bash scripts/dev/test-affected.sh && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check
```
受け入れ: replay の文字列と cards が byte 一致、安全ケースの回帰 0、message の二重投入 0。方針の改善は同等移行の完了後に別の差分で行う。

### S5→S6（1 つの子 task。中で S5 を先に終え、S6 を後に行う）
#### S5: generation 管理・段階切替・rollback（依存: S2・S4。基盤の表は S2 で作る）
作るもの:
- `crates/script-runtime/src/generation.rs` — 新 bundle を新 isolate で load・検証、旧 generation の drain（純粋判定 30 秒で打ち切り → `GenerationGone` → Rust 参照）
- `crates/task-core/src/store/script_domains.rs` — `script_domain_modes` の `version` CAS による atomic な切り替えと `script_generation_*` event の同一 transaction 追記
- checkpoint の版互換（`celeris.script-checkpoint/<n>`）と、副作用後は冪等 key と実行記録で照合し、照合できなければ止める処理
- 停止基準の監視 `crates/celeris/src/daemon/script_guard.rs`（ADR (f) の表: 説明のつかない不一致 1 件・安全違反 1 件・timeout 率と失敗率 0.1%/1000 呼び出し・10 分に timeout 3 件・連続失敗 3 件で `rust` へ戻す）
- API と CoS operations: mode の変更（`rust`→`compare` は既存の CoS 権限、`compare`→`canary`・`canary`→`ts` は人の承認、`rust` への戻しは誰でも）。
  `crates/task-api/src/` に endpoint、`crates/task-api/src/cos/operations.rs` の registry に分類を足す
- 起動時の復元（bundle の sha256 照合、合わなければ `rust` に戻し event と受信箱項目）
- `crates/celeris/tests/script_generation_e2e.rs` — 新旧併存 → 切替 → 途中 error → rollback → 再起動
触る既存 file: `crates/task-dispatch/src/dispatcher/work_units.rs`（2019 行の `decide_for_task` を mode に従わせる）、`docs/api/v1/api-v1.schema.json` と `web/api/generated`（再生成）、
`docs/architecture-map.md`、`docs/SPEC.md`（運用の節）。
受け入れの検証コマンド:
```sh
cargo nextest run -p celeris --test script_generation_e2e
cargo nextest run -p task-core -E 'test(script_domain)'
cargo nextest run -p task-api -E 'test(/script_mode|cos_op/)'
bash scripts/dev/test-affected.sh && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check   # web/ の型検査と単体も走る
```
受け入れ: event の欠落 0・意図しない二重副作用 0・未承認の本番変更 0。本番の mode を変えるのは人の承認の後（人が実行する手順を進捗の「本番への移行」に書く）。

#### S6: 実測・自己改善 pilot・移行範囲の確定（依存: S3・S4・S5）
作るもの: 測定の生データと報告 `agent-docs/progress/<S6 の進捗>/`（`edit-activate-baseline.sh` の `policy`・`cos`・`policy-ts`・`cos-ts` を同じ host・同じ編集課題で各 20 試行以上）、
自己改善 pilot 1 件（Celeris が TS の制御コードを変更 → `ts-verify.sh` → 候補 bundle → `compare` → 人の承認で `canary` → rollback の記録）、
二重実装を整理する終了条件（どの条件で Rust 参照を消すか）を ADR の付記に書く。
受け入れの検証コマンド:
```sh
bash scripts/dev/edit-activate-baseline.sh --scenario cos-ts --mode warm --reps 20 --out agent-docs/progress/<S6 の進捗>/cos-ts.jsonl
bash scripts/dev/edit-activate-baseline.sh --validate agent-docs/progress/<S6 の進捗>/*.jsonl
bash scripts/dev/test-affected.sh && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check
```
報告に書くこと: 旧 Rust 経路と TS 経路の p50/p95（最近傍順位）・試行数・生データの path、型検査・bundle の秒、境界と queue の overhead、RSS・isolate 数、
cold の build 増分、決定一致率・task 成功率・人の介入・LLM 利用量、TS で標準稼働した領域と残る Rust 領域。未達は未達と書く。
S6 の打ち切り基準: TS 経路の warm p50 が 78 秒を超えたら次の領域の移行を止める（ADR (f)）。

### 段の依存
`S0 → S1 → S2 → {S3, S4}（並行可）→ S5 → S6`。S5 と S6 は同じ子 task で、S6 は S5 の受け入れの後に始める。

## 3. 最小 PoC: lane 規則表の当て方
対象: `crates/task-core/src/model_policy.rs` の `ModelPolicy::decide` のうち、`RULES` を上から当てて `(rule_id, lane, condition)` を決める部分。
`LaneCeiling::clamp`（天井）と `decide_for_task` の人の明示（`TierSource::Human`）・System の扱いは Rust に残す（ADR (a)）。

選んだ理由:
1. 純粋: 入力は `TaskFeatures`（`Level` 3 値 × 9 軸、Copy の値）だけで、I/O・時計・乱数が無い。出力は 3 つの文字列（`rule_id`・lane 名・`condition`）。
2. 入出力が小さい: 入力 1 件は 9 欄の JSON、出力は 3 欄。Host API は op を足さずに済み、S1 の最小 Host API で動く。
3. 呼び出し経路が 1 本: `ModelPolicy.decide` の本番の呼び出しは `decide_for_task`（`model_policy.rs` 661 行）だけ（他は `model_policy/tests.rs` の試験）。
   dispatcher からの入口は `dispatch_run.rs` 1379 行と `work_units.rs` 2019 行の 2 か所だが、S2 の `compare` は `dispatch_run.rs` の 1 か所だけに付ける。
4. 効果を測れる: baseline の policy scenario はこの `RULES` の編集で、warm 中央値 221.0 秒（S0 の計測）。S3・S6 で同じ編集課題を TS 経路で測れる。
5. 安全条件と結合しない: 天井と人の明示は Rust が TS の結果の後に当てるので、TS が誤っても lane の天井越えは起きない（起きれば安全違反として数える）。
inventory の S4 第一候補（digest）は文面と cards の組み立てで出力が大きく、`CosTriageItemReport` の直列化の前段変更が要るので、PoC には選ばなかった。

比較 fixture の作り方:
1. 全列挙: 9 軸 × 3 値の全組み合わせ 3^9 = 19,683 件を、Rust の `ModelPolicy.decide(features, &LaneCeiling::default())` に通し、
   1 行 1 件の JSONL `{"features":{…},"rule_id":"…","proposed":"…","condition":"…"}` を `control-ts/fixtures/lane_rules/all-features.jsonl` に書く。
   書き出しは `cargo run -p task-core --example dump-lane-fixtures`（S2 で作る）。行の順は軸の宣言順の辞書順で固定し、決定的にする。
2. 境界: 各規則の条件をちょうど満たす／1 軸だけ外す組を全列挙の中から選び、試験名付きで `control-ts/test/lane_rules.test.ts` に置く。
3. 未知・欠損: 欠けた軸・未知の値（`"extreme"`）・余計な欄の入力 3 件。Rust は `deny_unknown_fields` で拒否し、TS の結果を使わず Rust 参照へ fallback することを確かめる。
4. 比較の正規化: しない（3 欄とも byte 一致を要求する）。`reasons` の文面は Rust が組み立てるので比較の対象外。
5. 天井: `LaneCeiling` を変えた組（cheap・standard の天井）でも、TS は `proposed` だけを返し、`clamp` の後の `lane` が Rust 参照と一致すること。

## 4. S0 の受け入れと証拠
すべて提出時の HEAD で、repo の root で実行する。

| 受け入れ（親 task の S0 節） | 証拠の file | 確かめる command と結果 |
|---|---|---|
| 根拠付きの分類 | [inventory.md](2026-11-02-ts-control-layer-s0/inventory.md) §2（file:関数・変更頻度・逆依存・試験・結合・根拠の列）、§2.4（削除済み機構の除外）、§5（再現の command） | inventory.md §5 の `churn.sh`・`cargo tree -i … -e normal`・`grep -n` を実行して表と照合する。`grep -rn "tier_binding\|TierBinding" crates --include=*.rs \| wc -l` → 0 |
| 再現できる baseline | [baseline.md](2026-11-02-ts-control-layer-s0/baseline.md)（再現手順・環境表・段別の秒）、生データ `baseline/policy.jsonl`・`cos.jsonl`・`cos-link.jsonl`・`cos-affected.jsonl`、script `scripts/dev/edit-activate-baseline.sh` | `bash scripts/dev/edit-activate-baseline.sh --validate agent-docs/progress/2026-11-02-ts-control-layer-s0/baseline/<file>.jsonl` → 各 exit 0（記録時: policy 130 行・cos 132 行・cos-link 59 行・cos-affected 55 行、problems 0）。再測定は `--scenario policy\|cos --mode warm --reps 3` |
| 境界 ADR | [agent-docs/adr/2026-11-02-ts-control-layer-boundary.md](../adr/2026-11-02-ts-control-layer-boundary.md)（(c) Host API 契約、(e) 採用版 `deno_core` =0.413.0・`deno_v8` 0.5.0・`v8` 150.4.0・TypeScript 7.0.2・esbuild 0.28.2、(f) S5 の停止基準の数値） | `bash scripts/dev/check-adr-numbers.sh` → ok。`grep -n "=0.413.0\|停止基準" agent-docs/adr/2026-11-02-ts-control-layer-boundary.md` |
| 後続依存が残る実装計画 | このファイル §2（S1〜S6 の path・検証 command・依存）、§3（PoC） | `for s in S1 S2 S3 S4 S5 S6 PoC; do grep -q "$s" agent-docs/progress/2026-11-02-ts-control-layer-s0.md \|\| echo missing $s; done` → 出力なし。親 task は S1〜S6 の子 task が done になるまで done にしない |

この WorkUnit の検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）:
- `git diff --quiet $(git merge-base HEAD main) -- crates/ web/ Cargo.toml Cargo.lock` → exit 0
- `bash scripts/dev/check-doc-links.sh`・`bash scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → 報告本文に結果を書く
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` → 報告本文に結果を書く

## 5. 未解決事項
1. `v8` 150.4.0 の事前 build archive は未取得。大きさ・sha256・置き場（`/local/celeris/data/scratch/rusty-v8/`）は S1 の最初に人か S1 の run が決める。release gate は外部 network に出ないので、archive が無いと S1 の build が通らない。
2. rustc 1.98.1 での `deno_core` 0.413.0 の build 可否と依存の版衝突は未確認（S1 の `cargo check -p celeris-script`・`cargo tree -d`）。
3. TS の型検査・bundle・vitest の秒は未計測（S3 で測る）。p50 30 秒の目標に収まるかは未確認。
4. daemon の現在の RSS は未計測（S1 で前後を測る）。
5. `test-affected.sh` の cos の計測は `celeris-campaign::ops_r2_socket socket_flag_rejects_non_sockets` の失敗で fail-fast し、実行時間は下限（baseline-cos.md）。失敗が sandbox の中だけかは未確認。
6. sccache が効く状態の cold は未計測。
7. `Math.random`・`Date.now` を入力で置き換えるか throw させるか、`terminate_execution` 後の isolate を再利用するか作り直すかは S1 で試験して決める（ADR (c)・(b)）。
8. `control-ts/` の `node_modules` を pnpm の store から offline で入れられるか（`typescript` 7.0.2・`esbuild` 0.28.2・`vitest` 5.0.0 が store にあるか）は未確認。esbuild は platform 別の native binary を含む。

## 6. 提案
1. TS の置き場 `control-ts/` と配備先 `script-bundles/<bundle_sha256>/` を、S1 で境界 ADR の付記として固定する（この WorkUnit の範囲は進捗ファイルだけなので ADR を変えていない）。
2. S1 の範囲 check には `Cargo.toml`・`Cargo.lock`・`crates/script-runtime/`・`crates/celeris/`・`control-ts/`・`docs/architecture-map.md`・`docs/api/script-host/`・`scripts/dev/test-affected.sh`・境界 ADR を最初から入れる。
3. S2 の `model_policy.rs` の分割（`match_rule` と `decide_with`）は、挙動を変えない Rust だけの commit として TS の追加より先に入れる。S4 の `CosTriageItemReport` の derive も同じ扱いにする。
4. S3 の `edit-activate-baseline.sh` の TS scenario は、S0 の policy・cos と同じ編集課題（`RULES` に当たらない規則を 1 件足す、`job_wait_extend::validate` の 33 行の制御）を TS 側で再現し、比べられる形にする。
