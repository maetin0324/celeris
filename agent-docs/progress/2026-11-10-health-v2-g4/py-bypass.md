---
title: 健全性v2 G4 py-bypass — 判定とすり抜けの試験（Python）
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: blocked
updated: 2026-11-10
---
# 健全性v2 G4 py-bypass — 判定とすり抜けの試験（Python）

完了日: 未完了。7 試験のうち 5 試験は合格した。2 試験は検査器の能力不足 2 件（下の「未解決」の G1・G2）で落ちる。どちらの不足も合格にはならない側に倒れる（fail-closed）が、判定の種類が違う。検査器は許可範囲の外なので plan_issue で申告した。基準は `$CELERIS_WU_BASE`。

## 実装前 brief
1. 根本原因: (d) の判定と (f) のすり抜けを、Celeris でない repo の履歴の上で本物の検査器を使って確かめる試験が無い。rule 単体の試験は偽の rule か inline の repo を使っている。
2. 責務 owner: fixture は `scripts/tests/repo_health_fixtures.py`（builder、ADR (1)）、試験は `scripts/tests/test_repo_health_generic_bypass.py`（ADR (3)）。判定の owner は検査器 `scripts/dev/repo-health/` で、範囲の外（変えない）。
3. 既存の再利用先: builder の `build`・`git`・`commit`・`write`、検査器の CLI、`validate_report.py`、`--print-review-policy`。
4. 選択肢: policy を弱めた profile を variant ごとに文字列で写す案は採らなかった。同じ `profile()` に引数 `weaken`・`tools` を足して、base と head を 1 つの関数から作る（fixture の内容は builder の 1 か所だけにある）。
5. 新しい状態・分岐・fallback・port: 無し。builder の variant を足しただけ。
6. 撤去対象: 無し。
7. 不変条件: 候補の弱化は base 側の policy で評価され、`policy.changed` と `deps.violation` の fail 級が残る。各弱化は、base に入れれば違反を隠せることを対照で示す（何も変えない編集で合格する試験にしない）。

## 変更
- `phase 1: add generic bypass fixtures and judgement tests` — builder に variant を足した: docs-only・non-git・unsupported-tool・tool-failure・policy-exclude・policy-rule-off・policy-exception・policy-threshold・baseline-rewrite・history-smuggle。`profile(kind, weaken=, tools=)`、`violation_fingerprint(kind)`。base に policy の file `size-baseline.json` を足した（全 variant 共通）。試験 file を新規に作った。
- ADR (2) の表との差:
  - tool-failure は表に無い variant で、包んだ検査の故障に使う。
  - unsupported-tool と tool-failure は base の profile に包んだ検査の command を足す。そのため base は kind ごとに 1 つ、という前提の例外になる。
  - 弱化 5 種と tool の 2 種は profile 無しでは作れないので、`ValueError` にした。
  - docs-only の判定は ADR が書く `inputs.doc_only = true` という欄ではなく、report の契約の欄 `inputs.mode = "doc-only"` で確かめた。

## 試験と結果（提出時の HEAD で実行）
| 試験 | 確かめること | 結果 |
|---|---|---|
| `test_generic_docs_only_not_applicable` | ts・rust・multi、profile 有り・無し: `inputs.mode=doc-only`、構造 rule 5 つが `not_applicable`（`doc_only`）、exit 0 | 合格 |
| `test_generic_non_git_error` | `.git` の無い dir（親も repo でないことを先に確かめる）: exit 2、`not_a_git_repo`、report なし | 合格 |
| `test_generic_unsupported_required_not_pass` | good の変更でも、無い tool（`adr.numbers`）と無い script（`doc.links`）の必須 rule が unsupported で exit 2、fail 級 0 | 不合格（G2） |
| `test_generic_tool_failure_is_error` | 包んだ検査の出力が UTF-8 でない → `error`（`crashed:doc.links`）・exit 2。adapter が例外 → `deps.*` が `error`・exit 2。`health/` の module を欠いた写し → exit 2・report なし | 包んだ検査と adapter は合格。module 欠けは不合格（G1） |
| `test_generic_policy_weakening_fails` | 弱化 5 種 × ts・rust・multi: exit 1、`policy.source=base`、`policy.changed` と `deps.violation`（required、fail 級）が残る。`--print-review-policy` は head を checkout した状態でも base の profile の digest を出す。対照: 弱化を base に入れると `deps.violation` が止めない | 合格 |
| `test_generic_history_smuggle_fails` | candidate が 2 commit（逆転→無関係）: `--base main` と `--base auto` は base の sha になり、`deps.violation` は new・fail 級で exit 1。対照: 途中の commit を base にすると exit 0 | 合格 |
| `test_generic_binding_tracks_candidate_base_policy` | candidate・base・head の policy・base の policy を順に変えると、head の sha・base の sha・`policy.files` の blob・review policy の `definition_sha256` が、変えたものだけ変わる | 合格 |

