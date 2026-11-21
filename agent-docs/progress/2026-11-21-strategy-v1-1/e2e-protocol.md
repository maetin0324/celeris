---
title: 戦略層 v1 子 1 e2e-protocol — e2e の試験に残った worker protocol 4 の literal を 5 に直す
tasks: [01M7XQ9Q4P4ZQ38Y509J9XYAQ0]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# e2e-protocol（WorkUnit。段 fix の統合の test-affected の 1 件の失敗を直す）

完了日: 2026-11-21。変えたのは試験の 1 行と comment 4 行、ADR の D5 の文、進捗 2 file だけ。`crates/` は変えていない。

## 結論

- `tests/e2e/tests/scenarios.rs:437` の版の assert を `"protocol":4` から `"protocol":5` に直した。版の assert は消さずに残した。
- 直す前は `binary(scenarios)` の 4 件中 1 件（`command_check_fails_once_then_passes_after_retry`）が 437 行で落ち、直した後は 4 件とも通る。
- `PROTOCOL_VERSION` は 5 のまま（4 に戻していない）。

## 原因

- prompt-fix が `PROTOCOL_VERSION`（`crates/task-worker/src/protocol.rs`）を 4 から 5 に上げた（ADR 2026-11-21-ancestor-objective-in-prompts D5。
  子の一覧を `context` の直下から `context.lineage.children` に移した）。
- `tests/e2e/tests/scenarios.rs:437` は daemon が worker に渡す `run` 行の先頭を `{"type":"run","protocol":4` と固定していた。
- prompt-fix の範囲 check は `tests/e2e/` を許さなかったので、prompt-fix はこの行を直せなかった（prompt-fix.md の「範囲の外に残った 1 行」）。
- e2e の crate の workspace 内の依存は task-core だけ（`cargo tree -p e2e -e normal,dev` の workspace crate は `task-core` の 1 件）。
  test-affected は変更 crate の逆依存閉包だけを選ぶので、task-worker・task-dispatch だけを変えた prompt-fix の WU の test-affected は
  e2e を選ばなかった。段 fix の統合の test-affected は e2e を選び、8534 件中この 1 件が落ちた。

## 直した行（commit「strategy-v1 1 e2e-protocol: e2e の worker protocol の literal を 5 に直す」）

| file | 行 | 変更 |
|---|---|---|
| `tests/e2e/tests/scenarios.rs` | 437 | `assert!(run1.starts_with(r#"{"type":"run","protocol":4"#));` → `"protocol":5` |
| `tests/e2e/tests/scenarios.rs` | 435〜436 | comment を v4 の説明（ADR-0033 D4/D6）から v5 の説明（ADR 2026-11-21-ancestor-objective-in-prompts D5。子の一覧を `context.lineage.children` に移し、`context.lineage.ancestors` を足した）に直した |
| `tests/e2e/tests/delegation_scenarios.rs` | 269・618 | doc comment の `context.children` を `context.lineage.children` に直した。fake worker の script（`grep -q '"children":\[{'`）は変えていない |
| `agent-docs/adr/2026-11-21-ancestor-objective-in-prompts.md` | D5 | 「直す必要がある（release gate の全体の試験だけが走らせる）」を、この unit で直した事実と前後の試験の結果に書き換えた |
| `agent-docs/progress/2026-11-21-strategy-v1-1/prompt-fix.md` | front matter・完了日・範囲の外の節・未解決 | status を done、完了日を 2026-11-21 にし、範囲の外の 1 行の解消と証拠を書いた |

`git grep -n 'context\.children' -- tests/e2e/` → 0 件（exit 1）。

## 直す前の試験（この WU の開始時の HEAD = commit「strategy-v1 1 prompt-fix: 進捗に設計 review の指摘への対応・提出時の検査・範囲の外の 1 行を書く」）

