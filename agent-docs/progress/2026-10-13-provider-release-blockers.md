---
title: provider 廃止リリースを本番に入れられる形にする — 不具合 5 件の修正と closeout
tasks: [01M4T94XWN31GW9SENK41GAP8F]
status: done
completed: 2026-10-13（受け入れ条件 0・1 を満たす。未達は無し。未解決は下の事項 1〜8。うち人の決定は 1 件）
updated: 2026-10-13
---

# provider 廃止リリースを本番に入れられる形にする

親の計画は 3 段（adr → fix → closeout）に fix2 段（legacy-db・h3-userns）を足した 4 段。ADR 付記は `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の「2026-10-13 付記（本番投入の不具合）」。本番 config・DB・daemon は変更していない。

## 結果の要約

- 不具合 5 件（旧 routing event・Pi 宣言・proxy wire 名・OpenCode model ID・class 別 route）はそれぞれ実装 leaf が入り、対応する試験は通った。
- **解消（旧未達 1）: 旧 event 入り DB の起動試験。** fix2 の `legacy-db` で `crates/celeris/tests/legacy_event_startup.rs` の `startup_loads_legacy_routing_decided_event` が入った。一時 SQLite に `record.optimizer.mode = "legacy"` の `routing_decided` 行を入れ、`build_dispatcher` と `tick()` を通し、events を読み直す。前後の行数と payload 文字列が変わらないことも確かめる。
- **解消（旧未達 2）: `CELERIS_USERNS_TESTS=1` の全体検査。** fix2 の `h3-userns` で socket path 長超過（`SUN_LEN` 108 byte）を直した。`test-parallel.sh` の一時 root は継承 `$TMPDIR` の下に作らず `/tmp/celeris-test-parallel.*` を使い、H3 fixture の credentiald socket は `/tmp/h3-*` に置く。今回の全体実行は exit 0（5036 passed、0 failed、`userns: true`）。
- **人の判断が要る 1 件: CoS の Codex fallback。** 2026-10-13 付記 §3 は「claude_code は proxy endpoint を宣言できないので Codex OAuth proxy fallback を外し、CoS は Claude source のみ」とする。一方、runbook と試験 `migration_runbook_toml_loads` は 2026-10-12 付記の CoS `codex_oauth / gpt-6-astra` を残している。この食い違いは付記と runbook のどちらを正とするかの決定で、本 WU では選ばず、runbook も試験も変えていない。下の未解決事項 1 を参照。
- 付記の試験名のうち 5 件が実在しなかったので、ADR の Pi 試験名を実在する名前に直した（下の「証拠」）。

## 各 leaf の要点（`2026-10-13-provider-release-blockers/` のメモ）

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
- `agent-docs/adr/2026-10-11-llm-source-harness-selection.md`（fix2 の後の整合）: 付記 §2 の Pi 試験名を実在する名前に直し、未作成の 2 試験を明記した（事項 2）。

## 証拠

最終検査（closeout2 の WU、2026-10-13、`/local/prb` を TMPDIR にして実行。`unshare -Ur true` は exit 0）:

- 条件 1: `CELERIS_USERNS_TESTS=1 TMPDIR=/local/prb bash scripts/dev/test-parallel.sh` — exit 0。nextest `Summary [179.393s] 5036 tests run: 5036 passed (1 slow), 13 skipped`。doc-test も ok（`test result: ok` が全 crate）。`CELERIS_TEST_SUMMARY` の `userns: true`、`failed: 0`。ログ: `/local/prb/tp-usns-closeout2.log`。
- 条件 1: `env -u CELERIS_USERNS_TESTS TMPDIR=/local/prb bash scripts/dev/test-parallel.sh` — exit 0。nextest `Summary [100.857s] 5036 tests run: 5036 passed, 13 skipped`。`CELERIS_TEST_SUMMARY` の `userns: null`、`failed: 0`。ログ: `/local/prb/tp-nouserns-closeout2.log`。
- `CELERIS_TEST_SUMMARY` の `passed` は USERNS 版で `3`、無し版で `5039` と食い違う。USERNS 版は解析が途中で止まった表示で、正は nextest の Summary 行（5036）。下の事項 8。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished dev profile`、警告なし）。ログ: `/local/prb/clippy-closeout2.log`。
- 文書検査: `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — exit 0（`check-doc-layout: ok`）。`sh scripts/dev/check-doc-links.sh` — exit 0（`check-doc-links: ok`）。`sh scripts/dev/check-adr-numbers.sh` — exit 0（`check-adr-numbers: ok (198 files)`）。
- 付記の試験名: 付記 §1・§3・§4・§5 の試験名は `grep -rn "fn <name>\b" crates` で全件実在。付記 §2 の Pi 試験名 5 件はすべて実在しなかった。実在する 3 件（別名または近い試験）に置き換え、残り 2 件は試験未作成として ADR に明記した（事項 2）。

前段（fix2 の前、closeout の再実行）の結果は、旧 event 試験が無い・`browser_h3_injection` が 503 で落ちた状態。この WU の最終検査はそれを上書きする。

## 未解決事項

1. **CoS の Codex fallback の決定（人）。** 選択肢: (a) 2026-10-13 付記どおり Claude のみにする → runbook の `[cos].route_candidates` と `[[llm_routes]] claude_code←codex_oauth` を消し、試験 `migration_runbook_toml_loads` の CoS 検査を直す。(b) 2026-10-12 のまま Codex fallback を残す → 付記 §3 の「CoS は Claude source のみ」を改める。推奨は (a)（付記のほうが新しく、proxy 経由の claude_code は実装されていないため）。どちらにも本 WU は手を付けていない。
2. **Pi の未作成試験 2 件。** (i) `[harnesses.pi].extensions` の相対 path が TOML 基準で解決されることを名前付きで確かめる試験（`config/mod.rs` の解決処理は通るが、試験は無い）。(ii) 宣言 `env` と `env_from_secrets` が Pi の run process まで届くことを確かめる試験（`celeris::daemon::adapters::tests::pi_harness_declaration_reaches_pi_config` は PiConfig までしか確かめない）。どちらも既存の実装は変えずに試験を足す作業で、次の WU の範囲。
3. **proxy_model_name 試験の件数。** `cargo test -p celeris proxy_model_name` は 0 件実行（celeris crate に該当試験が無い）。受け入れ条件に使うなら、`-p task-dispatch` と `-p task-worker` に寄せるか、celeris に 1 件足す。
4. **coding class は opus 1 件だけ。** opus が使えないと sonnet へは落ちず、claude_code が飛ばされて Codex へ行く。sonnet を 2 番目に入れるかは人が決める。
5. **daemon の `GET /api/v1/harnesses` は class 候補を出さない。** runbook 手順 6 の jq では coding class は見えないので、確認は `routing show`（手順 3）で行う。
6. **Pi の値は本番 backup からの転記が要る。** Hashline extension path と tool 名（read/edit）、env・env_from_secrets は、runbook では placeholder。checkout に旧 `pi-*` の実値は無かった。placeholder のまま `routing show` を通すと Pi 宣言の値が誤るので、転記してから検査する。
7. **前回の全体検査で出た `e2e::worker_db_read_only` の失敗は原因未確認。** `h3-userns.md` によれば、修正前の USERNS 全体実行（5001 passed、34 failed）でこの試験が落ち、出力は `run_db=/local/celeris/data/db/celeris.sqlite3` と `raw_db_write_exit=0` / `raw_wal_write_exit=0` を示した。本 WU の 2 回の全体実行では落ちなかったが、ログは summary 行だけで個別の結果を確かめられない。live DB への書き込みが通ったように読める出力なので、試験の出力を単独で再現して確かめる必要がある。
8. **`CELERIS_TEST_SUMMARY` の `passed` 欄が USERNS 版で `3` になる。** 集計の解析が途中で止まる不具合で、`scripts/dev/test-parallel.sh` の要約行の解析を直す作業。検査の判定は nextest の Summary 行を使う。

## 提案

- 事項 2 の試験を足す（相対 path と宣言 env の run 到達）。旧 event 試験と同じく、本番に近い宣言を使う。
- 事項 7 は `e2e::worker_db_read_only` を単独で流し、出力と DB の書き込み可否を確かめてから全体検査に戻す。
- CoS の決定（事項 1）を先に人が下し、runbook・試験・付記を一緒に直す。
- class 別 route の選択理由（どの候補が選ばれたか）を `routing_decided` の metadata に入れると、本番確認が DB だけで済む。

## 本番への移行

本番の config・DB・daemon・release は人が実行する。次の順で行う（runbook の「切替前の退避と確認」〜「読み込み・切替・確認」に従う）。

1. **config に入れる値（移行後 TOML の変更点）**:
   - `[harnesses.pi]`: `extensions` に旧 `pi-*` の Hashline extension path、`tools` に Hashline の read/edit 名と `bash`・`grep`・`find`・`ls`、旧 provider の env を `env`、secret は `env_from_secrets` に転記する。placeholder は実値に置換する（事項 6）。
   - `[harnesses.opencode].model_namespace = "qwen-local"`: OpenCode config の provider ID と一致することを確かめる。
   - `[route_defaults.claude_code]`: 先頭は `claude-sonnet-5-5`、`[route_defaults.claude_code.classes.coding]` は `claude-opus-5-5`（`candidates` は `[route_defaults.<h>].classes.<coding|planner|review>.candidates` で宣言する。class 宣言があると harness 既定の代わりに使われ、fallback しない）。lane・tier は使わない。
   - `[adapters.local_deep_research].settings."llm.model"` と `[knowledge.langmem].model`: `qwen/qwen3.8-27b`。`[knowledge.langmem].base_url` は llm-proxy を指す。
   - CoS: 事項 1 の決定に従う。
2. **staging で確かめる順（人が実行）**:
   - 作業用コピーの config を新リリースの `celerisctl routing show --config …` で検査（exit 0）。`claude_code:` の先頭が sonnet、`claude_code class coding:` が opus だけ、`pi` の候補が `opencode_go/opencode-go/grok-4.6 > qwen/qwen3.8-27b`。
   - `CELERIS_CONFIG=<作業用コピー> scripts/selfdeploy/verify.sh <新リリース>` を流す。本番の旧 event を含む DB のコピーを使うと、本番での落ち方を確かめられる（試験は一時 DB で行うので、これは追加の確認）。
   - 昇格後、`journalctl --user -u 'celeris@<新リリース>.service'` に設定エラーが無いこと。
   - proxy: 接頭辞付き `qwen/qwen3.8-27b` が 200（`x-celeris-source: openai-compatible:qwen`）、裸の `qwen3.8-27b` が 422。
   - 低リスクな coding task・Pi task・`local-qwen` task・CoS を 1 件ずつ流し、`routing_decided` の harness・source・model を選択結果の表と照合する。
3. **戻し方**: runbook の rollback に従い、release と config を対で戻す。旧 event は読み取りのみなので、戻しても書き換えは発生しない。
