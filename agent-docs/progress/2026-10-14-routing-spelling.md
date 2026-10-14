---
title: "routing-eval の source・harness id を config 宣言の綴りに揃える"
tasks: [01M4M8D4AQHBC2KG3H1BPJ7Z0J]
status: done
updated: 2026-10-14
completed: 2026-10-14
---
# routing-eval の id の綴りを宣言に揃える

選択器（`task_core::llm_route`・`crates/celeris/src/config/llm_route.rs`）は根拠表を config の宣言の綴り
（`[llm_sources.claude_oauth]`・`[llm_sources.codex_oauth]`・`[harnesses.claude_code]`・`[harnesses.codex]`）で引く。
routing-eval の候補表がハイフン綴り（`claude-oauth`・`claude-code`・`codex-oauth`）だったため、評価試行・判断 record・
根拠表の行がその綴りで書かれ、評価結果が選択に効かなかった。正規化の分岐は足さず、書き込む側を宣言の綴りにした。

## 変更

- `crates/routing-eval/src/candidates.rs` `CANDIDATES`: `source_id` を `claude_oauth`・`codex_oauth`、`harness_id` を
  `claude_code`（codex は元から `codex`）に。trial 要求（`run.rs` `make_trials`）・判断 record・`aggregate` の根拠表行は
  この表から流れるので、書き込みはすべて宣言の綴りになる。
- 同 crate の試験・fixture（`aggregate.rs`・`export.rs`・`run.rs`・`tests/pipeline_stub.rs`）の id も宣言の綴りに。
- 試験 `candidates::tests::candidate_ids_match_declared_spelling`: `config/*.example.toml` をすべて
  読み、各候補の `source_id` が `[llm_sources.<id>]` に宣言され `kind` も同じ綴りで
  `task_core::LlmSourceRef` として読めること、`harness_id` が `[harnesses.<id>]` に宣言され `kind` が同じで
  `source_kinds` にその source を含むことを確かめる。節見出しと `kind`・`source_kinds` の行を試験内で行単位に拾う（Cargo.toml・Cargo.lock は変えない。attempt 2 で `toml` の dev-dependency を外した）。

## 証拠

- `grep -rnE 'claude-oauth|claude-code|codex-oauth' crates/routing-eval` → 0 件（exit 1）
- `cargo test -p routing-eval --lib -- --exact candidates::tests::candidate_ids_match_declared_spelling` → 1 passed
- `cargo test -p routing-eval` → lib 29 passed・pipeline_stub 1 passed、0 failed
- `bash scripts/dev/test-parallel.sh` → exit 0、5134 passed・0 failed
- `cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/check-doc-links.sh`・
  `sh scripts/dev/progress-index.sh --check` → いずれも exit 0

## 本番への移行

本番 DB（`/local/celeris/data/db/celeris.sqlite3`、2026-10-14 読み取りで確認、schema 79）には既にハイフン綴りの評価行がある:

- `eval_trials`: 400 行すべて（`claude-oauth`/`claude-code` 160 行、`codex-oauth`/`codex` 240 行）
- `route_decisions` の `stage='eval'`: 307 行すべて（`input_json.candidate` と `output_json.selected` に旧綴り）。
  `stage='shadow'` の 128 行には旧綴りは無い
- `routing_evidence`: 0 行（集計前なので移す行なし）
- `route_options`: 307 行の `options_json` に旧綴りがあるが、`hash` を主キーにした内容 record で `route_decisions.options_hash`
  から参照されるため書き換えない（`aggregate` は読まない）

`celeris-routing-eval aggregate` は `route_decisions.output_json.selected` と `eval_trials` を読むので、この 2 表を移せば
根拠表は宣言の綴りで出る。worker は本番 DB を書かない。人が次を実行する:

1. 新しい release（この変更を含む）へ昇格する前でも後でもよいが、評価 batch が走っていないこと（`queued`・`running` が 0）を確かめる:
   ```sh
   sqlite3 "file:/local/celeris/data/db/celeris.sqlite3?mode=ro" \
     "select status,count(*) from eval_trials where status in ('queued','running') group by 1;"
   ```
   （出力が空であること）
2. backup を取る:
   ```sh
   sqlite3 /local/celeris/data/db/celeris.sqlite3 "VACUUM INTO '/local/celeris/data/db/celeris.pre-routing-spelling.sqlite3';"
   ```
3. 移す（1 transaction）:
   ```sql
   BEGIN IMMEDIATE;
   UPDATE eval_trials SET source_id = 'claude_oauth' WHERE source_id = 'claude-oauth';
   UPDATE eval_trials SET source_id = 'codex_oauth'  WHERE source_id = 'codex-oauth';
   UPDATE eval_trials SET harness_id = 'claude_code' WHERE harness_id = 'claude-code';
   UPDATE route_decisions
      SET input_json  = replace(replace(replace(input_json,
                          '"claude-oauth"', '"claude_oauth"'), '"codex-oauth"', '"codex_oauth"'), '"claude-code"', '"claude_code"'),
          output_json = replace(replace(replace(output_json,
                          '"claude-oauth"', '"claude_oauth"'), '"codex-oauth"', '"codex_oauth"'), '"claude-code"', '"claude_code"')
    WHERE stage = 'eval';
   UPDATE routing_evidence SET source_id = 'claude_oauth' WHERE source_id = 'claude-oauth';
   UPDATE routing_evidence SET source_id = 'codex_oauth'  WHERE source_id = 'codex-oauth';
   UPDATE routing_evidence SET harness_id = 'claude_code' WHERE harness_id = 'claude-code';
   COMMIT;
   ```
   （`routing_evidence` は今 0 行。移行前に `aggregate` を本番へ書いていた場合の分）
4. 確認（すべて 0 であること、件数が変わっていないこと）:
   ```sh
   D="file:/local/celeris/data/db/celeris.sqlite3?mode=ro"
   sqlite3 "$D" "select count(*) from eval_trials where source_id like '%-%oauth' or harness_id='claude-code';"
   sqlite3 "$D" "select count(*) from route_decisions where stage='eval' and (input_json like '%-oauth\"%' or output_json like '%-oauth\"%' or output_json like '%\"claude-code\"%');"
   sqlite3 "$D" "select count(*) from routing_evidence where source_id like '%-oauth' or harness_id='claude-code';"
   sqlite3 "$D" "select source_id,harness_id,count(*) from eval_trials group by 1,2;"   # claude_oauth|claude_code|160, codex_oauth|codex|240
   ```
5. 戻すときは 2 の backup を daemon 停止中に戻す（`events` には触れていない）。

## 未解決事項

- 計画の check `cargo test … | grep -E 'FAILED|failed'; test $? -ne 0` は `test result: ok. N passed; 0 failed` の行と名前に failed を含む試験の行に必ず当たるので、試験が全部通っても exit 1 になる（plan_issue で申告）。`grep -E 'FAILED|[1-9][0-9]* failed'` などに直す必要がある。
- `task-api --test browser_live_cross` は run の `$TMPDIR` が長いと unix socket の path が SUN_LEN を超えて落ちる（短い TMPDIR では 3 passed）。環境依存で、この WU の変更とは無関係。

- `crates/task-dispatch/src/dispatcher/eval_trials.rs` の `normalize_id`（`-` → `_`、小文字化）で宣言を引く分岐が残っている。
  書き込みが宣言の綴りになったので不要だが、task-dispatch はこの WU の範囲外。

## 提案

- 上の移行を済ませた後、`normalize_id` を消して宣言の綴りの完全一致で引く（旧綴りの trial 要求は `Unroutable` にする）。
