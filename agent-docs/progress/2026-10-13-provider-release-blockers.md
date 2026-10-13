---
title: provider 廃止リリースを本番に入れられる形にする — 不具合 5 件の修正と closeout
tasks: [01M4T94XWN31GW9SENK41GAP8F]
status: blocked
completed: 未完了（受け入れ条件 0 が未達: 旧 event 入り DB の起動試験が無い。受け入れ条件 1 は USERNS 無しの全体検査が通るが、USERNS=1 の全体検査で browser_h3 試験 1 件が落ちる。下の未解決事項を参照）
updated: 2026-10-13
---

# provider 廃止リリースを本番に入れられる形にする

親の計画は 3 段（adr → fix → closeout）。ADR 付記は `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の「2026-10-13 付記（本番投入の不具合）」。本番 config・DB・daemon は変更していない。

## 結果の要約

- 不具合 5 件（旧 routing event・Pi 宣言・proxy wire 名・OpenCode model ID・class 別 route）はそれぞれ実装 leaf が入り、対応する試験は通った。
- **未達 1: 旧 event 入り DB の起動試験が無い。** `crates/celeris/tests/legacy_event_startup.rs` は空の DB で bootstrap と tick を見るだけで、`routing_decided`（mode `legacy`）の行を入れていない。ADR が挙げた `task_core::store::tests::legacy_routing_decided_event_loads_from_store` と `celeris::daemon::tests::startup_loads_legacy_routing_decided_event` は存在しない。読み取りは serde 単体（`legacy_routing_event_deserializes_without_becoming_writable`）でしか確かめていない。
- **未達 2: `CELERIS_USERNS_TESTS=1 bash scripts/dev/test-parallel.sh` が exit 100（closeout の再実行で再現）。** 5035 件中 5034 件が通り、1 件（`task-api::browser_h3_injection production_h3_injects_once_without_exposure`）が落ちた。USERNS 無しの `bash scripts/dev/test-parallel.sh` は exit 0（5035 passed）。本 WU は crate を変えていない（`git diff HEAD -- crates` は 0 file）。
- **人の判断が要る 1 件: CoS の Codex fallback。** 2026-10-13 付記 §3 は「claude_code は proxy endpoint を宣言できないので Codex OAuth proxy fallback を外し、CoS は Claude source のみ」とする。一方、試験 `migration_runbook_toml_loads` は 2026-10-12 付記の CoS `[1] = codex_oauth / gpt-6-astra` を検査している。試験コードを変えると closeout の範囲を超えるため、runbook は 2026-10-12 の形（Codex fallback あり）に残した。下の未解決事項 3 を参照。

## 各 leaf の要点（`2026-10-13-provider-release-blockers/` のメモ）

- `adr`（[adr.md](2026-10-13-provider-release-blockers/adr.md)）: 付記の 5 件の決定と試験名を ADR に書いた。
- `legacy-events`（[legacy-events.md](2026-10-13-provider-release-blockers/legacy-events.md)）: `RoutingMode::Legacy` を deserialize 専用で受ける（serialize は `skip_serializing`）。events は書き換えない。他の provider 削除由来の Event 欄（`WorkerHint.adapter` は未知 field として無視される）は追加互換不要と棚卸し。**未達: 上の「未達 1」。**
- `schema-sync`（[schema-sync.md](2026-10-13-provider-release-blockers/schema-sync.md)）: `#[schemars(skip)]` 付き variant が enum を `oneOf[enum]` に変えて `event.schema.json` と `api-v1.schema.json` がずれた。`RoutingMode` の JsonSchema を手書きに替え、`{"type":"string","enum":["shadow","enforce"]}` にした。生成型と schema は一致。
- `pi-harness`（[pi-harness.md](2026-10-13-provider-release-blockers/pi-harness.md)）: `[harnesses.pi]` に `extensions`・`tools`・`env`・`env_from_secrets` を追加。extensions/tools が空なら読み込みエラー。相対 extension path は TOML のあるディレクトリ基準で解決。旧 `pi-*` の値は本番 backup にしかないので、runbook では placeholder にした。
- `model-names`（[model-names.md](2026-10-13-provider-release-blockers/model-names.md)、leaf: `wire-core`・`worker-proxy`・`opencode-ns`・`pi-fixture`）:
  - `wire-core`: daemon の route 選択で `via = "llm_proxy"` の route だけ model を `<source>/<model>` にする（一度だけ作る）。event の canonical model は接頭辞なしのまま。
  - `worker-proxy`: PaperQA の `settings.llm`・`summary_llm`・`agent.agent_llm` を wire 名で上書き、LangMem は `with_model` で route model を使う。LDR は既存の route model 上書きを維持。
  - `opencode-ns`: `[harnesses.opencode].model_namespace` を OpenCode ACP にだけ適用。空値・`/` を含む値は拒否。
  - `pi-fixture`: `celerisctl` の `worker_tests.rs` の Pi fixture に extensions・tools を足した。
