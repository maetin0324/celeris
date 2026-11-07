---
title: 健全性v2 G1 rules-structure — 依存違反・循環と巨大責務の悪化
tasks: [01M6TYVJSRM3SFQ3VHWQE4MZ9M]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G1 rules-structure — 依存違反・循環と巨大責務の悪化

完了日: 2026-11-07

WorkUnit `rules-structure`（G1 の工程「rule の実装」）。基準は `$CELERIS_WU_BASE`。契約は [ADR 2026-11-07 汎用 repo health の契約](../../adr/2026-11-07-repo-health-contract.md) D5・D6、中核は [core の進捗](core.md)。

## 結論
- `scripts/dev/repo-health/rules/deps.py` に `deps.violation`・`deps.cycle`、`rules/size_growth.py` に `size.responsibility-growth` を置いた。3 つとも必須（既定 required）で、静的推定の候補ではない（candidate=false）。
- 依存の辺は `health/lang_deps.py` が作る。B0 の `module-deps-measure.py` の方法（use の展開・src 直下の module 単位・Tarjan）を汎用化した。B0 の file は変えていない。
- 構造指標の本体の行・試験の範囲・block の終わりは `scripts/dev/source-size-report.py` の関数を `health/source_size.py` から import して使う（二重実装なし）。`source-size-report.py` は WU の範囲外なので変えない（初回の提出で `rust_test_mask` を切り出したが、範囲の check が `scripts/dev/source-size-report.py` を範囲外として落としたため戻した）。行ごとの `#[cfg(test)]` mask は `analyze_rust_file` が返さないので、`health/source_size.py` が同じ判定（`CFG_TEST_RE`・`MOD_DECL_RE`・`block_end` は import）で作り、試験の行数が `analyze_rust_file(lines).inline_test_lines` と一致することを毎回確かめる。食い違えば `Unsupported(source_size_mask_mismatch)` にする。
- この repo への実行（既定 profile）: deps.violation は not_applicable（層の宣言が無い）、deps.cycle は existing 18 件で pass、size.responsibility-growth は existing 104 件で pass。

## rule の決めごと
- 単位: Rust は package の `src/` 直下の第 1 要素（`<pkg>/src/store.rs` と `<pkg>/src/store/**` が 1 単位）、Python・TypeScript は file、Cargo は package。package は tree の `Cargo.toml` の `[package]` から見つける（`cargo metadata` を使わないので外部 tool 不要で、base の tree にも同じ方法を当てる）。
- B0 との違い: crate root（lib.rs・main.rs・bin）を単位にしない。`crate::X` の X が module でなければ root の再 export とみなし辺を作らない（B0 では lib を通る循環が出ていた）。`#[cfg(test)]` の範囲（source-size と同じ判定）と、試験・fixture・生成の file は辺の元にしない。
- deps.violation: profile の `[[rules."deps.violation".layers]]`（name・paths・allow）。辺の元と先の path を層に当て、許されない向きを finding にする（Cargo の dev-dependencies は `include_dev` で opt-in）。層が無ければ not_applicable（`no_layers_declared`）。設定の誤り（未知の欄・未知の層）は unknown で、合格にしない。
- deps.cycle: 強連結成分（2 単位以上）。Rust の use は同じ package の中だけ、package をまたぐ循環は Cargo の辺で見る。TypeScript の `import type` は既定で数えない。head の循環は節点を最も多く共有する base の循環と同じ fingerprint にする。節点が増えれば worsened、縮んだ・分かれただけなら existing、共有が無ければ new。
- size.responsibility-growth: 指標は file_lines（既定は source-size の production 閾値 2000）、module_lines（8000）、type_methods（60）、type_fields（40）、fn_lines（200）。値が閾値を超えた単位を finding にし、base と比べる（engine の D6）。
- 分割だけの見かけの改善: module・型・関数の鍵は file の path でなく module（Python は dir）と名前で作る。`foo.rs` を `foo/mod.rs`・`foo/more.rs` に分けても module foo の行と型 Foo の method 数は同じ fingerprint・同じ値で existing のまま残る。module_lines は空行・注釈・use・mod 宣言を数えないので、分割で増える宣言は悪化にならない。
- 汎用の既定値はコードにあり、Celeris 固有の層・上限は celeris-profile WorkUnit が `profiles/celeris.toml` に書く。`profiles/default.toml` は変えていない。

## 中核に足したもの
- `health/rule.py` に `NotApplicable` 例外、`health/engine.py` でそれを not_applicable にする（層の宣言が無いときに pass と報告しないため）。
- `scripts/tests/test_repo_health_core.py` の写しの rules/ を core の rule（policy_changed.py）だけにした。本物の deps.cycle が試験用の rule と同じ ID で二重登録になるため。rules-dup が同じ箇所を直していれば統合で揃える。

## 変更（commit の件名）
- `phase G1: rule deps.violation・deps.cycle・size.responsibility-growth と試験`
- `phase G1: rules-structure の進捗`
- `phase G1: source-size-report.py を戻し、cfg(test) の mask を repo-health 側で作って行数を照合する`

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → exit 0（32 tests。structure 10 件: Python 循環の新規（path と行）、既存循環の放置（existing・pass）、循環の拡大（worsened）、Rust の module 循環と再 export・cfg(test) の除外、層違反の new/existing と層なしの not_applicable、Cargo の層違反と crate 循環・dev 依存の除外、設定誤りの unknown、閾値の new/worsened/existing、分割だけの見かけの改善（同じ fingerprint の existing）と分割＋悪化（worsened）、inline 試験の除外）
- `python3 scripts/dev/repo-health/repo_health.py --repo . --base "$(git merge-base HEAD main)" --head HEAD --policy-version 1 --profile scripts/dev/repo-health/profiles/default.toml --rules deps.violation,deps.cycle,size.responsibility-growth --out <artifacts>/self-head.json` → exit 0（約 21 秒、policy=bootstrap）。`validate_report.py` → 1 valid
- `python3 -m unittest discover -s scripts/tests -p 'test_source_size*.py'` → 60 tests OK
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set G1 …/slices.json` → write-set G1: ok
- `bash scripts/dev/test-affected.sh` → exit 0（crate の変更なし）
- `bash scripts/dev/check-health.sh` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0

## 未解決
- 解析は文字列の静的解析で、macro が作る path・条件付き import・動的 import・TypeScript の path alias（`@/…`）は拾わない。Rust の型・関数の範囲は rustfmt の書式が前提（source-size と同じ）。
- Python の型・関数の鍵は dir 単位（自由関数は file 単位）なので、Python の自由関数を別 file に移すと新しい単位になる。
- この変更での `test-affected.sh` の実行には `test_repo_health*.py` が入らなかった（走ったのは scripts の unittest 60 件など）。上の unittest を別に実行した。

## 提案
- `source-size-report.py` に行ごとの mask を返す関数（`rust_test_mask`）を置き、`health/source_size.py` の写しの判定を消す（write_set に source-size-report.py を含む task で行う）。
- celeris-profile WorkUnit で、`profiles/celeris.toml` に crate の層（B0 の module-deps.json の crate_deps に合う向き）と size の上限を書き、`policy_paths` に `scripts/dev/source-size-report.py` を足す（rule がこの file の分類を使うため）。
- `test-affected.sh` の対象に `scripts/dev/repo-health/` の変更 → `test_repo_health*.py` を足す（G1 の write_set の外なので別 task）。
