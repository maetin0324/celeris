---
title: 健全性v2 G4 py-bypass — 判定とすり抜けの試験（Python）
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: done
updated: 2026-11-10
---
# 健全性v2 G4 py-bypass — 判定とすり抜けの試験（Python）

完了日: 2026-11-10。試験は 9 件で、7 件が合格、2 件（G1・G2）が `@unittest.expectedFailure` の expected failure。unittest は `OK (expected failures=2)` で exit 0。G1・G2 は検査器の欠けで、どちらも合格にはならない側（fail-closed）に倒れる。ただし判定の種類が違う。人の決定 checker-gaps は a（G4 では直さず、expectedFailure と進捗に残し、G4 が main に入った後に policy だけの別 task で直す）。検査器・policy・baseline は変えていない。基準は `$CELERIS_WU_BASE`。

## 実装前 brief
1. 根本原因: (d) の判定と (f) のすり抜けを、Celeris でない repo の履歴の上で本物の検査器を使って確かめる試験が無い。rule 単体の試験は偽の rule か inline の repo を使っている。
2. 責務 owner: fixture は `scripts/tests/repo_health_fixtures.py`（builder、ADR (1)）、試験は `scripts/tests/test_repo_health_generic_bypass.py`（ADR (3)）。判定の owner は検査器 `scripts/dev/repo-health/` で、範囲の外（変えない）。
3. 既存の再利用先: builder の `build`・`git`・`commit`・`write`、検査器の CLI、`validate_report.py`、`--print-review-policy`。この run は前回の commit の builder と試験 file を作り直さずに使った。
4. 選択肢: G1・G2 の期待を元の試験に残したまま expectedFailure を付ける案は採らなかった。同じ試験の合格する assert（包んだ検査の故障・adapter の例外・PATH に無い tool）まで expected failure の中に隠れるため。期待を 2 つの新しい試験に移し、元の試験には今の検査器で成り立つ assert だけを残した。
5. 新しい状態・分岐・fallback・port: 無し。試験の helper `checker_without`（検査器の写しから module を 1 つ消す）と `env_with_adr_tool`（PATH の先頭に exit 0 の `orchard-adr-lint` を置いた環境）、`check()` の引数 `env` を足しただけ。
6. 撤去対象: 元の 2 試験にあった G1・G2 の期待（新しい 2 試験へ移した。弱めていない）。
7. 不変条件: 候補の弱化は base 側の policy で評価され、`policy.changed` と `deps.violation` の fail 級が残る。各弱化は、base に入れれば違反を隠せることを対照で示す。検査器の欠け（G1・G2）があっても、必須の検査が走らない変更は exit 0・verdict pass にならない。

## 変更
- `phase 1: add generic bypass fixtures and judgement tests`（前回の試行）— builder に variant を足した: docs-only・non-git・unsupported-tool・tool-failure・policy-exclude・policy-rule-off・policy-exception・policy-threshold・baseline-rewrite・history-smuggle。`profile(kind, weaken=, tools=)`、`violation_fingerprint(kind)`。base に policy の file `size-baseline.json` を足した（全 variant 共通）。試験 file を新規に作った。
- `phase 1: keep checker gaps G1 and G2 as expected failures`（この run）— 試験 file だけを変えた。
  - G1 の期待（module を欠いた写しで exit 2、report なし）を `test_generic_checker_import_failure_is_error` に移した。
  - G2 の期待（`doc.links` が `unsupported`・`command_missing`、fail 級 0、exit 2）を `test_generic_missing_script_unsupported` に移した。この試験は、そのままの fixture（`adr.numbers` の tool も無い）と、`orchard-adr-lint` を PATH に置いて無い script だけを残した場合の 2 通りで確かめる。後者は exit 2 が `doc.links` だけから来ることを確かめるため。
  - 2 試験とも `def` の直前の行に `@unittest.expectedFailure` を付け、docstring に欠けの理由・owner・この進捗の未解決 G1・G2 を書いた。直ったら unexpected success になり、外し忘れが分かる。
  - `test_generic_tool_failure_is_error` は、包んだ検査の故障と adapter の例外の assert をそのまま残した。module 欠けの写しは「report を書かない」と「exit 0 にならない」を通常の assert で確かめる。
  - `test_generic_unsupported_required_not_pass` は、PATH に無い tool（`adr.numbers`）が `unsupported`（`tool_missing:orchard-adr-lint`）で exit 2・verdict error になることを確かめる。さらに `orchard-adr-lint` を PATH に置き、無い script だけが残った run でも、exit 0 にも verdict pass にもならず、`doc.links` が pass・not_applicable にならないことを通常の assert で確かめる（`adr.numbers` が pass になったことも確かめ、tool を置いたことが効いていると示す）。
  - fixture の文書の path（`guide.md`・`notes.md`・`changelog.md` を `docs` の下に置く）を `str(Path('docs') / …)` で書いた。builder と同じ書き方で、`check-doc-links.sh` がこの repo の壊れた参照として数えないようにした。前回の記録の「`check-doc-links.sh` は exit 0」は、試験 file のこの 3 行について誤りだった（この run の開始時は 3 件の壊れた参照で exit 1）。
