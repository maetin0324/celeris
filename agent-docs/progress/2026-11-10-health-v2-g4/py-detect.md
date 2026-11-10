---
title: 健全性v2 G4 py-detect — isolated Git fixture と検出試験
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: blocked
updated: 2026-11-10
---
# 健全性v2 G4 py-detect — isolated Git fixture と検出試験

完了日: 未完了（検査器の能力不足を plan_issue で申告）。基準は `$CELERIS_WU_BASE`。

## 実装前 brief
1. 根本原因: rule 単体試験では、非 Celeris の Git 変更を profile 解決から report 集約まで実証できない。
2. owner: `scripts/tests/repo_health_fixtures.py` と `test_repo_health_generic.py`。検査器の owner は `scripts/dev/repo-health/` だが今回の変更範囲外。
3. 再利用: 既存の検査器・validator・Git 一時 repo の作法。実装内容は ADR health-generic-fixtures の範囲。
4. 選択肢: inline fixture の写しを増やさず、Python import と CLI が同じ builder を呼ぶ方式を採用。
5. 新しい状態・分岐: fixture の base/head、kind と variant、期待ラベル。fallback・製品の状態は追加しない。
6. 撤去対象: 無し。既存試験・policy は変更しない。
7. 不変条件: 一時 repo だけを書く。実ユーザー repo・ネットワーク・LLM を使わない。report の根拠と集約を本物の validator で確認し、検査器は file を変更しない。

## 変更
- `phase 1: add isolated generic health fixtures and detection tests` — ts・rust・multi と6 variant の builder、6種の試験を追加。CLI は ADR の JSON を出す。multi の scalar base/head は先頭の ts repo を表し、各 repo は main/candidate で解決する。
- profile 無しは組み込み generic、profile 有りは fixture の base にある最小 profile で検査する。generic の依存逆転も同じ検出条件で評価する（現行は層が無く未達）。静的な複製候補の検出も確認する。
- SQL の書き手・型の同形検出・逆向き依存の file:line を確認する。security の独立検証は base の理由付き例外で維持し、vendor/generated は fail 級にならず file も残ることを確認する。
- 既存試験は rule 単位のため、この試験は profile 解決・完全な report・複数 repo の Git 履歴という別の不変条件を守る。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- CLI `python3 -I scripts/tests/repo_health_fixtures.py build <kind> <variant> <dest>` — 3 kind × 6 variant = 18回すべて exit 0。profile 無しも関数で生成し、本物の検査器を当てた48 report は validator がすべて exit 0。run の artifacts/matrix.json と各 report に記録。
- `python3 -m unittest discover -s scripts/tests -p test_repo_health_generic.py` — exit 1、6 tests、12 subtest failures。good・security・vendor/generated の3種は合格。複製契約・状態の2種で各4件の不一致（ts・rust・multi の両 repo）、profile 無しの依存逆転でも4件の不一致。
- `cargo clippy --workspace -- -D warnings` — exit 0。
- `cargo fmt --all -- --check` — exit 0。
- `bash scripts/dev/test-affected.sh --list` — exit 0、対象 crate 無し。
- `sh scripts/dev/check-doc-links.sh` — exit 0、ok。
- `sh scripts/dev/check-adr-numbers.sh` — exit 0、414 files。
- `sh scripts/dev/progress-index.sh --check` — exit 0、ok。
- `sh "$CELERIS_WU_SCOPE_PATHS"` と許可 path の照合 — exit 0、変更は builder・汎用試験・本進捗の3 path のみ。base profile の policy_paths と変更 path の積集合は0。`git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/dev/check-health.sh` は exit 0、差分無し。`git diff --check` も exit 0。
- `bash scripts/dev/test-affected.sh` — 最終版で exit 1。health 本体は verdict pass、fail_grade=0、policy.changed 0件。architecture-map の1 test は OK、続く既存を含む84 tests では新規の12 subtest が上記の能力不足で失敗。crate の試験へ進む前に停止した。全体 nextest・doc test 一括・web e2e・実 browser/userns は実行していない。

## 未解決
- 再現: `build ts dup-contract <dest>` または `build rust dup-contract <dest>` 後、本物の repo_health.py に `--repo <dest> --base main --head candidate --policy-version 1 --out <report>` を渡す。`dup-state` でも同じ。
- 期待: ADR (2) の dup-contract・dup-state は required な finding が new/error になり exit 1、verdict fail。
- 実際: required=true でも finding は candidate=true、severity=warning、delta=new、exception=null。exit 0、verdict pass、fail_grade=0。名前の異なる Session と SessionView、sessions の2 module の書き手は evidence 付きで検出されるが blocking にはならない。
- 原因の所在: `rules/dup_contract.py`・`rules/dup_state.py` が candidate=True/default_severity=warning を宣言し、engine が candidate の severity を warning に制限する。required は rule の集約への参加を決めるだけで候補を error にしない。現行 report schema も candidate=true の error を禁止する。
- profile 無しの dep-inversion も期待は根拠付きの依存逆転検出と fail 級判定。実際は `deps.violation` が no_layers_declared で not_applicable、finding 0件。TS は exit 0、Rust は Cargo の循環が別 rule `deps.cycle` で exit 1 になるが、層の逆転の根拠は無い。profile 有りでは両言語・multi とも deps.violation を fail 級で検出する。profile を必要条件に変えて試験を通すことはしていない。
- 試験の期待は維持した。検査器・policy・baseline は変更していない。能力不足は許可範囲外なので、本 WU は完了としない。

