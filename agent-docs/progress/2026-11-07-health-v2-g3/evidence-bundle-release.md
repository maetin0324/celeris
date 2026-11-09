---
title: 健全性v2 G3 evidence bundle-release — release に repo health の検査器を同梱する
tasks: [01M6YJF7HJ9J0EQHRM5SN5TWV4]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 evidence bundle-release — release に repo health の検査器を同梱する

完了日: 2026-11-09

WorkUnit `bundle-release`（task 01M6YJF7HJ9J0EQHRM5SN5TWV4 の段 core）。[ADR 2026-11-08 健全性の強制ループ](../../adr/2026-11-08-health-enforcement-loop.md) の D1.4 を selfdeploy に実装した。daemon が同梱の所在を求める部分は後続の unit `bundle-path` が行う。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 変更
- `scripts/selfdeploy/lib.sh` に `sd_bundle_repo_health <build の git 作業ツリー> <stage の repo-health dir>` を足した。
  - build した commit（`<build>` の `HEAD`）から `git archive` で `scripts/dev/repo-health/` と `scripts/dev/source-size-report.py` を出し、`<dest>` の下に repo 相対の path のまま展開する。`task-dispatch` の `health_review::policy::with_base_checker` が base から出すときと同じ取り出し方（`git archive` → `tar -x`）・同じ相対配置なので、daemon は `<release>/repo-health/scripts/dev/repo-health/repo_health.py` を base の版と同じ形で起こせる。
  - 展開の後に `scripts/dev/repo-health/profiles/` を消す（Celeris の profile を他の repo に当てない）。写す file は「`profiles/` を除く全部」で、今の commit では `repo_health.py`・`health/`・`rules/`・`schema/`・`validate_report.py` に加えて `ci.sh`（G3 ci-entry が足した CI の入口）も入る。
  - `<dest>/CHECKER_TREE` に 2 行を書く: `repo-health <HEAD:scripts/dev/repo-health の tree id>`、`source-size-report <HEAD:scripts/dev/source-size-report.py の blob id>`。
  - 写せないとき（`HEAD` に `scripts/dev/repo-health/` の dir・`repo_health.py`・`source-size-report.py` が無い、git でない、archive・展開・書き込みの失敗）は `sd_die`（exit 1）する。`<dest>` は毎回消してから作るので、同じ sha の作り直しで古い file が残らない。
- `scripts/selfdeploy/release.sh`: `sd_bundle_selfdeploy_scripts` の直後に `sd_bundle_repo_health "$BUILD" "$STAGE/repo-health"` を呼び、CHECKER_TREE の 2 行を log に出す。失敗は `sd_die` なので、release dir（`<sha12>`）は作られない（`<sha12>.partial` は次の release.sh が消す。`sd_bundle_selfdeploy_scripts` の失敗と同じ扱い）。
- `scripts/selfdeploy/tests/repo_health_bundle.sh`（新規、bash、引数なし）: 一時 git repo の fixture に検査器の file と `profiles/` を commit し、関数を呼んで次を確かめる。本物の release.sh は起動しない。
  1. release.sh が `sd_bundle_repo_health "$BUILD" "$STAGE/repo-health"` を呼ぶ（`bundle_includes_verify_helpers.sh` と同じ形の grep）
  2. 写した 8 file が repo 相対の配置にあり、内容が commit の blob と同じ（`git hash-object` と `git rev-parse HEAD:<path>` の一致）。作業ツリーの未 commit の変更・未追跡の `__pycache__`・検査器の外の file は入らない。実行 bit が残る
  3. `profiles/` が無い
  4. CHECKER_TREE がちょうど 2 行で、`git rev-parse HEAD:scripts/dev/repo-health`・`git rev-parse HEAD:scripts/dev/source-size-report.py` と一致する
  5. 同じ dest に 2 回目を呼ぶと前回の file が残らない
  6. `source-size-report.py` が無い・`repo_health.py` が無い・`scripts/dev/repo-health/` が無い・git でない、の 4 通りで exit 1 し、理由に path が出る
  7. 最後に `bash -n` で release.sh と lib.sh の構文を検査する
- 既存の release の試験 4 本（`release_web_required.sh`・`browser_ledger_release_stages.sh`・`release_web_bundle_requires_deps.sh`・`release_parallel_test_gate.sh`）の fixture の repo に `scripts/dev/repo-health/repo_health.py` と `scripts/dev/lib/test-tmp-lifecycle.sh` を足した。
  - 前者が無いと、この変更で release.sh が同梱の段で `sd_die` する。
  - 後者は base の時点で既に欠けていた（`test-parallel.sh` が ADR 2026-11-06-test-runner-tmp-lifecycle で `scripts/dev/lib/test-tmp-lifecycle.sh` を読むようになったが fixture が写していない）。4 本とも base で cargo-test の段で落ちており、同梱の段まで届かなかった。直したので 4 本とも release dir の作成まで通る。
  - `release_parallel_test_gate.sh` の成功の場合に、作った release の `repo-health/` に `repo_health.py`・`source-size-report.py` があり、CHECKER_TREE の 1 列目が `repo-health`・`source-size-report` であることの確認を足した（偽の build で release.sh 全体を通したときに同梱が入ることを守る。中身の照合は `repo_health_bundle.sh`）。

