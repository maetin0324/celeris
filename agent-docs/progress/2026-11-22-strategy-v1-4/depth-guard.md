---
title: 戦略層 v1 子 4 depth-guard — Markdown の front matter の flow 値の入れ子に深さの上限を付け、取り込みの stack overflow を 422 にする
tasks: [01M7XQ9QK1KC34Q2A6PJ960TJB]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# depth-guard（WorkUnit。子 4 の計画 v4 の段 harden）

完了日: 2026-11-22（2 回目の run）。最終 review（reviewer run 01M8237ECGA6QER1CZKWEXNJCQ）の criterion 4 の不合格の直し。
設計は [ADR 2026-11-22-external-llm-discussion-exchange](../../adr/2026-11-22-external-llm-discussion-exchange.md) の D1.3・D4.2。

## 結論

- `crates/task-core/src/discussion_exchange/format.rs` の `parse_value` は、front matter の flow 値（`{…}`・`[…]`）を深さの上限なしに再帰で読んでいた。
  32,768 byte の中で `source: ` の後に `[` を 5,000 段入れると、2 MiB の stack（tokio の `spawn_blocking` と同じ）で stack overflow になり process が abort した。
- 直し: 定数 `discussion_exchange::FLOW_DEPTH_MAX = 16` を D1.3 の他の上限と同じ場所に置いた。`parse_value(s, depth)` を `Result` にし、
  新しい `flow_parts` が 17 段目の flow collection を `ExchangeRefusal`（`exchange_invalid`、REST は 422、detail は
  `a front matter flow value nests deeper than 16 levels`）で拒否する。切り詰めて受けない。呼び出し元の `parse_fields` は既に `Result` を返すので
  `?` で伝えるだけで、`markdown_fields`・`split_front_matter`・`snapshot_from_markdown` の形は変わらない。
- JSON として読める値は従来どおり `serde_json`（上限 128 段）が読み、flow の段に数えない。
- handler（`crates/task-api/src/chat/exchange.rs`）には何も足していない。
- ADR D1.3 の表に「Markdown の front matter の flow 値の入れ子 | 16 段」の行を足した。
- 副次の直し: `crates/task-api/src/lib.rs` で `/// ADR-0072（Phase E2）…` の doc comment が `pub mod exchange_wire;` に付いていたので、
  `pub mod exchange_wire;` をその comment の上に移し、comment を `pub mod execution;` に戻した。

## 実装前 brief

1. 根本原因: `format.rs` の `parse_value` が flow 値の 1 段ごとに自分を呼ぶ再帰で、段数の上限が無い。`DOCUMENT_MAX_BYTES` は長さだけを抑え、
   1 段 2 byte なら 32,768 byte に 16,000 段まで入る。症状は REST の `POST /api/v1/chat/threads/{t}/exchange/imports`（format=markdown）の
   blocking thread の stack overflow（process の abort）として出る。
2. 責務 owner: front matter の読み手 `task_core::discussion_exchange::format`（`parse_value`）。
3. 既存の再利用先: `ExchangeRefusal`・`INVALID`・`invalid()`（同じ file）。`parse_fields` は既に `Result`。試験は既存の `discussion_exchange_tests.rs` と
   `crates/task-api/tests/chat_exchange.rs` の helper（`env`・`thread`・`base`・`exchange_rows`・`assert_problem`）。
4. 選択肢: (a) parser に深さの上限を付けて拒否する（採用。owner の直し、決定的、切り詰めない）。(b) handler の blocking thread の stack を大きくする
   （上限が残らず、他の呼び出し元を守れない。規約 1 の症状の場所の特例）。(c) 再帰を explicit stack に書き換える（上限なしの入れ子を受けること自体が
   入力長の規則の外で、変更も大きい）。(d) 上限の段で切り詰める（task の規則「切り詰めて受けない」に反する）。
5. 新しい状態・分岐・fallback・port: 無し。定数 1 つと、上限を超えたときの拒否の分岐 1 つ（`flow_parts`）。
6. 撤去対象: 無し（旧い経路は無い）。`lib.rs` の付け違えた doc comment を正しい module に戻す。
7. 不変条件: 32,768 byte 以内の Markdown の取り込みは、入れ子の深さによらず 2 MiB の stack で値を返す（abort しない）。17 段以上の flow 値は
   `exchange_invalid`（422）で拒否し、何も書かない。16 段までは従来どおり読む。試験は下の 3 件。

## 試験（守る不変条件と、既存の試験で検出できない理由）

既存の試験は flow 値を 2 段より深く入れず、入力長の試験（`external_exchange_import_too_long`）は長さの上限しか見ないので、深さの不具合を検出できなかった。

- `external_exchange_format_depth_deep_nesting_refused`（task-core）: 2 MiB の stack の `std::thread` で `check_import`（format=markdown）に、
  `source: [`×5,000…`]`×5,000、`source: {a: `×5,000 `x` `}`×5,000、`references:` の block list の項目に `[`×15,000…`]`×15,000 を渡す。
  文書全体が `DOCUMENT_MAX_BYTES` 以内であることを assert し、`exchange_invalid`・422・detail に `deeper than 16` を確かめる（長さの上限ではなく深さの上限で拒否）。
