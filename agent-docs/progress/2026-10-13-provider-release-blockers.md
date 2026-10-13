---
title: provider 廃止リリースを本番に入れられる形にする — 不具合 5 件の修正と closeout
tasks: [01M4T94XWN31GW9SENK41GAP8F]
status: done
completed: 2026-10-13（closeout5 で USERNS 無し全体検査・clippy・文書検査 3 本を再確認。USERNS=1 の証拠は `/local/prb/tp-usns-fix5.log`。未解決は下の事項 2（i）・3〜8）
updated: 2026-10-13
---

# provider 廃止リリースを本番に入れられる形にする

親の計画は 3 段（adr → fix → closeout）に fix2 段（legacy-db・h3-userns）を足した 4 段。ADR 付記は `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の「2026-10-13 付記（本番投入の不具合）」。本番 config・DB・daemon は変更していない。

## 結果の要約

- 不具合 5 件（旧 routing event・Pi 宣言・proxy wire 名・OpenCode model ID・class 別 route）はそれぞれ実装 leaf が入り、対応する試験は通った。
- **解消（旧未達 1）: 旧 event 入り DB の起動試験。** fix2 の `legacy-db` で `crates/celeris/tests/legacy_event_startup.rs` の `startup_loads_legacy_routing_decided_event` が入った。一時 SQLite に `record.optimizer.mode = "legacy"` の `routing_decided` 行を入れ、`build_dispatcher` と `tick()` を通し、events を読み直す。前後の行数と payload 文字列が変わらないことも確かめる。
- **解消（旧未達 2）: `CELERIS_USERNS_TESTS=1` の全体検査。** fix2 の `h3-userns` で socket path 長超過（`SUN_LEN` 108 byte）を直した。`test-parallel.sh` の一時 root は継承 `$TMPDIR` の下に作らず `/tmp/celeris-test-parallel.*` を使い、H3 fixture の credentiald socket は `/tmp/h3-*` に置く。今回の全体実行は exit 0（5036 passed、0 failed、`userns: true`）。
- **決定 cos-codex（2026-10-13）: CoS は Claude のみ。** ADR 付記・runbook・進捗 TOML・`migration_runbook_toml_loads` を揃えた。`[cos].route_candidates` は `claude_oauth / claude-opus-5-5` の 1 件で、claude_code から codex_oauth への route 宣言は無い。claude_code を llm-proxy 経由にして Codex fallback を戻す作業は別 task とする。
- 付記の試験名のうち 5 件が実在しなかったので、ADR の Pi 試験名を実在する名前に直した（下の「証拠」）。

## 各 leaf の要点（`2026-10-13-provider-release-blockers/` のメモ）

- `cos-fallback/align-cos.md`: [align-cos leaf メモ](2026-10-13-provider-release-blockers/cos-fallback/align-cos.md) — runbook・進捗 TOML・試験を CoS の Claude 1 候補に揃えた。
- `cos-fallback/adr-note.md`: [adr-note leaf メモ](2026-10-13-provider-release-blockers/cos-fallback/adr-note.md) — 決定 cos-codex と、fallback を戻すための別 task を ADR に記録した。

- `adr`（[adr.md](2026-10-13-provider-release-blockers/adr.md)）: 付記の 5 件の決定と試験名を ADR に書いた。
- `legacy-events`（[legacy-events.md](2026-10-13-provider-release-blockers/legacy-events.md)）: `RoutingMode::Legacy` を deserialize 専用で受ける（serialize は `skip_serializing`）。events は書き換えない。他の provider 削除由来の Event 欄（`WorkerHint.adapter` は未知 field として無視される）は追加互換不要と棚卸し。
- `legacy-db`（[legacy-db.md](2026-10-13-provider-release-blockers/legacy-db.md)）: 旧 event 入り DB の bootstrap・tick の試験（fix2）。
- `h3-userns`（[h3-userns.md](2026-10-13-provider-release-blockers/h3-userns.md)）: socket path 長超過の根因と修正（fix2）。
- `schema-sync`（[schema-sync.md](2026-10-13-provider-release-blockers/schema-sync.md)）: `#[schemars(skip)]` 付き variant が enum を `oneOf[enum]` に変えて `event.schema.json` と `api-v1.schema.json` がずれた。`RoutingMode` の JsonSchema を手書きに替え、`{"type":"string","enum":["shadow","enforce"]}` にした。生成型と schema は一致。
- `pi-harness`（[pi-harness.md](2026-10-13-provider-release-blockers/pi-harness.md)）: `[harnesses.pi]` に `extensions`・`tools`・`env`・`env_from_secrets` を追加。extensions/tools が空なら読み込みエラー。相対 extension path は TOML のあるディレクトリ基準で解決。旧 `pi-*` の値は本番 backup にしかないので、runbook では placeholder にした。
- `model-names`（[model-names.md](2026-10-13-provider-release-blockers/model-names.md)、leaf: `wire-core`・`worker-proxy`・`opencode-ns`・`pi-fixture`）:
  - `wire-core`: daemon の route 選択で `via = "llm_proxy"` の route だけ model を `<source>/<model>` にする（一度だけ作る）。event の canonical model は接頭辞なしのまま。
  - `worker-proxy`: PaperQA の `settings.llm`・`summary_llm`・`agent.agent_llm` を wire 名で上書き、LangMem は `with_model` で route model を使う。LDR は既存の route model 上書きを維持。
  - `opencode-ns`: `[harnesses.opencode].model_namespace` を OpenCode ACP にだけ適用。空値・`/` を含む値は拒否。
  - `pi-fixture`: `celerisctl` の `worker_tests.rs` の Pi fixture に extensions・tools を足した。
