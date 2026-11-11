---
title: 健全性v2 R2 unit adapter-settings — WorkerAdapter の with_* 5 つを configure 1 つにする
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R2 unit adapter-settings — WorkerAdapter の with_* を configure 1 つにする

完了日: 2026-11-10

task 01M6TYVJT0Z1CBGNK8VTNK8ZJH の WorkUnit adapter-settings の記録。task のまとめは [R2 の進捗](../2026-11-10-health-v2-r2.md)、設計は [worker capability ports ADR](../../adr/2026-11-10-worker-capability-ports.md) の D3（以下 ADR）。基準は `$CELERIS_WU_BASE`。

## 変更

commit（件名で記す）:

- `phase 1: fold WorkerAdapter with_* into configure(AdapterSettings)`（実装と試験の移行）
- `phase 2: record adapter-settings evidence`（`AdapterSetting` の使われない `as_str`・`Display` の削除と、この記録）

内容:

- `WorkerAdapter` の `with_model`・`with_llm_endpoint`・`with_permission_mode`・`with_env`・`with_container` を消し、`fn configure(&self, &AdapterSettings) -> Configured` 1 つにした（`crates/task-worker/src/adapter.rs`）。既定実装は何も受けず、指定された項目を全部 `unsupported` に入れる（許可側に倒れない）。
- 新しい型（`adapter.rs`、`lib.rs` で再公開）: `AdapterSettings`（5 欄。空の欄は「変えない」）、`LlmEndpoint`、`AdapterSetting`（項目の enum）、`Configured`（`adapter: Option<複製>` と `unsupported`）。`Configured::resolve` は複製が無ければ base を返す。
- 9 adapter（acp・aider・claude_code・codex・fake・langmem・local_deep_research・paperqa・pi）の写し 26 個を `configure` 1 つずつにした。受ける項目は ADR D3 3a の表のとおりで、各 adapter は受ける項目の配列を `AdapterSettings::configured` に渡すだけにした。「受けない項目を報告し、受けた項目があれば複製する」判定はこの helper 1 か所にある。pi は env の後に endpoint の鍵を置き直す（旧 `with_env` → `with_llm_endpoint` の順と同じ）。
- `BrowserSpecialistAdapter` の転送 5 個を `configure` の転送 1 個にした（内側の `configure` の複製を包み直し、`unsupported` はそのまま返す）。
- 呼び出し側（ADR D3 3b）:

| 呼び出し側 | 変更後 | 受けられないとき（今と同じ） |
|---|---|---|
| `task-worker/src/route.rs` `route_adapter` | model・account の env・endpoint を 1 回の `configure` | model は無視。`Env` → `AccountUnsupported`（`LlmEndpoint` より先）、`LlmEndpoint` → `EndpointUnsupported`。`CredentialUnresolved` は `configure` の前 |
| `dispatcher/route_launch.rs` `route_adapter`・`route_failure.rs` | model と account の env を 1 回の `configure`。`adapter_for_account` は env の組を返す `account_env` にした | model は無視。`[accounts]` の root が無い・`Env` が unsupported → `AccountUnavailable` |
| `dispatcher/worker_task.rs` `prepare_run_adapter` | env を cargo → followups → WU → run の tmp の順に 1 度組み立て、container・permission mode と 1 回の `configure`。4 つの match は消えた | `Env` → 適用しなかった env の種類を並べた debug log 1 行、`Container` → 今と同じ warn、`PermissionMode` → debug log。`env_applied` = cargo の env があり `Env` が unsupported でない |
| `dispatcher/housekeeping.rs`（reviewer の cargo env） | `AdapterSettings::env_only` で 1 回 | rust の repo なら warn、続行 |
| `review.rs` `with_run_tmpdir` | 同上 | warn、TMPDIR なしで続行 |
| `dispatcher/eval_trials.rs` | 同上 | trial を失敗にして error |
| `dispatcher/cos_chat/launch.rs` | 同上。`launch_claimed` から `adapter_with_run_env` に出した | credential を失効させて error |
| `crates/celeris/src/daemon/adapters.rs`（試験） | `configure(..).resolve(..)` と `unsupported` が空の assert | — |

- 試験の adapter 22 個の with_* 31 個を `configure` に移した（task-worker の `route/tests.rs`、task-dispatch の dispatcher/tests の 14 file と `cos_chat/harness_tests.rs`・`review/tests.rs`、celeris の `tests/trial_context_canary_e2e.rs`）。内側を包む試験の adapter 4 個は、`BrowserSpecialistAdapter` と同じ `Configured::map_adapter` を使う。
- task-worker の試験の呼び出し（acp・aider・claude_code・codex・langmem・local_deep_research・paperqa の tests.rs、`browser_tests.rs`）を `configure` に移した。試験の名前は変えていない。
- `docs/architecture-map.md` の「Adapter 選択」行に `configure`・呼び出し側 2 つ・`settings_tests`・不変条件を足した（ADR D4 4b）。
- with_* の名前を指していた doc comment を `configure` の語に直した（task-worker の `claude_code.rs`・`paperqa.rs` と adapter の試験、task-dispatch の `review.rs`・`route_failure.rs` と dispatcher の試験）。