- ADR (2) の表との差（前回から変わらない）:
  - tool-failure は表に無い variant で、包んだ検査の故障に使う。
  - unsupported-tool と tool-failure は base の profile に包んだ検査の command を足す。そのため base は kind ごとに 1 つ、という前提の例外になる。
  - 弱化 5 種と tool の 2 種は profile 無しでは作れないので、`ValueError` にした。
  - docs-only の判定は ADR が書く `inputs.doc_only = true` という欄ではなく、report の契約の欄 `inputs.mode = "doc-only"` で確かめた。

## 試験と結果（提出時の HEAD で実行）
| 試験 | 確かめること | 結果 |
|---|---|---|
| `test_generic_docs_only_not_applicable` | ts・rust・multi、profile 有り・無し: `inputs.mode=doc-only`、構造 rule 5 つが `not_applicable`（`doc_only`）、exit 0 | 合格 |
| `test_generic_non_git_error` | `.git` の無い dir（親も repo でないことを先に確かめる）: exit 2、`not_a_git_repo`、report なし | 合格 |
| `test_generic_unsupported_required_not_pass` | PATH に無い tool の必須 rule が unsupported で exit 2・verdict error。無い script だけが残っても exit 0・verdict pass にならない | 合格 |
| `test_generic_missing_script_unsupported` | G2 の期待: 無い script の `doc.links` が `unsupported`（`command_missing`）、fail 級 0、exit 2 | expected failure（G2） |
| `test_generic_tool_failure_is_error` | 包んだ検査の出力が UTF-8 でない → `error`（`crashed:doc.links`）・exit 2。adapter が例外 → `deps.*` が `error`・exit 2。`health/` の module を欠いた写し → report なし・exit 0 でない | 合格 |
| `test_generic_checker_import_failure_is_error` | G1 の期待: module を欠いた写しで exit 2、report なし | expected failure（G1） |
| `test_generic_policy_weakening_fails` | 弱化 5 種 × ts・rust・multi: exit 1、`policy.source=base`、`policy.changed` と `deps.violation`（required、fail 級）が残る。`--print-review-policy` は head を checkout した状態でも base の profile の digest を出す。対照: 弱化を base に入れると `deps.violation` が止めない | 合格 |
| `test_generic_history_smuggle_fails` | candidate が 2 commit（逆転→無関係）: `--base main` と `--base auto` は base の sha になり、`deps.violation` は new・fail 級で exit 1。対照: 途中の commit を base にすると exit 0 | 合格 |
| `test_generic_binding_tracks_candidate_base_policy` | candidate・base・head の policy・base の policy を順に変えると、head の sha・base の sha・`policy.files` の blob・review policy の `definition_sha256` が、変えたものだけ変わる | 合格 |

## 証拠（提出時の HEAD で実行）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_generic_bypass.py' -v`: exit 0。`Ran 9 tests`、`OK (expected failures=2)`。expected failure は `test_generic_checker_import_failure_is_error` と `test_generic_missing_script_unsupported` だけ。所要約 24 秒。
- 2 試験の expectedFailure の印を外して走らせた結果（試験 file は変えず、`__unittest_expecting_failure__` を False にして実行）: 9 subtest failures。G2 は 8 件で、そのままの fixture では `'fail' != 'unsupported'`（ts・rust・multi の ts・multi の rust）、無い script だけでは `1 != 2`。G1 は 1 件で `1 != 2`（traceback は `exec_module`）。どちらも記録した欠けのとおりで、ほかの理由では落ちていない。
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'`: exit 0。`Ran 94 tests`、`OK (expected failures=2)`。所要約 44 秒。
- `git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/dev/check-health.sh size-baseline.json`: exit 0。検査器・policy・baseline に差分は無い。
- `sh scripts/dev/check-doc-links.sh`: exit 0（`check-doc-links: ok`）。

