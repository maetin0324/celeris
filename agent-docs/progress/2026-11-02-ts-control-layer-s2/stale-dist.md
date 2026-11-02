---
title: S2 stale-dist — 本物の host の試験が古い control-ts/dist を使って落ちないようにする
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 stale-dist

## 結論
- 本物の host の試験は、manifest の `domains` が `control-ts/bundle.config.json` の `domains` の key 集合と違う bundle、または試験が要る領域を含まない bundle を「古い bundle」として SKIP する。`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら失敗にする。
- 変更は `crates/celeris/tests/` の 3 file だけで、本番コード（`crates/script-runtime` など）は変えていない。

## 原因
integrate-second の test-affected で `script_compare_route_priority.rs` の 2 件が落ちた。
- `real_host_matches_rust_on_all_fixtures`: `assert manifest.domains に route_priority` で失敗。
- `dispatch_match_rollback_and_replay`: `TsError: unknown domain: route_priority`。

task 根の worktree に git 管理外（`control-ts/.gitignore` の `dist/`）の古い `control-ts/dist` が残っていた。その manifest の `domains` は `host_selftest` と `lane_rules` だけで、route_priority を足す前の source で作った bundle だった。`support/script_compare.rs` の `dist_bundle` は読めれば使うので、古い bundle を正として比較した。

## 直した判定
- `usable_bundle(dir, config, test, domain, require)`（`support/script_compare.rs`）: bundle を検証して読み、manifest の `domains` の集合が config の `domains` の key 集合と一致し、かつ `domain` を含むときだけ `Some`。違えば理由（manifest の domains・source_commit・config の domains）を `eprintln` して `None`。`require` が真なら panic。
- `dist_bundle(test, domain)`: `control-ts/dist` と `control-ts/bundle.config.json` で `usable_bundle` を呼ぶ。`require` は `CELERIS_SCRIPT_REQUIRE_BUNDLE=1`。
- 呼び出しを lane_rules（3 か所）と route_priority（2 か所）の両方で直した。

## 足した試験（名前に stale_dist を含む、`script_compare_route_priority.rs`）
tempdir に検証の通る bundle と、domains を変えた manifest、3 領域を宣言した config を置く。
- `script_compare_stale_dist_missing_domain_is_skipped`: domains が `host_selftest`・`lane_rules` だけなら route_priority でも lane_rules でも `None`。
- `script_compare_stale_dist_matching_domains_is_used`: 揃っていれば `Some`（`require` 真でも）。
- `script_compare_stale_dist_unknown_domain_is_skipped`: config に無い領域を要る試験は `None`。
- `script_compare_stale_dist_required_panics`: `require` 真で古い bundle は panic（`stale bundle`）。

## 確認（提出時の HEAD で実行）
この WU の worktree で `pnpm install --offline` と `pnpm bundle` を実行して `control-ts/dist` を作った（git 管理外）。task 根の worktree の dist には触っていない。

| コマンド | 結果 |
|---|---|
| `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_compare_route_priority --test script_compare_lane_rules` | 14 件合格、0 件 skip |
| dist の manifest の domains を一時的に `host_selftest`・`lane_rules` に書き換えて同じ 2 件を実行（確認後に戻した） | REQUIRE なし: 2 件とも SKIP（stale bundle の理由付き）。REQUIRE=1: 2 件とも `stale bundle` で panic |
| `bash scripts/dev/test-affected.sh` | exit 0、870 件合格、0 件 skip |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo clippy -p celeris --test script_compare_route_priority --test script_compare_lane_rules -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |

## commit
- phase s2: stale-dist: 古い control-ts/dist を SKIP する判定と stale_dist 試験

## 未確認の点
- task 根の worktree の古い dist は残ったまま。integrate-second ではその dist の試験が SKIP になり、REQUIRE=1 のときだけ失敗する。本物の host で比べるには、統合の前に dist を作り直す必要がある（作業ツリー外の操作なのでこの WU ではしていない）。

- `cargo clippy -p celeris --tests -- -D warnings` は、この WU が触っていない `campaign_lock_e2e` の `clippy::duplicate_mod` で落ちる（base から在る。workspace の clippy は tests を見ないので通る）。

## 提案
- integrate の check で `pnpm bundle` を先に走らせるか、`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` の試験の前に dist を作り直す手順を置く。