## 実装前 brief

1. 根本原因: 設定の 5 項目ごとに trait method があり、9 adapter と browser-specialist が同じ「config を複製して欄を書き換える」形を項目ごとに写していた。呼び出し側は項目ごとに「受けられないとき」の match を繰り返した（`prepare_run_adapter` は `with_env` の match を 4 回）。
2. 責務 owner: `task-worker::adapter`（`WorkerAdapter`）。受ける項目の宣言は各 adapter、unsupported の扱いは各呼び出し側。
3. 既存の再利用先: 各 adapter の config の欄（`model`・`env`・`container`・`permission_mode`・`base_url`）、既存の `with_env_overrides_…`・`worker_route_…`・dispatcher の試験。
4. 選択肢: (a) ADR D3 の `configure` 1 つと `Configured`（採用。写しと呼び出し側の match が 1 つになる）。(b) with_* を残して既定の「全部 None」をまとめる helper だけ足す（写しが残り、objective を満たさない）。
5. 新しい状態・分岐・fallback・port: 無し。型 `AdapterSettings`・`Configured` は値の受け渡しだけで状態を持たない。新旧切替の flag・互換の別名は置かない。
6. 撤去対象: `WorkerAdapter` の with_* 5 つ、9 adapter の写し 26 個、browser-specialist の転送 5 個と `wrap`、試験の adapter の写し 31 個、`prepare_run_adapter` の match 4 つ、`adapter_for_account`、with_* を指す doc comment。
7. 不変条件: LLM endpoint・account の env を受けない harness の route は fail closed。env を受けない adapter は log して続行（呼び出し側ごとに今と同じ）。browser-specialist の転送は内側の `unsupported` を保ち id を変えない。試験は下の「試験の契約」。

## 試験の契約

新規 5 件（名前は `adapter_settings_`。task-worker の lib 試験）。既存の試験は adapter ごとに「受ける」側（`with_env_overrides_…`・`…_route_model…`）だけを見ており、受けない側の報告・既定実装・browser-specialist の転送・route の `AccountUnsupported` を見る試験は無かった。

| 試験 | 守る不変条件 |
|---|---|
| `adapter.rs` `adapter_settings_default_configure_reports_every_requested_item` | 既定の `configure` は複製を作らず、指定した 5 項目を全部 `unsupported` に入れる。何も指定しなければ空。`resolve` は base を返す |
| `adapter.rs` `adapter_settings_each_harness_reports_what_it_cannot_take` | 9 adapter の `unsupported` が ADR D3 3a の表と一致し、受けた項目があれば同じ id の複製が返る。受けない項目だけを指定すると複製を作らない |
| `adapter.rs` `adapter_settings_browser_specialist_forwards_and_keeps_identity` | browser-specialist は内側の `unsupported` をそのまま返し、複製の id は `browser-specialist`。run は設定を受けた内側へ届き、env は順に重なる（同名は後勝ち）。内側が何も受けなければ複製を作らない |
| `adapter.rs` `adapter_settings_env_is_appended_in_order_and_empty_fields_change_nothing` | env は並べた順に末尾へ足す。空の欄（model・container）は既存の値を変えない |
| `route/tests.rs` `adapter_settings_route_account_env_fails_closed_before_endpoint` | model を受けない harness は base のまま。account の env を受けない harness は `AccountUnsupported`、endpoint もあるときも `AccountUnsupported` が先。account が無ければ `EndpointUnsupported` |

既存試験は削除 0 件。`$CELERIS_WU_BASE` から HEAD までに変わった .rs 47 file の `#[test]`・`#[tokio::test]` の fn 名の集合を比べ、削除 0 件・追加はこの 5 件だけだった（`$TMPDIR` の一時台本で `git show <rev>:<file>` を読んで比較）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

この WorkUnit は main の取り込み・rebase をしていない。celeris が review 前に同期した場合は、同期後の HEAD で同じ検査を再実行する。

| 条件 | 実行したコマンド | 出力の要点 |
|---|---|---|
| with_* の撤去 | `python3 agent-docs/progress/2026-11-10-health-v2-r2/measure.py --check-adapter-settings` | exit 0、`check-adapter-settings: ok` |
| Browser の分離を壊していない | `python3 agent-docs/progress/2026-11-10-health-v2-r2/measure.py --check-browser-split` | exit 0、`check-browser-split: ok` |
| 新規試験 | `cargo test -p task-worker --lib adapter_settings_` | exit 0、5 passed、1194 filtered out |
| 影響試験 | `bash scripts/dev/test-affected.sh` | exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker の 7 crate。nextest 5867 件すべて合格・22 skipped。doc test 合格。冒頭の health は verdict=pass、new=0・worsened=0・fail_grade=0。script bundle の SKIP 0 件 |
| lint | `cargo clippy --workspace -- -D warnings` | exit 0 |
| format | `cargo fmt --all -- --check` | exit 0 |
| 健全性 | `bash scripts/dev/check-health.sh` | exit 0、verdict=pass、new=0・worsened=0 |
| owner 地図 | `python3 scripts/dev/check-architecture-map.py` | exit 0、993 件の path を確認 |
| 範囲 | `sh "$CELERIS_WU_SCOPE_PATHS"` | 変えた path は ADR D4 4a の adapter-settings の path、4a の共有 path（`worker_task.rs`・`lib.rs`）、4b の `docs/architecture-map.md`、B0 の write_set の内（`adapter.rs`・`browser_specialist.rs`・`browser_tests.rs`・この file）だけ |

