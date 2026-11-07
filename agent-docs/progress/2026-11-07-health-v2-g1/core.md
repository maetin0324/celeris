---
title: 健全性v2 G1 core — repo_health.py の中核
tasks: [01M6TYVJSRM3SFQ3VHWQE4MZ9M]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G1 core — repo_health.py の中核

完了日: 2026-11-07

WorkUnit `core`（G1 の工程「中核の実装」）。基準は `$CELERIS_WU_BASE`。契約は [ADR 2026-11-07 汎用 repo health の契約](../../adr/2026-11-07-repo-health-contract.md)。

## 結論
- `scripts/dev/repo-health/repo_health.py` を契約 D1 の CLI で実装した。report は `--out` にだけ書き、exit code は 0・1・2。report を書けない入力（non-Git・git が PATH に無い・base が解決できない非 shallow repo・policy 版の不一致・profile の誤り・未知の `--rules`）は stderr に理由語を出して exit 2。
- 中核は `scripts/dev/repo-health/health/` に分けた: `snapshot.py`（base・head の sha、dirty、shallow、submodule、変更 file と変更行）、`profile.py`（解決順 cli > repo > builtin、base 側読み・bootstrap、項目の検査）、`adapters.py`（言語と種類の分類）、`rule.py`（登録口）、`engine.py`（base・head の両側評価、delta、例外、集約、report）。
- rule は `scripts/dev/repo-health/rules/` の module が `RULES` で登録する。core が置いた rule は `policy.changed` だけ。他の rule（deps.*・size.*・dup.*・arch-map.*・既存検査の包み）は後続 WorkUnit が同じ登録口に足す。profile に書いてあるのに実装の無い rule は `unsupported`（`rule_missing:<id>`）で、合格にしない。
- 汎用の `profiles/default.toml` を置いた（policy.changed だけ。Celeris 固有の名前・上限は入れていない）。

## 契約の細部で決めたこと
- profile の項目の正本は `health/profile.py` の docstring。未知の項目・未知の rule ID・reason の無い例外・`policy.changed` への例外は読み込みエラー（exit 2）。`[rules."<id>"]` の `enabled`・`required`・`exceptions` 以外の欄は rule 固有の設定として rule に渡す。
- 例外の出典行は profile の `[[rules."<id>".exceptions]]` の見出し行。
- 言語: rust・python・shell・typescript（.js 系を含む）・markdown に adapter がある。go・java・c などコードの言語で adapter の無いものが diff に入ると、構造 rule は `unsupported`（`adapter_missing:<lang>`）。profile の `adapters.ignore_languages` で外せる（base 側の値）。
- 種類: markdown は常に docs。他は generated（先頭 10 行の marker か path）> fixture > migration > test > docs > product。profile の `[classes]` で path 規則を足せる。
- rule の対象の言語が head に 1 つも無ければ `not_applicable`（`no_target_files`）。rule が宣言した外部 tool が PATH に無ければ `unsupported`（`tool_missing:<name>`）。rule が投げた `Unknown` は unknown、`Unsupported` は unsupported、その他の例外は error（`crashed:<rule_id>:<例外の型>`）。
- 候補 rule（dup.*・link.unlinked・migration.remnant）の severity error は warning に下げる。
- `--rules` は手元の確認用。`policy.changed` は絞り込みで外れない。
- submodule は `snapshot.submodules` に記録し、変更 file・解析の対象から外す（中に入らない）。複数 repo は `--repo` が 1 つだけ受け付ける（repo ごとに実行する）。
- report の `metrics` に diff の file 数・行数と head の言語別・種類別の file 数を入れた（決定的な値だけ）。
- 対象 repo に `__pycache__` を作らない（`sys.dont_write_bytecode`）。作ると worktree が dirty になる。

## 変更（commit の件名）
- `phase G1: repo_health.py の中核（snapshot・profile・言語 adapter・diff・rule 登録口・集約）と試験`
- `phase G1: core の進捗`
- `phase G1: core の試験の文書 path を docs 参照検査に掛からない名前にする`（試験の文字列 docs/… を check-doc-links が実在検査したため）

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → exit 0（22 tests。core 18 件: 決定性・dirty・shallow（`git clone --depth 1 file://`）・doc-only・policy 弱化・bootstrap・tool 欠如（PATH 差し替え）・git 欠如・non-Git・版不一致・existing/new/worsened・例外・候補・adapter 欠如・crash/unknown・実装の無い rule・未知の profile 項目・`--rules`・submodule・分類）
- `python3 -I scripts/dev/repo-health/repo_health.py --repo . --base $CELERIS_WU_BASE --head HEAD --policy-version 1 --profile scripts/dev/repo-health/profiles/default.toml --out <artifacts>/self-head.json` → exit 0（policy=bootstrap、policy.changed は warning 1 件）。`validate_report.py` → 1 valid
- `git diff --name-only $CELERIS_WU_BASE -- crates web` → 空
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set G1 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → write-set G1: ok
- `bash scripts/dev/check-health.sh` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（crate の変更なし。scripts の unittest 60 件 OK）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- D2 の「検査器も base 側で読む」は、実行するのは手元の検査器で、検査器の file を head が変えれば `policy.changed` が fail になる形で守る。base の検査器 code を取り出して実行する仕組みは置いていない。
- `--rules` で必須 rule を外した report も schema 上は区別がつかない。review の証跡には `--rules` 無しの report を使う（G2 で reviewer に渡す時に決める）。

## 提案
- celeris-profile WorkUnit で `profiles/default.toml` を `extends = "celeris"` に替えるとき、`policy_paths` に既存検査の script と `scripts/dev/source-size-baseline.json` を足す。