- `class-routes`（[class-routes.md](2026-10-13-provider-release-blockers/class-routes.md)、leaf: `core-dispatch`・`config`）: `[route_defaults.<h>].classes.<coding|planner|review>.candidates` を追加。class 列が宣言されていれば harness 既定の代わりに使い、fallback しない。claude_code の既定は `claude-sonnet-5-5`、coding だけ `claude-opus-5-5`。lane・tier・`RoutePolicy` は使わない。

## この WU（closeout）の変更

- `docs/ops/provider-llm-source-migration.md`:
  - 選択結果の表に claude_code の行（sonnet 先頭、coding は opus）と「その他の claude_code 仕事」行を反映（class-routes が入れた分を確認）。
  - opencode は `model_namespace = "qwen-local"` を宣言し、adapter は `qwen-local/qwen3.8-27b` を渡す、と手順 6 を直した（旧文は model を `qwen3.8-27b` にすると書いていた）。
  - LDR・PaperQA・LangMem は `via = "llm_proxy"` route の wire 名（`qwen/qwen3.8-27b`）が渡る、と説明を直した（旧文は「直接指すなら `qwen3.8-27b`」だったが、route があれば常に接頭辞付き）。LDR の `settings.llm.model` を `qwen/qwen3.8-27b` に揃えた。
  - 手順 3 に claude_code の先頭 sonnet・coding opus の確認、手順 7 に裸 model の 422 確認を足した。
  - 手順 8 に「旧 `routing_decided`（mode `legacy`）は読み取りだけで残り、移行で書き換えない」を足した。
- `agent-docs/progress/2026-10-11-provider-layer-redesign.md`: `migration_runbook_toml_loads` が runbook と一字一句一致を見るため、marker 間の TOML の写しを runbook に合わせた。
- 本ファイルを新規作成。

## 証拠コマンドと結果

- `CELERIS_USERNS_TESTS=1 bash scripts/dev/test-parallel.sh`（`TMPDIR=/local/prb`）— exit 100。nextest `Summary 5035 tests run: 5034 passed (1 slow), 1 failed, 13 skipped`。失敗: `task-api::browser_h3_injection production_h3_injects_once_without_exposure`（inner test が 503 `credential broker is not configured`）。同じ試験は個別実行（`cargo nextest run -p task-api --test browser_h3_injection`、`CELERIS_USERNS_TESTS=1` 付きでも）では 3/3 通る。全体実行では 2 回とも落ちた。`CELERIS_TEST_SUMMARY` 行の `passed: 3` は summary の解析が途中で止まったことによる表示で、nextest の集計は上の値。
- `cargo nextest run --workspace -E 'test(legacy) | test(pi_harness) | test(proxy_model_name) | test(opencode_model) | test(class_route) | test(route_priority) | test(migration_runbook)' --no-fail-fast` — `78 tests run: 78 passed, 4970 skipped`。
- `cargo nextest run -p celeris migration_runbook_toml_loads` — 1 passed（runbook 修正後）。途中、CoS の Codex 行を外したときは `cos_route_candidates[1]` で落ちたため、行を戻した。
- closeout で再実行（`TMPDIR=/local/prb`、`CARGO_TARGET_DIR` は Celeris 指定の scratch）:
  - `CELERIS_USERNS_TESTS=1 bash scripts/dev/test-parallel.sh` — exit 100。nextest `Summary [238.5s] 5035 tests run: 5034 passed (1 slow), 1 failed, 13 skipped`。失敗は上と同じ `browser_h3_injection production_h3_injects_once_without_exposure`。ログ: `/local/prb/tp-run.log`。
  - `bash scripts/dev/test-parallel.sh`（USERNS 無し）— exit 0。`Summary [118.8s] 5035 tests run: 5035 passed, 13 skipped`。`CELERIS_TEST_SUMMARY` の `passed: 5038` は doc-test を含む値。ログ: `/local/prb/tp-run2.log`。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished dev profile`、警告なし）。closeout 再実行でも exit 0。
- 文書検査: `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — exit 0（`check-doc-layout: ok`）。`sh scripts/dev/check-doc-links.sh` — exit 0（`check-doc-links: ok`）。`sh scripts/dev/check-adr-numbers.sh` — exit 0（`check-adr-numbers: ok (198 files)`）。
- runbook と 2026-10-11 progress の TOML 一致: 試験 `migration_runbook_toml_loads` が確認（pass）。
- `git diff HEAD -- crates` — 0 file（この WU はコードを変えていない）。

## 未解決事項