- 1 回目: `cargo nextest run -p e2e --no-tests=fail -E 'binary(scenarios)'` → exit 0、4 passed。
  - 落ちなかった理由: e2e の試験は `CARGO_TARGET_DIR/debug/celeris` を起動する（`scenarios.rs` の `bin()`）。`-p e2e` は celeris の
    binary を build しない。target dir にあった celeris の binary は版 5 の変更より前の source から build されていたので、`run` 行に
    `"protocol":4` を書いた。
- 2 回目（binary を作り直す）: `cargo nextest run --workspace --no-tests=fail -E 'binary(scenarios)'` → exit 100。
  `Summary 4 tests run: 3 passed, 1 failed`。失敗の行:
  ```
  FAIL e2e::scenarios command_check_fails_once_then_passes_after_retry
  thread 'command_check_fails_once_then_passes_after_retry' panicked at tests/e2e/tests/scenarios.rs:437:5:
  assertion failed: run1.starts_with(r#"{"type":"run","protocol":4"#)
  ```
- 3 回目（作り直した binary で objective の command）: `cargo nextest run -p e2e --no-tests=fail -E 'binary(scenarios)'` → exit 100、
  同じ 437:5 の panic、`4 tests run: 3 passed, 1 failed`。

## 直した後の試験

- `cargo nextest run -p e2e --no-tests=fail -E 'binary(scenarios)'` → exit 0、`4 tests run: 4 passed, 0 skipped`。
- `cargo nextest run -p e2e --no-tests=fail -E 'binary(delegation_scenarios)'` → exit 0、`5 tests run: 5 passed, 0 skipped`
  （doc comment だけを変えた file。fake worker の動きが変わっていないことの確認）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `bash scripts/dev/test-affected.sh` → exit 0（3 分 32 秒）。対象 crate は `e2e`。`Starting 45 tests across 11 binaries`、
  `45 tests run: 45 passed, 0 skipped`。repo-health は `verdict=pass`（fail_grade=0、new=0、worsened=0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更 path は `tests/e2e/tests/scenarios.rs`・`tests/e2e/tests/delegation_scenarios.rs`・ADR 1 file・
  `agent-docs/progress/2026-11-21-strategy-v1-1/` の 2 file だけ。`crates/` の変更は 0 件。

## LoC（この WU の開始時の HEAD と HEAD。分類規則は開始時の HEAD の `scripts/dev/source-size-report.py` と `scripts/dev/source-size-report.toml` に固定）

`python3 <開始時の source-size-report.py> --root . --config <開始時の toml> --exclusive-classes --rev <rev> --format json`:

| 区分 | 開始時 | HEAD | 差 |
|---|---|---|---|
| production（全言語） | 376159 | 376159 | 0 |
| production（Rust） | 331468 | 331468 | 0 |
| external_test | 463935 | 463935 | 0 |
| own_total_with_tests | 1035657 | 1035657 | 0 |

試験は 5 行を消して 5 行を足した（行の置き換えだけ）。増えたのは docs（markdown）だけ。

## 未解決

- release gate の全体の試験（`cargo nextest run --workspace`）はこの WU では走らせていない（ADR 2026-10-20-full-tests-at-release-only）。
  版 4 の literal は他の file に残っていない（`git grep -n '"protocol":4' -- tests crates` → 0 件、exit 1）。

## 提案

- WU の test-affected は、task-worker だけを変えた WU で e2e を選ばない。e2e の試験は celeris の binary を起動して worker protocol の
  wire を確かめるが、e2e の `Cargo.toml` の workspace 内の依存は task-core だけなので、逆依存閉包に入らない。
  `crates/task-worker/src/protocol.rs`（wire の形）の変更で e2e を対象に足す規則を test-affected に置くかを、別の task で決める。
  今回はこの抜けのため、版を上げた WU ではなく段の統合で初めて落ちた。
- `cargo nextest run -p e2e` は celeris の binary を作り直さないので、古い binary で通ることがある（この WU の 1 回目）。e2e を手で
  走らせる手順では `--workspace` で build するか、先に `cargo build --workspace --bins` を走らせると書く。