## 提案
- 別の policy だけの branch で、静的候補を blocking にする条件を設計・承認して実装するか、ADR の期待と現行契約の矛盾を人が解決する。required=true だけで安全に断定できるとは未確認。一般的な候補を一律 error に変更する判断はこの WU では行わない。
- 後続 py-bypass の builder variant 追加は既存 builder を拡張し、fixture の内容を別 file に複製しない。

## 再実行（2026-11-10、run attempt 1）
- 前回 check の失敗を先に再現した。`python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_generic*.py' -v` は exit 1、6 tests、12 subtest failures（96.660秒）。good・security-double・vendor-generated は合格。失敗の内訳と原因は上の「未解決」と一致し、検査器の契約は変わっていない。
- `doc.links` は builder の文字列にある一時 repo の文書パスを、この worktree の参照として検査していた。生成先の文書パスを `Path` の結合で表した。fixture の内容と試験の期待は変えず、検査器の除外リストも変更していない。`sh scripts/dev/check-doc-links.sh scripts/tests/repo_health_fixtures.py` は exit 0（ok）。
- 実装前 brief の補足: 原因 owner は fixture の文書パスの表記。既存の `Path` と `write()` を再利用する。検査器への特例追加は採らない。新しい状態・分岐・fallback・port と撤去対象は無し。一時 repo の文書内容を維持し、CLI と実検査器の試験で確認する。
- `cargo clippy --workspace -- -D warnings` は exit 0（vendored deno_core の既存 deprecated warning 1件）。`cargo fmt --all -- --check` は exit 0。
- この再実行でも完了にはしない。必要な修正は検査器・policy の owner にあり、本 WU の許可範囲外。期待を弱めず plan_issue を再申告する。
- CLI の全18組（3 kind × 6 variant）を別々の一時 dir で再実行し、全件 exit 0、各 repo の main/candidate が存在することを確認した。
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` は exit 1、84 tests、12 subtest failures（58.570秒）。新規試験の上記12件以外の失敗は無い。
- `sh scripts/dev/progress-index.sh --check` は exit 0（ok）。`sh "$CELERIS_WU_SCOPE_PATHS"` は builder・汎用試験・本進捗の3 path のみ。`git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/dev/check-health.sh` は exit 0、検査器・policy に差分無し。
- `bash scripts/dev/test-affected.sh --list` は exit 0、対象 crate 無し。`bash scripts/dev/test-affected.sh` は exit 1。health report は verdict=pass、fail_grade=0（既存 finding 717件、新規・悪化0件）、続く architecture-map の1 test は OK、health の84 tests は上記12件で失敗。crate 試験には進んでいない。全体試験は実行していない。
- 再実行の commit: `phase 1: fix fixture document reference and record checker limitations`。提出時の HEAD の内容で上記検査を実行した。最後の同期は無い。

## 再現確認（2026-11-10、run 01M73A3RN4T6K80K0BE67B4TPT）
- 前回の check を同じ cwd で実行した。`python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_generic*.py' -v` は exit 1、6 tests、12 subtest failures（9.935秒）。良い変更・security・vendor/generated は合格。複製契約・状態の configured 各4件、profile 無しの依存逆転4件が失敗した。
- 原因を本体でも確認した。`dup_contract.py` と `dup_state.py` は candidate=True/default_severity=warning、`health/engine.py` は candidate の error を warning に制限する。`rules/deps.py` は層の宣言が無い場合 no_layers_declared とする。fixture の調整で解消する不備ではない。
- 検査器・policy を変えない制約と期待を維持する指示に従い、コード・試験は変更しない。同じ失敗の再試行を続けず plan_issue とする。別の policy 専用 branch で blocking 条件と generic の層宣言要件を解決し、その成果を取り込んでから再開する必要がある。
- 今回は Phase 未完了。全体試験・LLM・外部ネットワーク・実ユーザー repo の操作は実行していない。
- もう一方の指定 check `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` も exit 1、84 tests、同じ12 subtest failures（33.434秒）。`git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/dev/check-health.sh` は exit 0（差分無し）。進捗書式検査・`git diff --check` は exit 0。今回の commit は `phase 1: record repeated generic checker capability blockers`。最後の同期は無い。