1. **旧 event 入り DB の起動試験（受け入れ条件 0）。** 必要な試験: `routing_decided` で `record.optimizer.mode = "legacy"` を 1 件以上書いた SQLite を作り（本番相当の payload）、`build_dispatcher` と `tick` を通し、events を書き換えていないことを確かめる。`crates/celeris/tests/legacy_event_startup.rs` にこの行を足すのが自然。実装の修正ではなく試験の追加なので、次の WU で行う。
2. **`browser_h3_injection` の失敗（受け入れ条件 1 の USERNS 版）。** `CELERIS_USERNS_TESTS=1` の全体実行でだけ落ちる（closeout で 1 回、前 WU で 2 回。個別実行では通る）。USERNS 無しの全体実行は通る。inner test は 503 `credential broker is not configured` を返す。原因は未確認。候補: 全体実行で credential broker 用の環境が渡らない、試験の inner process 起動の競合。本 WU の範囲外（crate を変えない）。
3. **CoS の Codex fallback の決定（人）。** 選択肢: (a) 2026-10-13 付記どおり Claude のみにする → runbook の `[cos].route_candidates` と `[[llm_routes]] claude_code←codex_oauth` を消し、試験 `migration_runbook_toml_loads` の CoS 検査を直す。(b) 2026-10-12 のまま Codex fallback を残す → 付記 §3 の「CoS は Claude source のみ」を改める。推奨は (a)（付記のほうが新しく、proxy 経由の claude_code は実装されていないため）。
4. **proxy_model_name 試験の件数。** `cargo test -p celeris proxy_model_name` は 0 件実行（celeris crate に該当試験が無い）。受け入れ条件に使うなら、`-p task-dispatch` と `-p task-worker` に寄せるか、celeris に 1 件足す。
5. **coding class は opus 1 件だけ。** opus が使えないと sonnet へは落ちず、claude_code が飛ばされて Codex へ行く。sonnet を 2 番目に入れるかは人が決める。
6. **daemon の `GET /api/v1/harnesses` は class 候補を出さない。** runbook 手順 6 の jq では coding class は見えないので、確認は `routing show`（手順 3）で行う。
7. **Pi の値は本番 backup からの転記が要る。** Hashline extension path と tool 名（read/edit）、env・env_from_secrets は、runbook では placeholder。checkout に旧 `pi-*` の実値は無かった。placeholder のまま `routing show` を通すと Pi 宣言の値が誤るので、転記してから検査する。
8. **test-parallel の補足。** 試験後に `tests left 1 entries in TMPDIR` の警告が出た（`/local/prb` 内、run 終了で消える）。`CELERIS_TEST_SUMMARY` の `passed` 値は解析が途中で止まるため当てにしない。

## 提案

- 未解決 1 の試験を、旧 event の行を入れた形で追加する（最も本番に近い受け入れの証拠）。
- 未解決 2 は、全体実行で broker 環境が inner process に渡るかを先に確かめる。
- CoS の決定（未解決 3）を先に人が下し、runbook と試験を一緒に直す。
- class 別 route の選択理由（どの候補が選ばれたか）を `routing_decided` の metadata に入れると、本番確認が DB だけで済む。

## 本番への移行

本番の config・DB・daemon・release は人が実行する。次の順で行う（runbook の「切替前の退避と確認」〜「読み込み・切替・確認」に従う）。

1. **config に入れる値（移行後 TOML の変更点）**:
   - `[harnesses.pi]`: `extensions` に旧 `pi-*` の Hashline extension path、`tools` に Hashline の read/edit 名と `bash`・`grep`・`find`・`ls`、旧 provider の env を `env`、secret は `env_from_secrets` に転記する。placeholder は実値に置換する（未解決 7）。
   - `[harnesses.opencode].model_namespace = "qwen-local"`: OpenCode config の provider ID と一致することを確かめる。
   - `[route_defaults.claude_code]`: 先頭は `claude-sonnet-5-5`、`[route_defaults.claude_code.classes.coding]` は `claude-opus-5-5`（`candidates` は `[route_defaults.<h>].classes.<coding|planner|review>.candidates` で宣言する。class 宣言があると harness 既定の代わりに使われ、fallback しない）。lane・tier は使わない。
   - `[adapters.local_deep_research].settings."llm.model"` と `[knowledge.langmem].model`: `qwen/qwen3.8-27b`。`[knowledge.langmem].base_url` は llm-proxy を指す。
   - CoS: 未解決 3 の決定に従う。
2. **staging で確かめる順（人が実行）**:
   - 作業用コピーの config を新リリースの `celerisctl routing show --config …` で検査（exit 0）。`claude_code:` の先頭が sonnet、`claude_code class coding:` が opus だけ、`pi` の候補が `opencode_go/opencode-go/grok-4.6 > qwen/qwen3.8-27b`。
   - `CELERIS_CONFIG=<作業用コピー> scripts/selfdeploy/verify.sh <新リリース>` を流す。**verify check 1（旧 `routing_decided` の読み込み）は、本番の旧 event を含む DB のコピーで確かめる。** 空の DB では本番の落ち方を再現しない（未解決 1）。
   - 昇格後、`journalctl --user -u 'celeris@<新リリース>.service'` に設定エラーが無いこと。
   - proxy: 接頭辞付き `qwen/qwen3.8-27b` が 200（`x-celeris-source: openai-compatible:qwen`）、裸の `qwen3.8-27b` が 422。
   - 低リスクな coding task・Pi task・`local-qwen` task・CoS を 1 件ずつ流し、`routing_decided` の harness・source・model を選択結果の表と照合する。
3. **戻し方**: runbook の rollback に従い、release と config を対で戻す。旧 event は読み取りのみなので、戻しても書き換えは発生しない。