- `external_exchange_format_depth_limit_16`（task-core）: flow mapping と flow list のそれぞれで、16 段の `references` の項目は `check_import` を通り
  （受け取った metadata の入れ子が 16 段）、17 段は `exchange_invalid` になる。flow mapping の中の 40 段の JSON 値は `serde_json` が読み、拒否されない。
- `chat_exchange_import_deep_nesting_rejected`（task-api、REST）: `source:` に 5,000 段（文書 10 KB）の Markdown の取り込みが 422 `exchange_invalid`
  （problem に `deeper than 16`）で、交換の記録は (0, 0)。続けて同じ daemon（router）への通常の Markdown の取り込みが 201 で受け取られる。

### 変更前に落ちる probe（base の `format.rs` で実行）

試験と定数 `FLOW_DEPTH_MAX` を先に書き、`format.rs` を変える前（base の内容のまま。git で戻していないので mtime の問題は無い）に実行した。

- `cargo nextest run -p task-core --lib -E 'test(/external_exchange_format_depth_/)' --no-fail-fast` → exit 100、`2 tests run: 0 passed, 2 failed`。
  - `external_exchange_format_depth_deep_nesting_refused`: `SIGABRT`。`thread '<unknown>' has overflowed its stack` / `(test aborted with signal 6: SIGABRT)`。
  - `external_exchange_format_depth_limit_16`: FAIL。16 段は読めたが、17 段が `Inbound { client_message_id: "imp-17", … }` で受け取られ
    `17 levels are refused` の `expect_err` で panic（base には深さの上限が無い）。
- `cargo nextest run -p task-api --test chat_exchange -E 'test(chat_exchange_import_deep_nesting_rejected)' --no-fail-fast` → exit 100、
  `1 test run: 0 passed, 1 failed`。`SIGABRT`、`thread 'tokio-rt-worker' has overflowed its stack` / `(test aborted with signal 6: SIGABRT)`
  （試験の process 全体が abort。本番では celeris の daemon 全体が止まる経路）。

### 変更後

- `cargo nextest run --status-level all -p task-core --lib -E 'test(/external_exchange_format_depth_/)'` → PASS 2 件、`2 tests run: 2 passed`。
- `cargo nextest run --status-level all -p task-api --test chat_exchange -E 'test(chat_exchange_import_deep_nesting_rejected)'` → PASS、`1 test run: 1 passed`。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-core --lib -E 'test(/external_exchange_|chat_exchange_|strategy_contract_|chat_discussion_summary_strategy/)'` → exit 0、
  `49 tests run: 49 passed`（前回の review の 47 件と新しい 2 件。Markdown と JSON の往復の試験も通る）。
- `cargo nextest run -p task-api --test chat_exchange` → exit 0、`13 tests run: 13 passed`（既存 12 件と新しい 1 件）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は task-core の逆依存の 14 crate で `Summary 8700 tests run: 8700 passed (12 slow), 31 skipped`、
  web の typecheck と unit（`Test Files 105 passed`・`Tests 745 passed`）、`test-ts-verify: 20 passed, 0 failed`、`script bundle SKIP: 0 件`。
  repo-health は `verdict=pass`・`fail_grade=0`（new 13・worsened 2 は close の時点と同じ数で、この unit で増えていない）、`repo_health_gate: all checks passed`。
- `sh scripts/dev/progress-index.sh --check` → ok。`sh scripts/dev/check-doc-links.sh` → ok。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は ADR・この進捗・`crates/task-api/src/lib.rs`・`crates/task-api/tests/chat_exchange.rs`・
  `crates/task-core/src/discussion_exchange.rs`・`discussion_exchange/format.rs`・`discussion_exchange_tests.rs` の 7 件。

## LoC

production の変更は `format.rs`（+34・-13 行、doc comment を含む）、`discussion_exchange.rs`（定数と doc comment の +4 行）、`lib.rs`（2 行の入れ替え）。
試験は `discussion_exchange_tests.rs` +134 行・`chat_exchange.rs` +49 行。純増の理由は、上限の定数・拒否の分岐と、それを守る試験だけで、消せる旧い仕組みは無い。

## 回帰の条件

- 16 段以内の hand-written の flow 値・JSON の値・block list は従来どおり読む（`external_exchange_markdown_json_roundtrip_hash`・`external_exchange_format_depth_limit_16`）。
- 上限を超えた入れ子は `exchange_invalid`（422）で、何も書かない。daemon は次の要求に答える（`chat_exchange_import_deep_nesting_rejected`）。

## 未解決事項

- `snapshot_from_markdown` も同じ読み手を通るので同じ上限が掛かる（本番の呼び出し元は試験と手引きの往復だけ）。
- 本番での確認は未確認（人の昇格の後でないと本番に無い）。

## 提案

- 無し。