## 提出時の検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"`: exit 0。report は verdict=pass、fail_grade=0、new=0、worsened=0。続けて走る `test_repo_health*.py` は 94 tests、`OK (expected failures=2)`。
- `sh scripts/dev/progress-index.sh --check`: exit 0（`progress-index --check: ok`）。
- `python3 scripts/dev/check-human-text.py --root .`: exit 0（0 findings）。
- `bash scripts/dev/test-affected.sh`: exit 0。Rust の対象 crate は無し（`試験対象の変更なし`）。health の試験は 94 tests、`OK (expected failures=2)`、`repo_health_gate: all checks passed`。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 未解決
- G1（module を欠いた検査器）
  - 再現: `scripts/dev/repo-health/` を `$TMPDIR` に写す。写しから `health/scan.py` を消す。`build ts good <dest>` で作った repo に、その写しの `repo_health.py --repo <dest> --base main --head candidate --policy-version 1 --out <report>` を当てる。試験は `test_generic_checker_import_failure_is_error`。
  - 期待: exit 2、report なし。根拠は ADR health-generic-fixtures (3) の (d) と、契約の exit code（1 は必須 rule の fail に限る。検査器の例外は error）。
  - 実際: `ModuleNotFoundError` の traceback で exit 1、report なし。
  - 影響: Celeris の gate は report が無いので Error（`report_missing`）にし、合格にはしない。CLI と `check-health.sh` は exit 1 を fail として報告し、error と区別できない。
  - owner: `scripts/dev/repo-health/repo_health.py` の import と `main`（policy の file）。
- G2（包んだ検査の script が無い）
  - 再現: `build ts unsupported-tool <dest>`（rust・multi も同じ）に本物の検査器を当てる。`doc.links` だけを見るには、exit 0 の `orchard-adr-lint` を PATH の先頭に置く。試験は `test_generic_missing_script_unsupported`。
  - 期待: `doc.links`（base の profile の command `sh tools/check-docs.sh`。script は無い）が `unsupported`（`command_missing`）、fail 級 0、exit 2。根拠は ADR (2) の unsupported-tool の行と、`rules/existing_checks.py` の docstring「script の file が無い（exit 126・127）は unsupported」。
  - 実際: `doc.links` は `fail`（fail 級の finding 1 件）。`/bin/sh` は dash で、読めない script には exit 2 を返し、126・127 にならない。そのままの fixture では `adr.numbers`（PATH に無い tool）が `unsupported` なので verdict は error・exit 2 になる。無い script だけが残ると verdict は fail・exit 1。
  - 影響: 合格にはならないが、Celeris は Fail として repair に回し、無い script を「直す対象の finding」として渡す。
  - owner: `scripts/dev/repo-health/rules/existing_checks.py`（policy の file）。
- 試験の期待は弱めていない。検査器・policy・baseline は変えていない。

## 観察（契約どおりで、不足とはしない）
- 包んだ検査が signal で終わると（`kill -9 $$`）、`doc.links` は `fail` になる。契約の rule 表は包んだ検査を「既存 script の非 0 exit を finding に写す」と定めるので、試験の期待には入れなかった。error に分けるかは G2 と同じ policy の review で決めるとよい。
- profile の無い repo では、候補が足した profile が bootstrap として head の側で評価される（契約 D2 の初回）。弱化 5 種は base の profile を前提にするので、profile 無しでは作らない。

## 提案
- G4 が main に入った後、policy だけの別 task で G1・G2 を直し、2 試験の expectedFailure を外す。起票は operator が行う（人の決定 checker-gaps の回答）。
  - G1: `repo_health.py` の import と実行の失敗を exit 2 にして、stderr に理由を出す。
  - G2: command の script の有無を起動の前に確かめるか、shell の違いに依らない判定に直す。
  - 直した後、2 試験が unexpected success になるので、その task で `@unittest.expectedFailure` を消す。
- record は ADR (2) の表に次の 2 点を付記する: tool-failure の variant と、docs-only の欄名 `inputs.mode`。
