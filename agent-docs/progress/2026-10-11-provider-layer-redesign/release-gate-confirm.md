---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
status: done
title: provider 層の再設計 — release gate で落ちた e2e worker_db_read_only の修正（16af126f）を確かめる
completed: 2026-10-13
updated: 2026-10-13
---

# release-gate-confirm: e2e worker_db_read_only

親: `agent-docs/progress/2026-10-11-provider-layer-redesign.md`。修正の内容は `release-gate-fix.md`（16af126f）。
この葉はコードを変えず、修正を確かめて結果を記録する。`tests/e2e/tests/worker_db_read_only.rs` も crates・docs も変えていない。

## 条件ごとの結果

### 0. e2e worker_db_read_only が build でき exit 0 で終わる

- `cargo test -p e2e --test worker_db_read_only`（userns なし）: exit 0、`1 passed; 0 failed`、0.00s。
  この試験は `CELERIS_USERNS_TESTS=1` が無いと early return する（ADR-0126）ので、実質は SKIPPED。build と起動だけ確かめた。
- `CELERIS_USERNS_TESTS=1 cargo test -p e2e --test worker_db_read_only`（userns 込み）: 初回は exit 101。
  `celerisctl not found; run cargo test --workspace` で落ちた。試験は同じ target dir の `celerisctl` を前提にし、
  `cargo test --workspace` が作る。`cargo build -p celerisctl -p celeris` で補ったあと再実行し、exit 0、
  `a_codex_worker_run_cannot_write_the_daemon_db_but_can_read_it_with_celerisctl ... ok`、`1 passed; 0 failed`、2.24s。
  これは試験の build の前提の問題で、config の問題ではない。

### 1. 試験 config が新しい宣言を持つ

`tests/e2e/tests/worker_db_read_only.rs` の config に次があることを読んで確かめた（行は 16af126f 時点）。

- `[accounts] codex_dir = "codex-accounts"`（227〜228 行）
- `[llm_sources.codex]`、`kind = "codex_oauth"`、`models = ["codex"]`（234〜237 行）
- `[harnesses.codex]`、`kind = "codex"`、`source_kinds = ["codex_oauth"]`（239〜242 行）
- `[route_defaults.codex]`、`candidates = [{ source = "codex", model = "codex" }]`（251〜252 行）

`[adapters.codex]`（230 行）は残っている。このため旧形の欄が config に 1 つ残る。試験の意図には影響しない。
ただし `[adapters]` は本 task が消す旧い仕組みの一部なので、後の段で消す候補として残す。

### 2. 記録

- 本ファイル（`agent-docs/progress/2026-10-11-provider-layer-redesign/release-gate-confirm.md`）。

### 3. 文書検査 3 本

- `bash scripts/dev/check-doc-links.sh`: exit 0、`check-doc-links: ok`
- `bash scripts/dev/check-adr-numbers.sh`: exit 0、`check-adr-numbers: ok (198 files)`
- `sh scripts/dev/progress-index.sh --check`: exit 0、`progress-index --check: ok`

## 追加の検査（人の指示どおり release gate と同じ条件）

- `cargo fmt --all -- --check`: exit 0（出力なし）
- `cargo clippy -p e2e --tests -- -D warnings`: exit 0、`Finished dev profile`
- `CELERIS_USERNS_TESTS=1 bash scripts/dev/test-parallel.sh`: **2 回とも exit 100**。
  - 1 回目: `Summary [247.053s] 4995 tests run: 4994 passed (1 slow), 1 failed, 13 skipped`
  - 2 回目: `Summary [210.887s] 4995 tests run: 4994 passed (1 slow), 1 failed, 13 skipped`
  - 失敗は 2 回とも同じ 1 件: `task-api::browser_h3_injection production_h3_injects_once_without_exposure`。
    `503 browser_unavailable — credential broker is not configured`（`crates/task-api/tests/browser_h3_injection.rs:985` が内側の試験を起こす）。
  - 同じ試験を単独で `CELERIS_USERNS_TESTS=1 cargo test -p task-api --test browser_h3_injection production_h3_injects_once_without_exposure` で走らせると exit 0、`1 passed`、2.76s。
  - worker_db_read_only が全体の実行に含まれ通ったかは、ログからは確認できていない（nextest の出力は失敗だけを表示する）。失敗の一覧に含まれないことだけ言える。
  - この 2 回で `crates/` の差分は無い（16af126f から HEAD まで空）。
  - 判定: 全体の負荷（8 並列）の中で broker の起動が間に合わず、試験の待ちが足りないと読める。確証は無い。
    今回の変更（`tests/e2e` の試験 1 本と config）とは関係の無い crate の試験。人の条件「失敗 0」は**満たしていない**。

## 未解決事項

- **H3 試験の負荷依存の失敗（上の 1 件）。** 全体の負荷の中で 2 回続けて落ち、単独では通る。broker の起動待ちが時間依存に見える。
  直すなら `crates/task-api/tests/browser_h3_injection.rs` の broker 起動待ちを出来事待ちに置き換える（時間に依存しない待ち方）。
  この葉の範囲（crates を変えない）の外なので直していない。release gate の前に別の葉で直す必要がある。
- `CELERIS_TEST_SUMMARY` の `passed` は 3 と出る（nextest の `4994 passed` と合わない）。`release-gate-fix.md` の未解決事項と同じ。集計の parse の問題で、試験の結果とは別。
- 試験が `celerisctl` と `celeris` の binary を前提にする（`cargo test --workspace` で作られる）。`-p e2e --test` だけを単独で走らせると `not found` で落ちる。
  試験の前提を明示するか、binary が無ければ build する形に直すと、単独実行の失敗が無くなる。

## 提案

- release gate の前に、H3 試験の broker 起動待ちを出来事待ちに直す小さい葉を足す（`crates/task-api` の試験のみ）。
- `[adapters.codex]` は残っているので、後の段で `[adapters]` の欄を消し、読み込みで拒否する（破壊的変更の方針）。