計測値（`measure.py --json`、基準値は [baseline.json](baseline.json)）:

| 項目 | 基準 | HEAD |
|---|---|---|
| `WorkerAdapter` の with_* | 5 | 0（`configure` 1 つ） |
| 非試験の adapter の with_* fn | 31（9 adapter の 26 と browser-specialist の 5） | 0（`configure` は 10 impl） |
| 試験の adapter の with_* fn | 31 fn / 22 impl | 0 |
| browser-specialist の設定の転送 | 5 | 1 |
| `worker_task.rs` の `.with_env(` | 4 | 0 |
| 非試験の with_* の呼び出し | 20 | 0 |

## 判断

- `AdapterSettings` に ADR に無い helper を足した。`configured`（受ける項目の配列から `unsupported` と複製の有無を決める判定。ADR 3a の「受ける項目の集合を渡すと残りを返す helper」）、`apply_model`・`apply_env`・`apply_container`（空の欄を変えない規則と env の後勝ちの規則を 1 か所に置く）、`env_only`（env だけを渡す呼び出し側 4 か所と試験 1 か所が同じ形の struct literal を書かないため）。`Configured::map_adapter` は包む adapter の転送の形（browser-specialist と試験の 4 個）を 1 つにする。
- `cos_chat/launch.rs` の `launch_claimed` から、run credential の env を渡して受けられなければ失効させる処理を `adapter_with_run_env` に出した。`launch_claimed` は `size.responsibility-growth` の閾値（200 行）を既に超えており、`let … else` を `configure` と `unsupported` の判定に置き換えると 5 行増えて worsened になったため。`env_only` を使って dispatcher の module の本体の行の合計も基準値（35120）を超えないようにした。どちらも試験を外す・閾値を変える形ではない。
- `prepare_run_adapter` の env を受けない adapter の log は、4 つの debug log を 1 行にまとめ、適用しなかった env の種類（`CARGO_TARGET_DIR`・`CELERIS_FOLLOWUPS_FILE`・`CELERIS_WU_BASE / CELERIS_WU_TARGET`・`the run TMPDIR`）を並べる。env の対応は adapter 単位（全部受けるか全部受けないか）なので、適用の結果は 4 回の `with_env` と同じ。
- 文言の変更: warn と WorkerProgress の note の「does not support with_env」を「does not take env」にした。note を読む側（`dispatcher/tests/target_env.rs`）は接頭辞 `cargo-target:` だけを見るので影響しない。過去の event の文言は変えない（追記専用）。
- 挙動の差（実害なしと判断）: 旧 `with_env(&[])` は空の env でも複製を返したが、`configure` は空の env を「指定なし」とみなす。空の env を渡す呼び出し側は無い（housekeeping は空なら先に返る、cargo の env は常に `CARGO_TARGET_DIR` を含む、CoS の env は credential を含む）。

## ADR との食い違い（record unit が付記に使う）

- ADR D3 3a の `AdapterSettings`・`Configured` に、上の「判断」の helper（`configured`・`apply_*`・`env_only`・`map_adapter`）を足した。`AdapterSetting` は `Hash` も derive する。
- ADR D3 3b の route_launch の行「隣り合う 2 回の適用は 1 回の `configure` にまとめてよい」に従い、`route_failure.rs` の `adapter_for_account` を env の組を返す `account_env` に変え、`route_adapter` が 1 回の `configure` で model と一緒に重ねる。
- ADR D3 3b の cos_chat の行は `launch_claimed` の中の変更を想定していたが、上の理由で `adapter_with_run_env` に出した。

## 未解決

- `crates/task-dispatch/src/dispatcher.rs` の `planner_permission_mode` の doc comment（「`adapter.with_permission_mode` で実際の CLI 引数を上書きする」）と `crates/task-dispatch/src/dispatcher/cos_chat/launch/tests.rs` の 1 行目の doc comment（「passed to `with_env` at launch」）が、消した method の名前を指したまま。どちらも ADR D4 の path の外なので変えていない。提案: unit dup-cleanup か record で doc comment だけを `configure` の語に直す（コードの変更は無い）。
- userns・実 browser の試験は触っていない（task-api の browser の試験は変更の対象外）。

## 本番への移行

無し（config の欄・API・DB は変えていない）。
