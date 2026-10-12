---
title: "commit の無い root で取り込み判定を出さず、審査範囲を base..head と明示する"
tasks: [01M4P84BYNVR2FT0Y7VTN8P8PF]
status: done
updated: 2026-10-12
---

# 取り込む commit が無い root の delivery 見送り（2026-10-11）

決定: [ADR-0121 付記: 取り込む commit が無い root（2026-10-11）](../adr/0121-root-delivery-without-assignee.md)。

## 起きたこと

日次整理 task `01M4P733B9588PC6QZ9C2VH0NT` は commit 0 件（head = base = `ca7025be`）なのに【部署内のマージ可否判定】criterion が足され、
reviewer が「指定SHAの実際の差分」を `4d97d291..ca7025be`（別 task の成果）と読んで 2 回不合格にした。

## 変更（WU noop-delivery）

- `task-core`: `DeliverySkipReason::NoCommits`（code `no_commits`、label「取り込む commit がありません」）。
- `task-ops/src/delivery.rs` `prepare`: base・head 解決後に `git merge-base --is-ancestor <head> <base>` が真なら `no_commits` で見送り（criterion を足さない、`DeliverySkipped` 1 件）。
- 同 criterion 文: 審査対象を `git log {base}..{head}` と `git diff {base}...{head}` に限定し、head 単体の親との差分・base 側の commit は対象外と明記。旧語「指定SHAの実際の差分」は削除。
- `task-ops/src/inbox.rs` `build_attention`: `no_commits` は注意事項にしない。
- schema 再生成: `UPDATE_SCHEMA=1 cargo test -p task-core --lib schema` / `-p task-api --lib schema`、`corepack pnpm@12.6.0 -C web gen:types`。

## 証拠

- `cargo test -p task-ops --lib -- no_commits merge_criterion delivery_skip existing_reviewer` → 13 passed, 0 failed。
- `cargo test -p task-ops --lib -- delivery inbox` → 75 passed, 0 failed。
- `grep -c no_commits docs/api/v1/api-v1.schema.json docs/api/v1/event.schema.json web/api/generated/types.ts web/api/generated/schema.json` → 各 1。
### 検証 WU（Run #2、統合後の branch `celeris-wu/01M4P84BYNVR2FT0Y7VTN8P8PF/verify`、HEAD `c470611f`）

- `TMPDIR=/var/tmp bash scripts/dev/test-parallel.sh` → exit 0。`CELERIS_TEST_SUMMARY {"nextest_exit":0,"doctest_exit":0,"passed":5215,"failed":0,"ignored":14,"tmp_leftovers":0,"summary_parsed":true}`（nextest は 5212 run: 5212 passed, 13 skipped、doc-test 10 binary 全 ok）。ログ: `artifacts/test-parallel-8.log`。
- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished dev profile`）。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0（`ok`）。`check-doc-links.sh` → exit 0（`ok`）。`check-adr-numbers.sh` → exit 0（`ok (196 files)`）。

### 再検証（Run #4、HEAD `c470611f`、同じ branch）

- `TMPDIR=/var/tmp bash scripts/dev/test-parallel.sh` 1 回目 → exit 100。`CELERIS_TEST_SUMMARY {"passed":5211,"failed":4,"nextest_exit":100}`。失敗は task-worker `browser::launcher_run::tests::launcher_login` の 4 件（`launcher_authenticate_rejected`、負荷 load average 20〜50 の中で発生）。ログ: `artifacts/test-parallel-run4.log`。
- 同 6 件を単独で `cargo nextest run -p task-worker -E 'test(launcher_login)'` → exit 0、6 passed。ログ: `artifacts/launcher-login-alone.log`。本 branch は task-worker の browser 経路に触れていない（`ca7025be..HEAD` の変更は task-core・task-ops・schema・進捗のみ）ので、負荷による揺れと判断した。
- `TMPDIR=/var/tmp bash scripts/dev/test-parallel.sh` 2 回目 → exit 0。`CELERIS_TEST_SUMMARY {"runner":"nextest","binaries":186,"passed":5215,"failed":0,"ignored":14,"nextest_exit":0,"doctest_exit":0,"tmp_leftovers":0,"summary_parsed":true}`（nextest 5212 run: 5212 passed、13 skipped、doc-test 10 binary ok）。ログ: `artifacts/test-parallel-run4b.log`。
- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished dev profile`）。ログ: `artifacts/clippy-run4.log`。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0（`ok`）。`sh scripts/dev/check-doc-links.sh` → exit 0（`ok`）。`sh scripts/dev/check-adr-numbers.sh` → exit 0（`ok (196 files)`）。

### 前回の失敗と原因（試験の期待値は変えていない）

- 既定の `$TMPDIR`（93 文字）のままでは browser 系の試験（celeris-credentiald・task-worker・task-api・celerisctl）が、Unix socket path の `SUN_LEN`（108 byte）超過で fixture readiness timeout になった（上の初回証拠）。
- `test-parallel.sh` は `$TMPDIR/celeris-test-parallel.XXXXXX/tmp` を入れ子にして試験の `TMPDIR` にするため、socket path は `TMPDIR` を `/run/user/1001`（14 文字）にしても超えた（`browser_e2e` 4 件が 60 秒の readiness timeout）。`TMPDIR` は 9 文字以下が要る。
- `TMPDIR=/dev/shm` は短いが、db guard 試験が「/dev overlaps the production directory」で Refuse し、`worker_db_guard_*` 2 件と `dispatch_browser_fallback_*` 1 件が落ちた。db guard の判定は正しいので、`/dev` 配下を避ける必要がある。
- `TMPDIR=/var/tmp` で全試験が通った（上の最終証拠）。`/var/tmp` は 8 文字で socket 条件を満たし、`/dev` 配下でもない。
- 負荷（load 30〜110）による timing の揺れは、単独実行（`five-alone.log` で 5 件 pass、`nextest-4pkg.log` で 2077 pass）では再現しなかった。

### 検証 WU で得た注意

- 試験の `TMPDIR` は `test-parallel.sh` の入れ子で socket path を長くする。release host 側でも `TMPDIR` が長いと同じ失敗になる。直すなら `test-parallel.sh` の入れ子を短くするか、試験の socket dir を短くする（`scripts/dev/` と試験の変更になるため、本 WU では行わない。提案節を参照）。

## 本番への移行

不要。DB の変更なし（events の reason 値が 1 つ増えるだけで migration なし）。config の変更なし。

## 未解決事項

- `test-parallel.sh` の入れ子 TMPDIR と browser 試験の socket path 長の関係は本変更と無関係。検証は `TMPDIR=/var/tmp` で通した。`/var/tmp` は共有の場所なので、人が release の既定を決めるまでの暫定。

## 提案

- `scripts/dev/test-parallel.sh` は試験の `TMPDIR` を入れ子にしない（または試験の socket dir を短い固定の場所へ置く）。そうすれば `TMPDIR` の長さに依らず browser 試験が通る。試験と script の変更を伴うので別の task で扱う。
- 本番 `release` の host でも同じ `TMPDIR` 条件になるかを確かめる（人の判断）。