### 足した試験が守る不変条件
- 守る条件: release には build した commit の検査器（`profiles/` を除く）が base と同じ相対配置で入り、CHECKER_TREE はその commit の tree id・blob id と一致する。写せなければ release は作られない。
- 既存の試験で検出できない理由: `bundle_includes_verify_helpers.sh` は `scripts/selfdeploy` の平らな同梱だけを見る。release を通す 4 本は fixture に検査器が無く、base では cargo-test で止まっていた。

## ADR とのずれ
- ADR D1.4 は「その commit の tree id を `<release>/repo-health/CHECKER_TREE` に書く」とする。実装は commit の tree id ではなく、`repo-health <scripts/dev/repo-health の tree id>` と `source-size-report <scripts/dev/source-size-report.py の blob id>` の 2 行を書く。理由: ADR D2.2 は `checker.sha256` を「`git rev-parse <base>:scripts/dev/repo-health` の tree id と `scripts/dev/source-size-report.py` の blob id を domain `celeris.repo-health.checker/1` で sha256」と定め、「同梱の版は CHECKER_TREE の値から同じ式で計算する」とする。commit の tree id からはこの 2 つの id を git なしで導けないので、daemon（git の無い release dir を読む）が base の版と同じ値を計算できるよう、式の入力の 2 つをそのまま書く。同じ commit の検査器なら base でも同梱でも `checker.sha256` が同じ値になる（ADR D2.2 の要求）。
- ADR D1.4 の写す file の列挙（`repo_health.py`・`health/`・`rules/`・`schema/`・`validate_report.py`）は ADR を書いた時点のもの。実装は「`profiles/` を除く全部」を写すので、その後に足された `ci.sh` も入る。CHECKER_TREE の tree id は `profiles/` を含む `scripts/dev/repo-health` 全体の id で、base の版の式（`git rev-parse <base>:scripts/dev/repo-health`）と同じ入力になる。
- ADR (8) の表は selfdeploy の path として `^scripts/selfdeploy/release\.sh$`・`^scripts/selfdeploy/tests/[a-z0-9_.-]+$` だけを挙げ、`scripts/selfdeploy/lib.sh` は無い。lib.sh に関数を置いた理由: release.sh の同梱の手順は `sd_bundle_selfdeploy_scripts`（lib.sh）と同じく lib.sh の関数にしてあり、試験が release 全体を起こさず関数だけを一時 dir に対して呼べる（`bundle_includes_verify_helpers.sh` と同じ形）。release.sh に直接書くと、release.sh を起動しないと試せない。この unit の計画の範囲 check は `scripts/selfdeploy/(release|lib)\.sh` を許している。
- 既存の release の試験 4 本の fixture の変更は `scripts/selfdeploy/tests/` の下で、ADR (8) の表の `^scripts/selfdeploy/tests/[a-z0-9_.-]+$` は下位 dir を許さないが、変えたのは `tests/` 直下の file だけ（`tests/fixtures/` は変えていない）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
| コマンド | 結果 |
|---|---|
| `bash scripts/selfdeploy/tests/repo_health_bundle.sh` | exit 0、`OK: repo_health_bundle: 8 checker files bundled at repo-relative paths; profiles/ excluded; CHECKER_TREE matches git rev-parse; missing checker dies; release.sh and lib.sh pass bash -n`（0.24 秒） |
| `bash -n scripts/selfdeploy/lib.sh` | exit 0 |
| `bash scripts/selfdeploy/tests/bundle_includes_verify_helpers.sh` | exit 0 |
| `bash scripts/selfdeploy/tests/release_web_required.sh` | exit 0（`release_web_required: ok`。base では cargo-test で exit 1） |
| `bash scripts/selfdeploy/tests/browser_ledger_release_stages.sh` | exit 0（`PASS: browser_ledger_release`。base では exit 1） |
| `bash scripts/selfdeploy/tests/release_web_bundle_requires_deps.sh` | exit 0（`release_web_bundle_requires_deps: ok`。base では exit 1） |
| `bash scripts/selfdeploy/tests/release_parallel_test_gate.sh` | exit 0（`release_parallel_test_gate: ok`。base では exit 1） |
| 試験の検出力（`$TMPDIR` の写しを変えて実行） | `profiles/` を消さない → `FAIL: profiles/ was bundled`、release.sh の呼び出しを消す → `FAIL: release.sh does not bundle …`、CHECKER_TREE を 1 行にする → `FAIL: CHECKER_TREE is wrong`、lib.sh に構文の誤り → exit 1 |

`test-affected.sh`・`cargo clippy`・`cargo fmt --check` の結果は報告（result.json の evidence）に書く。

## 未解決
- daemon が `<release>/repo-health/` を見つけて gate に設定するのは unit `bundle-path`、CHECKER_TREE から `checker.sha256` を計算するのは unit `core-types`・`checker-gate`。この unit は同梱と CHECKER_TREE の書式までを行う。
- 同梱は、この変更を含む最初の release から入る。それより前の release の daemon には `<release>/repo-health/` が無い（ADR の「release」節のとおり）。

## 提案
- `scripts/selfdeploy/tests/*.sh` の release を通す試験は、どの検査（test-affected.sh・release gate）からも呼ばれていないため、fixture の欠け（`test-tmp-lifecycle.sh`）が気付かれずに残っていた。selfdeploy の shell 試験を release gate か test-affected.sh（`scripts/selfdeploy/` が変わったとき）から呼ぶことを提案する。
