---
title: S4 host-adapter — 本物の deno_core host で cos_digest の replay と compare を試す
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 host-adapter — cos_digest の本物の host の試験

完了日: 2026-11-02（unit host-adapter）。試験と文書だけの変更。本番コードは変えていない。`ScriptPolicy` の `cos_digest` に不備は無かった。

## 結論
試験 `crates/celeris/tests/script_compare_cos_digest.rs`（5 件）を足した。userns・外部ネットワーク・LLM は使わない。triage run の結果は store に直接書く。

| 目的文の項目 | 試験 | 確かめたこと |
|---|---|---|
| (1) 全行の一致 | `..._real_host_matches_rust_on_all_fixtures` | cases と traces の全行を同じ isolate に正順・逆順で通し、Rust の `cos_digest_decide` と byte 一致。isolate の作り直しは 0 回 |
| (2) compare の digest pass | `..._dispatch_match_replay_and_rollback` | run ごとに `match` が 1 行。message と item の行は mode rust の基準と byte 一致。2 回目の tick で増えない |
| (3) mode rust | 同上 | actor の完了数が増えず、記録も増えない。digest の message は書かれる |
| (4) 例外・違う答えの bundle | `..._throwing_bundle_...`・`..._different_answer_...` | outcome は `ts_error`（`exception`）と `mismatch`。message と item の行は基準と byte 一致 |
| (5) replay | `..._dispatch_match_replay_and_rollback` | 記録の `thread_id`・`run_id` から入力を組み直し、`input_digest` が合う run を TS に通すと `commands_digest` が記録と同じ |
| (6) 再起動 | `..._daemon_restart_without_bundle_restores_rust` | `[script]` なし・配備先に無い bundle の daemon で `cos_digest` が rust に戻る（`system:script-startup`）。digest は Rust のまま書かれ、記録は増えない |

support の `Env` に `with_config` を足した。config は `[cos] enabled = false` にする。既定の `true` のままだと、requeue した item で本物の CoS run を起動しうるため。digest pass は CoS が無効でも走る。

## 受け入れ条件と証拠（提出時の HEAD で実行）
- 条件 0（`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で全件通る）: `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_compare_cos_digest` → 5 passed、0 skipped（3 回続けて同じ）。
  - dist を外すと、`CELERIS_SCRIPT_REQUIRE_BUNDLE` なしでは bundle を使う 3 件が SKIP を出して抜ける。`=1` では 3 件が失敗する（残り 2 件は file の中の本文を使うので通る）。
- 条件 1（compare が全行で一致）: `node control-ts/scripts/compare.mjs cos_digest` → `2083/2083 matched`（cases 2,066 行 + traces 17 行）、exit 0。
- `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 bash scripts/dev/test-affected.sh` → exit 0（対象 crate は celeris・celerisctl、nextest 875 passed・0 skipped）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p celeris --test script_compare_cos_digest -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `python3 scripts/dev/check-architecture-map.py` → OK。範囲は `sh "$CELERIS_WU_SCOPE_PATHS"` で試験 2 file と `docs/architecture-map.md` だけ。

## commit（件名）
- phase s4: host-adapter: 本物の deno_core host で cos_digest の replay と compare を確かめる試験を足す
- phase s4: host-adapter: 進捗の記録を足す

## 未解決事項
- `cargo clippy -p celeris --tests -- -D warnings` は `campaign_lock_e2e` の compile error（`support_cases/mod.rs` を複数回 load）で止まる。この unit はその試験に触れていない。必須の検査（`--workspace`）は test target を含まないので通る。
- (6) の daemon の記録は、遅い host の 50 ms の `timeout` も許す（daemon の中では新しい DB でやり直せないため）。結線の確認が目的で、一致は (1)(2) で確かめている。
- completed の run に `running` のまま残った item が退避された場合に replay が drifted になるかは未確認（試験の場面に無い）。

## 提案
- close unit で付記 S4-5 の replay に書き足す: stopped・interrupted の run は requeue で item の `run_id` が消えるので、組み直した入力の `input_digest` が必ず合わず、replay は常に `drifted` になる（試験で確認した）。本番の log の replay で確かめられるのは requeue の無い run だけで、requeue の判断の同等性は fixture の replay（cases・traces）で確かめる。
