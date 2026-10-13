---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
status: done
title: provider 層の再設計 — crates の残り（tier alias と /providers の注釈・試験 fixture）
completed: 2026-10-13
updated: 2026-10-13
---

# crates-residue: crates に残る旧い仕組みを消す

親: `agent-docs/progress/2026-10-11-provider-layer-redesign.md`。方針: 破壊的変更を是とし、旧い仕組みを同じ変更で消す（ADR 2026-10-11-breaking-changes-by-default）。final review（01M4SQF6V9）の差し戻しと docfix（eff7b67a）の後、crates に残った `celeris/{frontier,standard,cheap}`・`/providers` の注釈と試験 fixture を直した。

## 変えたこと（leaf 別。詳細は `crates-residue/<key>.md`）

- **worker-route**（task-worker・生きた分岐）: `routing_context_transport.rs`（OpenCode・Aider）と `pi.rs` の tier alias 判定を消した。header は「接続先（proxy の base URL）」と「`routing_context_ref`」の両方がある時だけ付ける。Pi の model 名は `provider/model` として常に解釈する。試験 fixture も具体的な model（`qwen/qwen3.8-27b`・`opencode-go/grok-4.6`）に替えた。接続先や ref が無い時に header が付かないことを試験で確かめた。
- **worker-fixtures**（task-worker の試験・paperqa の例）: `celeris/standard|cheap` → `qwen/qwen3.8-27b`（前置き付きの形も同じ規則）、`celeris/frontier` → `opencode/opencode-go/grok-4.6`。tier 語の入った試験名を外した。`paperqa.rs` の `strip_provider_prefix` の tier alias 例外（`prefix != "celeris"`）を消した。
- **acp-fixture**（task-worker）: acp の routing context 試験に proxy の base URL を足した。`default-config` 側に接続先が無かったため header が付かず落ちていた（試験の誤り。transport は正しい）。接続先が無い時に header を付けないことの負の試験を追加。OpenCode 2 件の `celeris-proxy/celeris/cheap` → `celeris-proxy/qwen/qwen3.8-27b`。
- **core-residue**（task-core）: `harness.rs`・`knowledge_run.rs` の proxy の説明を `<source_id>/<model_id>` と route_defaults の語に直した。`routing_log_tests.rs` の要求例を `qwen/qwen3.8-27b` に。migration SQL の歴史的注釈は残した。
- **api-residue**（task-api）: `secrets.rs`・`secrets_admin.rs`・`cluster_connect_admin.rs` の `/providers` 注釈を今の語に。`tests/routing.rs` の `requested_model` を `qwen/qwen3.8-27b` に（task-api の src はこの欄を読まないので期待値は変わらない）。

## langmem fallback の扱い

- `HarnessFallback::Tier(cheap)` は、旧 provider の tier 対応には依存していない。`dispatch_run.rs` が langmem 接続先の probe に失敗したとき、`worker_hint.tier = cheap` を立てて通常の tier routing に戻すだけ。
- 明示の source/model に固定するには、task-core の形だけでは足りず、config 転送・dispatch の fallback・routing を含む task-dispatch / celeris の変更が要る。今回は task-core の範囲外として未変更にした（core-residue メモの「未解決」節）。

## 証拠

- 残りの grep（受け入れ 0）:
  - `grep -rnE 'celeris/(frontier|standard|cheap)|/providers\b' crates | grep -vE '^crates/llm-proxy/src/config.rs:|^crates/llm-proxy/tests/proxy_integration.rs:|^crates/task-core/migrations/|^crates/task-api/tests/cos_operations.rs:'` → 出力なし、exit 1（0 件）。
- `TMPDIR=/local/crc bash scripts/dev/test-parallel.sh`（replan v5 の closeout、統合後 HEAD d4f4153e）→ exit 0、`test-parallel: ok`。nextest の集計行は `Summary [270.902s] 4995 tests run: 4995 passed (1 slow), 13 skipped`（failed 0）。doc-test は全 binary ok、`doctest_exit 0`、`tmp_leftovers 0`。ログは `artifacts/test-parallel-v5.log`（`/local/crc/tp.log`）。
  - 注意: 同じ行の `CELERIS_TEST_SUMMARY` の `passed` は `3` と出る。`scripts/dev/test-parallel.sh` の集計が `(1 slow)` を含む行を読み違えるため（`npass` が立たない）。試験の結果ではなく集計の不具合。直すのは別 task（下の未解決事項）。
