---
title: 健全性v2 G1 rules-dup — 重複・未リンク・移行残骸の候補 rule
tasks: [01M6TYVJSRM3SFQ3VHWQE4MZ9M]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G1 rules-dup — 重複・未リンク・移行残骸の候補 rule

完了日: 2026-11-07

WorkUnit `rules-dup`（G1 の工程「rule の実装」）。基準は `$CELERIS_WU_BASE`。契約は [ADR 2026-11-07 汎用 repo health の契約](../../adr/2026-11-07-repo-health-contract.md) の D4・D5。

## 結論
- `scripts/dev/repo-health/rules/` に 6 rule を足した: `dup_contract.py`・`dup_state.py`・`dup_public_surface.py`・`link_unlinked.py`・`dup_test_fixture.py`・`migration_remnant.py`。共通の部品（設定の読み取り・対象 file の列挙・comment 除去）は `health/scan.py`。
- 6 rule とも `candidate=True`・既定 severity warning・既定 required false（ADR D5）。profile の `required = true` で必須化できる（必須でも候補は error にならないので fail にはならず、評価の失敗だけが verdict error になる）。file を読むだけで、削除・書き換えはしない。
- 既存負債との差分は core の仕組み（base と head の両側評価、fingerprint の突き合わせ）を使う。fingerprint の鍵は名前・表名・path から作り、行番号を含めない。value（定義の数・書き手の数・出現数）が base より増えれば worsened。
- 例外は core の `[[rules."<id>".exceptions]]`（path か fingerprint と reason）でそのまま付く。security 境界の独立検証・provider 固有 wire・migration/replay の除外を試験で確かめた。
- Celeris 固有の名前・語彙・閾値はコードに書いていない（既定は言語の一般の慣習だけ）。Celeris 向けの値は celeris-profile WorkUnit が `profiles/celeris.toml` に書く。

## rule ごとの方法
- dup.contract: rust の `pub struct/enum/type`・python の top level の `class`・typescript の `export interface/type/class/enum` の同名（同じ言語で 2 file 以上）と、field の名前の集合が同じ（既定 3 個以上）で名前の違う型。`*.schema.json` の `title` の同名。既定は product の file だけ。B0 の dup-candidates-scan.py の same_name_types を言語一般に広げた。
- dup.state: SQL の書き込み文（INSERT/REPLACE INTO・UPDATE … SET・DELETE FROM）の表ごとに書き手の module（既定は親 dir）を数え、2 以上で候補。既定は大文字の keyword だけ、product の file だけ（migration・試験は数えない）。
- dup.public-surface: http route（axum の `.route`、`app.get(` などの形）・cli subcommand（argparse の `add_parser`、clap の `#[command(name = …)]`）・行頭の `pub fn`／`export function` の同名が 2 module 以上。route の引数は `{}` に揃える。
- link.unlinked: pub item の名前が追跡 file 全体で定義の 1 回だけ（B0 の unreferenced_pub と同じ数え方）、`script_paths`（既定 `^scripts/`）・`doc_paths`（既定 `^docs/`）の file の名前が自分以外の file に出ない（B0 の unreferenced_scripts と同じ）。README・index・main などは入口として数えない。
- dup.test-fixture: 試験・fixture の file の行の集合（8 文字以上の行、21 file 以上に出る定型行は除く）の Jaccard 係数が 0.9 以上の組を連結成分にまとめる。
- migration.remnant: profile の語彙 `terms`（既定 deprecated・legacy・backward-compat・`use_new_*` 型の切替 flag）と `retired_paths` を product のコードから拾う。(file, 語) ごとに 1 件。

## 変更（commit の件名）
- `phase G1: 候補 rule 6 本（dup.contract・dup.state・dup.public-surface・link.unlinked・dup.test-fixture・migration.remnant）と試験`
- `phase G1: rules-dup の進捗`
- `phase G1: rules-dup の試験の文書 path を docs 参照検査に掛からない名前にする`（check-doc-links が試験の文字列 docs/… を実在検査したため。core と同じ対処）
- core の試験（`scripts/tests/test_repo_health_core.py`）は検査器の写しに試験用の `link.unlinked` を足すため、本物の rule が入ると二重登録で落ちた。写しから `policy_changed.py` 以外の本物の rule を外す 4 行を足した（上の 1 つ目の commit に含む）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → exit 0（37 tests。うち dup 15 件: 6 rule の正例・負例、例外 5 件（path 4・fingerprint 1）、existing・worsened、候補の file を書き換えない（内容の hash・git status・HEAD が変わらない）、必須化、設定の誤りは error（crashed:dup.state:ConfigError、exit 2）、doc-only は not_applicable）
- `python3 -I scripts/dev/repo-health/repo_health.py --repo . --base $CELERIS_WU_BASE --head HEAD --policy-version 1 --profile scripts/dev/repo-health/profiles/default.toml --rules dup.contract,dup.state,dup.public-surface,link.unlinked,dup.test-fixture,migration.remnant --out <artifacts>/self-a.json` → exit 1（6 rule とも pass・optional。findings 601: dup.contract 211、dup.state 24、dup.public-surface 92、link.unlinked 188、dup.test-fixture 4、migration.remnant 81、policy.changed 1。exit 1 は policy.changed の fail で、この WorkUnit が `scripts/dev/repo-health/` を変えたため。設計どおり）。所要約 27 秒。
- 同じ呼び出しを 2 回 → `cmp` で byte 一致。`validate_report.py` → 1 valid
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set G1 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → write-set G1: ok。`sh "$CELERIS_WU_SCOPE_PATHS"` の path は rules/・health/scan.py・2 つの試験・この進捗だけ
- `bash scripts/dev/check-health.sh` → exit 0（source-size gate 0 failure）
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（crate の変更なし。check-health の scripts 試験 60 件 OK。test-affected は `test_repo_health*` を走らせないので、上の unittest を別に実行した）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 候補の誤検出: rules/ の module（engine が dir から読む）は名前で参照されないので link.unlinked の script 候補になる。migration.remnant は自分の語彙の定義を拾う。どちらも celeris-profile で `ignore_paths` か例外に理由を付けて扱う。
- dup.public-surface の http は router の prefix（nest）を解かない。同じ相対 path を別の prefix に付けた route も候補になる。

- test-affected・check-health.sh は `scripts/tests/test_repo_health*.py` を走らせない。check-health.sh への接続は celeris-profile WorkUnit の範囲。

## 提案
- celeris-profile WorkUnit で `profiles/celeris.toml` に次を置く: dup.contract の `ignore_names`（Args・Error など慣習の名前）、dup.state の `ignore_tables` と TaskStore 以外の書き手の例外（理由付き）、link.unlinked の `ignore_paths = ['^scripts/dev/repo-health/rules/']`、migration.remnant の `retired_paths`（消した config 欄・旧 script）。
