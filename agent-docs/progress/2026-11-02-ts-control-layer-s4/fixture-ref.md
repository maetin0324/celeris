---
title: S4 fixture-ref — 本物の host の試験が消えた traces.jsonl を読まないように直す
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S4 fixture-ref — 本物の host の試験が消えた traces.jsonl を読まないように直す

完了日: 2026-11-03（unit fixture-ref）。変えたのは試験 1 file（`crates/celeris/tests/script_compare_cos_digest.rs`）だけで、製品コードは変えていない。

## 原因
a-verify の統合の check 0・1・5 が落ちた。traces-async が `control-ts/fixtures/cos_digest/traces.jsonl` を消して `pass_traces.jsonl` へ置き換えた一方、real-host の `script_compare_cos_digest.rs` は `fixture_rows("traces.jsonl")` を 3 か所で読んでいた。そのため試験 4 件が『No such file or directory』で panic した。

## 直した点
- 全 fixture の照合（`script_compare_cos_digest_real_host_matches_rust_on_all_fixtures`）は `cases.jsonl` の全行（2,000 行以上を assert、現在 2,066 行）を正順・逆順で流す。doc comment も直した。
- `warm` と `warm_pass` は `cases.jsonl` の最初の行を使う。
- `pass_traces.jsonl` はこの試験では読まない。replay は `compare.mjs` と task-dispatch の `cos_digest_pass_trace` が受け持つ。
- 試験の件数は変わらない（9 件）。mode ts の apply・fallback・再起動・restore_modes の試験本体は変えていない。

## traces.jsonl の削除
この unit の worktree は a-verify の統合（traces-async を含む）の上にあり、`traces.jsonl` は始めから無かった。計画の想定（real-host の上で traces.jsonl が残る）と違うので、`git rm` は行っていない。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `test ! -e control-ts/fixtures/cos_digest/traces.jsonl` と `grep -rn 'fixture_rows("traces\.jsonl")' crates/celeris/tests/`: file は無く、grep は 0 件。`crates/celeris/tests/` には文字列 `traces` 自体が残っていない。
- bundle を作り直した後の `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris -E 'test(/script_compare_cos_digest/)'`: exit 0。9 passed。bundle が無ければ panic する設定で通した（skip での通過は無い）。673 skipped は filter で外れた他の試験である。
- `bash scripts/dev/test-affected.sh`: exit 0。対象は celeris・celerisctl で、879 passed・0 skipped。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 未解決事項
- `control-ts/scripts/compare.mjs` の comment（10 行目）に `traces.jsonl` の名前が残る。`compare.test.mjs` の同じ名前は一時 directory に書く試験用の file で、消えた fixture は読まない。どちらもこの unit の範囲外（範囲 check は `crates/celeris/tests/`・`traces.jsonl`・`agent-docs/progress/` だけ）なので変えていない。

## 提案
- close-async で `compare.mjs` の comment の `traces.jsonl` を実態に合わせて直す。