## 証拠（提出時の HEAD で実行）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_generic_bypass.py' -v`: exit 1。7 tests、5 subtest failures（G1 1 件、G2 4 件）。所要約 25 秒。
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'`: exit 1。92 tests。失敗は上の 5 subtest だけで、py-detect の 7 試験を含む既存の試験は合格。所要約 124 秒。
- CLI `python3 -I scripts/tests/repo_health_fixtures.py build <kind> <variant> <dest>`: 3 kind × 12 variant の 36 回がすべて exit 0。non-git の JSON は `base`・`head` が null。
- `git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/dev/check-health.sh size-baseline.json`: exit 0。検査器・policy・baseline に差分は無い。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"`: report は verdict=pass、fail_grade=0、new 0。script 自体は exit 1。続けて走る `test_repo_health*.py` が G1・G2 で落ちるため。
- `sh scripts/dev/check-doc-links.sh`: exit 0。builder の壊れた検査の出力に repo の文書 path を書かないようにした（py-detect と同じ件）。

## 未解決
- G1（module を欠いた検査器）
  - 再現: `scripts/dev/repo-health/` を `$TMPDIR` に写す。写しから `health/scan.py` を消す。`build ts good <dest>` で作った repo に、その写しの `repo_health.py --repo <dest> --base main --head candidate --policy-version 1 --out <report>` を当てる。
  - 期待: exit 2、report なし。根拠は ADR health-generic-fixtures (3) の (d) と、契約の exit code（1 は必須 rule の fail に限る。検査器の例外は error）。
  - 実際: `ModuleNotFoundError` の traceback で exit 1、report なし。
  - 影響: Celeris の gate は report が無いので Error（`report_missing`）にし、合格にはしない。CLI と `check-health.sh` は exit 1 を fail として報告し、error と区別できない。
  - owner: `repo_health.py` の import と `main`（policy の file）。
- G2（包んだ検査の script が無い）
  - 再現: `build ts unsupported-tool <dest>`（rust・multi も同じ）に本物の検査器を当てる。
  - 期待: `doc.links`（base の profile の command `sh tools/check-docs.sh`。script は無い）が `unsupported`（`command_missing`）。根拠は ADR (2) の unsupported-tool の行と、`rules/existing_checks.py` の docstring「script の file が無い（exit 126・127）は unsupported」。
  - 実際: `fail`（fail 級の finding 1 件）。`/bin/sh` は dash で、読めない script には exit 2 を返し、126・127 にならない。同じ fixture の `adr.numbers`（PATH に無い tool）は期待どおり `unsupported` で、verdict は error・exit 2。
  - 影響: 合格にはならないが、Celeris は Fail として repair に回し、無い script を「直す対象の finding」として渡す。
  - owner: `rules/existing_checks.py`（policy の file）。
- 試験の期待は弱めていない。検査器・policy・baseline は変えていない。

## 観察（契約どおりで、不足とはしない）
- 包んだ検査が signal で終わると（`kill -9 $$`）、`doc.links` は `fail` になる。契約の rule 表は包んだ検査を「既存 script の非 0 exit を finding に写す」と定めるので、試験の期待には入れなかった。error に分けるかは G2 と同じ policy の review で決めるとよい。
- profile の無い repo では、候補が足した profile が bootstrap として head の側で評価される（契約 D2 の初回）。弱化 5 種は base の profile を前提にするので、profile 無しでは作らない。

## 提案
- G1・G2 を、policy だけの branch の子 task（G2 の D6、exc-expiry と同じ形）で直す。
  - G1: `repo_health.py` の import と実行の失敗を exit 2 にして、stderr に理由を出す。
  - G2: command の script の有無を起動の前に確かめるか、shell の違いに依らない判定に直す。
  - 直した後、この 2 試験が合格すれば、py-bypass の受け入れを満たす。
- record は ADR (2) の表に次の 2 点を付記する: tool-failure の variant と、docs-only の欄名 `inputs.mode`。