- `TMPDIR=/local/crc cargo clippy --workspace -- -D warnings` → exit 0（`Finished`、warning・error 0 件）。`cargo fmt --all -- --check` → exit 0。ログは `artifacts/clippy-v5.log`・`artifacts/fmt-v5.log`。
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。
- 前 run の check（`TMPDIR=/local/celeris/data/scratch/cr-close`）は exit 100（SUN_LEN）。replan v5 では TMPDIR を `/local/crc` にした objective に変わった。短い TMPDIR で全件通ることを確認済み。

### 失敗した試行（環境の問題。コードは変えていない）

- 1 回目（`TMPDIR` を run の tmp にした）と 2 回目（`/local/celeris/data/scratch/cr`）は、credentiald・browser launcher・browser_waits の計 30〜40 件が落ちた。原因は `path must be shorter than SUN_LEN`（Unix socket の path が 108 バイトを超えた）。`test-parallel.sh` は `$TMPDIR/celeris-test-parallel.XXXXXX/tmp` を作り、その下に試験の socket を作るので、TMPDIR が長いと超える。
- `TMPDIR=/local/cr` にすると全部通った。今後の試験も、TMPDIR は短い path にする（`test-parallel.sh` が中にさらに 2 段の directory を作るため。試験の方針は `agent-docs/guides/testing.md` を参照）。
- 3 回目の check（`TMPDIR=/local/celeris/data/scratch/cr-close`）も同じ SUN_LEN で exit 100（nextest 32 failed）。`/local/celeris/data/scratch/cr` より長く、`/local/cr` より長い。check の path が固定なので、コードの修正では通らない。
- `unshare -U true` は成功した（userns は使える。`userns: null` は CELERIS_USERNS_TESTS を立てていないため）。

## 未解決事項

- **paperqa の `strip_provider_prefix`**（worker-fixtures メモ 2）: 小文字の最初の区画を LiteLLM の接頭辞とみなして剥がす。`<source_id>/<model_id>` の形（例 `qwen/qwen3.8-27b`）を接頭辞なしで `config.model` や settings の `llm` に書くと `qwen3.8-27b` に剥がれ、検索語の LLM 先を誤るおそれがある。tier 例外とは別の、もともとの規則。剥がしを `openai/` 等の既知の接頭辞に限るかは判断が要る。この変更では挙動を変えていない。
- **langmem fallback の明示化**（上の節）: 明示 route にするなら別の task（task-dispatch・celeris を含む）。
- **test-parallel.sh の nextest 集計が `(N slow)` を読み違える**: `scripts/dev/test-parallel.sh` の正規表現 `(\d+) ([a-z ]+?)(?:,|$)` は `passed (1 slow)` の `passed` を取れず、CELERIS_TEST_SUMMARY の `passed` が 3 になる（nextest の本体は failed 0）。`(...)` を除いてから数える形に直す別 task。
- **計画の check の TMPDIR が長すぎる**（前 run で plan_issue として申告。replan v5 で `/local/crc` に直された）: 計画の check `TMPDIR=/local/celeris/data/scratch/cr-close bash scripts/dev/test-parallel.sh` は、unix socket の path が 108 バイトを超えて task-worker の browser launcher・credentiald 等 32 件が落ち、exit 100 になる。短い TMPDIR（`/local/cr`）なら全件通る。check を `TMPDIR=/local/cr` 等の短い path に直すか、`test-parallel.sh` が長い TMPDIR を先頭で弾くように直すのが要る。
- **crates 外の残り**（この unit の範囲外。grep はしていない）。これまでの記録からの引き継ぎ: `scripts/dev/cos-chat-bench.sh`・`cos-chat-live.sh`・`scripts/live-check-phase53.sh`・`scripts/selfdeploy/tests/migrate_to_local_test.sh`・`scripts/test_portable_providers.py` は `[[providers]]` を書く。`docs/ops/opencode-go-and-model-catalog.md` §3a は `POST /api/v1/providers` を説明している。
- **process_group の flake**（worker-fixtures の記録）: 並列負荷下で 25ms × 200 の待ちが足りず落ちた。最終の 2 回の実行では再現しなかった。再発したら、時間待ちを出来事待ちに直す（ADR-0125）。

## 提案

- 残りの crates の grep を CI（または release gate）の検査に入れ、例外の一覧を grep の `-v` 条件と一緒に固定する。今回の受け入れ条件と同じ形。
- `TMPDIR` の短さを `test-parallel.sh` の先頭で検査し、長ければ止めるか警告する（今回の失敗を事前に分かるようにする）。
- paperqa の剥がしを既知の接頭辞に限る変更を、別の leaf として切る（上の未解決事項 1）。
- langmem の fallback を明示 route にする設計を、ADR 付きで別 task にする。