- `class-routes`（[class-routes.md](2026-10-13-provider-release-blockers/class-routes.md)、leaf: `core-dispatch`・`config`）: `[route_defaults.<h>].classes.<coding|planner|review>.candidates` を追加。class 列が宣言されていれば harness 既定の代わりに使い、fallback しない。claude_code の既定は `claude-sonnet-5-5`、coding だけ `claude-opus-5-5`。lane・tier・`RoutePolicy` は使わない。

## closeout の変更（前段から）

- `docs/ops/provider-llm-source-migration.md`:
  - 選択結果の表に claude_code の行（sonnet 先頭、coding は opus）と「その他の claude_code 仕事」行を反映。
  - opencode は `model_namespace = "qwen-local"` を宣言し、adapter は `qwen-local/qwen3.8-27b` を渡す、と手順 6 を直した。
  - LDR・PaperQA・LangMem は `via = "llm_proxy"` route の wire 名（`qwen/qwen3.8-27b`）が渡る、と説明を直した。LDR の `settings.llm.model` を `qwen/qwen3.8-27b` に揃えた。
  - 手順 3 に claude_code の先頭 sonnet・coding opus の確認、手順 7 に裸 model の 422 確認を足した。
  - 手順 8 に「旧 `routing_decided`（mode `legacy`）は読み取りだけで残り、移行で書き換えない」を足した。
- `agent-docs/progress/2026-10-11-provider-layer-redesign.md`: `migration_runbook_toml_loads` が runbook と一字一句一致を見るため、marker 間の TOML の写しを runbook に合わせた。
- `agent-docs/adr/2026-10-11-llm-source-harness-selection.md`（fix2 の後の整合）: 付記 §2 の Pi 試験名を実在する名前に直し、未作成の 2 試験を明記した（事項 2）。その後 pi-process で宣言の process 到達試験を足し、未作成は相対 path の 1 件（事項 2（i））だけになった。

## 証拠

最終検査（closeout、2026-10-13、`/local/prb` を TMPDIR にして実行）:

- `mkdir -p /local/prb && CELERIS_USERNS_TESTS=1 TMPDIR=/local/prb bash scripts/dev/test-parallel.sh > /local/prb/tp-usns-cosfb.log 2>&1` — exit 0。nextest `Summary [185.359s] 5036 tests run: 5036 passed (1 slow), 13 skipped`。doc-test は全て ok。`CELERIS_TEST_SUMMARY` は `failed: 0`、`userns: true`（同欄の `passed: 3` は事項 8）。
- `env -u CELERIS_USERNS_TESTS TMPDIR=/local/prb bash scripts/dev/test-parallel.sh > /local/prb/tp-nouserns-cosfb.log 2>&1` — exit 0。nextest `Summary [102.318s] 5036 tests run: 5036 passed, 13 skipped`。doc-test は全て ok。`CELERIS_TEST_SUMMARY` は `passed: 5039`、`failed: 0`、`userns: null`。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished dev profile`、警告なし）。
- 文書検査: `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`, `sh scripts/dev/check-doc-links.sh`, `sh scripts/dev/check-adr-numbers.sh`, `sh scripts/dev/progress-index.sh --check` — 全て exit 0（それぞれ `check-doc-layout: ok`, `check-doc-links: ok`, `check-adr-numbers: ok (198 files)`, `progress-index --check: ok`）。
- 付記の試験名: 付記 §1・§3・§4・§5 の試験名は `grep -rn "fn <name>\b" crates` で全件実在。付記 §2 の Pi 試験名 5 件はすべて実在しなかった。実在する 3 件（別名または近い試験）に置き換え、残り 2 件は試験未作成として ADR に明記した（事項 2）。（閉じた時点の記述。pi-process で (ii) の試験を追加し、未解決は (i) のみ。下の pi-process の証拠を参照）

pi-process（Pi の宣言が process まで届く試験、2026-10-13。`TMPDIR=/local/prb/pp`）:

- 試験の追加: `cargo test -p celeris --lib pi_harness_declaration -- ` — `pi_harness_declaration_deserializes_extensions_tools_and_env`・`pi_harness_declaration_reaches_pi_config`・`pi_harness_declaration_reaches_pi_process` の 3 件が `ok`（`test result: ok. 3 passed; 0 failed`）。
- 製品コードの変更は無し。試験用に `task-worker` の `Task` 最小例を `protocol::sample_task_for_tests()`（`#[doc(hidden)] pub`）へ移し、既存の `tests::sample_task` はそれを呼ぶ薄い wrapper にした。
- `CELERIS_USERNS_TESTS=1 TMPDIR=/local/prb/pp bash scripts/dev/test-parallel.sh` — exit 0。nextest `Summary [248.742s] 5037 tests run: 5037 passed (1 slow), 13 skipped`。`CELERIS_TEST_SUMMARY` は `failed: 0`、`userns: true`（`passed: 3` は事項 8）。
- `TMPDIR=/local/prb/pp bash scripts/dev/test-parallel.sh`（USERNS 無し） — exit 0。nextest `Summary [108.172s] 5037 tests run: 5037 passed, 13 skipped`。`CELERIS_TEST_SUMMARY` は `passed: 5040`、`failed: 0`、`userns: null`（`passed` 欄の差は事項 8 の集計の不具合）。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished dev profile`、警告なし）。
- 文書検査: `check-doc-layout.sh` ok、`check-doc-links.sh` ok、`check-adr-numbers.sh` ok (198 files)、`progress-index.sh --check` ok（すべて exit 0）。

closeout5（2026-10-13、USERNS 無しの全体検査と clippy。コードは変えていない。TMPDIR は `/local/prb/t.*` の mktemp）:

- `d=$(mktemp -d /local/prb/t.XXXX) && TMPDIR=$d bash scripts/dev/test-parallel.sh > /local/prb/tp-nouserns-fix5.log 2>&1` — exit 0。log は `/local/prb/tp-nouserns-fix5.log`。nextest 結果は `5040` passed（`CELERIS_TEST_SUMMARY` の `passed: 5040`）、`failed: 0`、`ignored: 14`、`nextest_exit: 0`、`doctest_exit: 0`、`userns: null`（USERNS 無しの実行なので想定どおり）。doc-test は全て ok。
- 注記: 要約行の `tmp_leftovers: 6` は TMPDIR に残った 6 件の警告（`test-parallel: warning`）で、スクリプトが終了時に消した。exit code には影響しない。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished dev profile`、警告なし）。log は `/local/prb/clippy-closeout5.log`。
- 文書検査: `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` — 全て exit 0。
- USERNS=1 の結果は上の「USERNS=1 全体検査の証拠」節（`/local/prb/tp-usns-fix5.log`、exit 0、`"userns": true`）のとおり。この節は書き換えていない。

前段（fix2 の前、closeout の再実行）の結果は、旧 event 試験が無い・`browser_h3_injection` が 503 で落ちた状態。この WU の最終検査はそれを上書きする。

## USERNS=1 全体検査の証拠（reviewer の環境では再現不可）

### 条件 3 の解消（ADR 本文の CoS 記述）

cos-adr（葉メモ [cos-adr.md](2026-10-13-provider-release-blockers/cos-adr.md)、commit ac0db44d）で、決定 cos-codex に合わせて ADR `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の本文を書き換えた。付記の文を残して注記する形はとらず、本文そのものを直した。

- 「## CoS の経路」: Codex fallback の文を消し、CoS の候補を `claude_oauth / claude-opus-5-5` の 1 件にした。TOML 例の `route_candidates` も 1 件にした。
- 「実装突き合わせ付記（2026-10-12、本番移行 runbook）」: CoS の文を「standard の `claude-opus-5-5` の 1 件だけ（Codex へは落とさない）」に直した。
- 同じ付記の検証の記述: 「CoS の候補順」を「CoS の候補（Claude の 1 件）」に直した。
- 「2026-10-13 付記（CoS の Codex fallback 撤去 — 決定 cos-codex）」§1: 旧文の引用を消した。

### 環境の制約

final review の sandbox では `unshare -Ur` が Operation not permitted になり、test-parallel は userns: false を報告する。その結果、userns 試験 34 件が落ちる。これは環境の制約で、コードの不具合ではない。ADR-0126 のとおり `CELERIS_USERNS_TESTS=1` は userns が作れる環境が前提。

### USERNS=1 の全体検査（この葉の check）

- 実行: `mkdir -p /local/prb && d=$(mktemp -d /local/prb/t.XXXX) && CELERIS_USERNS_TESTS=1 TMPDIR=$d bash scripts/dev/test-parallel.sh > /local/prb/tp-usns-fix5.log 2>&1`
- 結果: exit 0。log は `/local/prb/tp-usns-fix5.log`（330 行）。`unshare -Ur true` が成功する host で実行した（userns 作成可）。
- log の nextest 行（引用）:
  `Summary [ 203.731s] 5037 tests run: 5037 passed (1 slow), 13 skipped`
- log の要約行（引用）:
  `CELERIS_TEST_SUMMARY {"runner": "nextest", "nextest_version": "0.9.146", "jobs": 8, "binaries": 178, "nextest_binaries": 168, "doc_binaries": 10, "passed": 3, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "nextest_secs": 350.6, "doctest_secs": 9.6, "tmp_leftovers": 4, "summary_parsed": true, "userns": true}`
- 判定: `failed: 0`、`"userns": true`、nextest の failed 0。doc-test は `all doctests ran`、exit 0。
- 注記: `passed: 3` は要約行の集計の不具合（未解決事項 8）。nextest の Summary の 5037 passed が正。`tmp_leftovers: 4` は TMPDIR に残った 4 件の警告で、スクリプトが終了時に消した。exit code には影響しない。log に `FAIL`・`panicked`・`TIMEOUT` は無い。

### 人の決定 userns-gate（回答: check-log）

葉の check を条件 4 の証拠とする。reviewer は sandbox での再実行の失敗では判定しない。log に `"userns": true` が無いか、失敗が 1 件でもあれば不合格。release gate でも同じ試験が流れる。

## 未解決事項

1. **CoS の Codex fallback — 決定・対応済み。** `cos-codex` は (a) Claude のみ。ADR 付記・runbook・進捗 TOML・`migration_runbook_toml_loads` を一致させ、CoS route は `claude_oauth / claude-opus-5-5` の 1 件にした。詳細は [align-cos](2026-10-13-provider-release-blockers/cos-fallback/align-cos.md) と [adr-note](2026-10-13-provider-release-blockers/cos-fallback/adr-note.md)。
2. **Pi の試験 2 件のうち 1 件は未解決。** (i) 未解決: `[harnesses.pi].extensions` の相対 path が TOML 基準で解決されることを名前付きで確かめる試験（`config/mod.rs` の解決処理は通るが、試験は無い）。次の WU の範囲。(ii) 解消（pi-process）: 宣言 `extensions`・`tools`・`env`・`env_from_secrets` が PiAdapter の起動する process まで届くことを `celeris::daemon::adapters::harness_adapter_tests::pi_harness_declaration_reaches_pi_process` で確かめる。stub の pi が argv（1 行 1 引数）と `env` の出力を一時 file に書き、試験がそれを検査する。`pi_config` の試験（PiConfig まで）は残す。
3. **proxy_model_name 試験の件数。** `cargo test -p celeris proxy_model_name` は 0 件実行（celeris crate に該当試験が無い）。受け入れ条件に使うなら、`-p task-dispatch` と `-p task-worker` に寄せるか、celeris に 1 件足す。
4. **coding class は opus 1 件だけ。** opus が使えないと sonnet へは落ちず、claude_code が飛ばされて Codex へ行く。sonnet を 2 番目に入れるかは人が決める。
5. **daemon の `GET /api/v1/harnesses` は class 候補を出さない。** runbook 手順 6 の jq では coding class は見えないので、確認は `routing show`（手順 3）で行う。
6. **Pi の値は本番 backup からの転記が要る。** Hashline extension path と tool 名（read/edit）、env・env_from_secrets は、runbook では placeholder。checkout に旧 `pi-*` の実値は無かった。placeholder のまま `routing show` を通すと Pi 宣言の値が誤るので、転記してから検査する。
7. **前回の全体検査で出た `e2e::worker_db_read_only` の失敗は原因未確認。** `h3-userns.md` によれば、修正前の USERNS 全体実行（5001 passed、34 failed）でこの試験が落ち、出力は `run_db=/local/celeris/data/db/celeris.sqlite3` と `raw_db_write_exit=0` / `raw_wal_write_exit=0` を示した。本 WU の 2 回の全体実行では落ちなかったが、ログは summary 行だけで個別の結果を確かめられない。live DB への書き込みが通ったように読める出力なので、試験の出力を単独で再現して確かめる必要がある。
8. **`CELERIS_TEST_SUMMARY` の `passed` 欄が USERNS 版で `3` になる。** 集計の解析が途中で止まる不具合で、`scripts/dev/test-parallel.sh` の要約行の解析を直す作業。検査の判定は nextest の Summary 行を使う。

## 提案

- 事項 2 の試験を足す（相対 path と宣言 env の run 到達）。旧 event 試験と同じく、本番に近い宣言を使う。
- 事項 7 は `e2e::worker_db_read_only` を単独で流し、出力と DB の書き込み可否を確かめてから全体検査に戻す。
- claude_code を llm-proxy 経由にして CoS の Codex fallback を戻す作業は別 task とする。
- class 別 route の選択理由（どの候補が選ばれたか）を `routing_decided` の metadata に入れると、本番確認が DB だけで済む。

## 本番への移行

本番の config・DB・daemon・release は人が実行する。次の順で行う（runbook の「切替前の退避と確認」〜「読み込み・切替・確認」に従う）。

1. **config に入れる値（移行後 TOML の変更点）**:
   - `[harnesses.pi]`: `extensions` に旧 `pi-*` の Hashline extension path、`tools` に Hashline の read/edit 名と `bash`・`grep`・`find`・`ls`、旧 provider の env を `env`、secret は `env_from_secrets` に転記する。placeholder は実値に置換する（事項 6）。
   - `[harnesses.opencode].model_namespace = "qwen-local"`: OpenCode config の provider ID と一致することを確かめる。
   - `[route_defaults.claude_code]`: 先頭は `claude-sonnet-5-5`、`[route_defaults.claude_code.classes.coding]` は `claude-opus-5-5`（`candidates` は `[route_defaults.<h>].classes.<coding|planner|review>.candidates` で宣言する。class 宣言があると harness 既定の代わりに使われ、fallback しない）。lane・tier は使わない。
   - `[adapters.local_deep_research].settings."llm.model"` と `[knowledge.langmem].model`: `qwen/qwen3.8-27b`。`[knowledge.langmem].base_url` は llm-proxy を指す。
   - [cos] route_candidates は claude_oauth / claude-opus-5-5 の 1 件。[[llm_routes]] claude_code←codex_oauth は入れない。
2. **staging で確かめる順（人が実行）**:
   - 作業用コピーの config を新リリースの `celerisctl routing show --config …` で検査（exit 0）。`claude_code:` の先頭が sonnet、`claude_code class coding:` が opus だけ、`pi` の候補が `opencode_go/opencode-go/grok-4.6 > qwen/qwen3.8-27b`。
   - `CELERIS_CONFIG=<作業用コピー> scripts/selfdeploy/verify.sh <新リリース>` を流す。本番の旧 event を含む DB のコピーを使うと、本番での落ち方を確かめられる（試験は一時 DB で行うので、これは追加の確認）。
   - 昇格後、`journalctl --user -u 'celeris@<新リリース>.service'` に設定エラーが無いこと。
   - proxy: 接頭辞付き `qwen/qwen3.8-27b` が 200（`x-celeris-source: openai-compatible:qwen`）、裸の `qwen3.8-27b` が 422。
   - 低リスクな coding task・Pi task・`local-qwen` task・CoS を 1 件ずつ流し、`routing_decided` の harness・source・model を選択結果の表と照合する。
3. **戻し方**: runbook の rollback に従い、release と config を対で戻す。旧 event は読み取りのみなので、戻しても書き換えは発生しない。
