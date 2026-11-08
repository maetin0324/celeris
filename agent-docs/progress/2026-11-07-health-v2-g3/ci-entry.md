---
title: 健全性v2 G3 ci-entry — CI から呼ぶ入口 scripts/dev/repo-health/ci.sh
tasks: [01M6Y8EG6QN8S421XYJNVDCPTA]
status: done
updated: 2026-11-08
---
# 健全性v2 G3 ci-entry — CI から呼ぶ入口 scripts/dev/repo-health/ci.sh

完了日: 2026-11-08（人が決定 ci-entry-policy-merge で policy の変更を承認し、operator がこの branch を main に no-ff merge した。merge の後の確認は WorkUnit `land`）

WorkUnit `ci-script`（task 01M6Y8EG6QN8S421XYJNVDCPTA の unit `ci-entry`）。[ADR 2026-11-07 健全性を設計と review に組み込む](../../adr/2026-11-07-health-review-integration.md) の D6.1〜D6.6 に従う。`scripts/dev/repo-health/ci.sh` は Celeris profile の `policy_paths`（`^scripts/dev/repo-health/`）に当たる policy の file なので、この branch は policy の file を変える branch になる。前例は [G2 profiles](../2026-11-07-health-v2-g2/guideline-profiles.md) と [G2 review-policy-cli](../2026-11-07-health-v2-g2/dispatch/review-policy-cli.md)。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。導入手順の文書は後続の unit `ci-guide` が書く。

## 変更
- 件名「G3 ci-entry: CI の入口 scripts/dev/repo-health/ci.sh とゲートの試験 (7)」の commit
  - `scripts/dev/repo-health/ci.sh`（新規、POSIX sh、dash で動く）: `sh scripts/dev/repo-health/ci.sh --base <rev> [--head <rev>] [--out <report.json>]`。
    - `--help`（`-h`）は使い方を stdout に出して exit 0。`--base` が無い・値の無い option・未知の引数は使い方を stderr に出して exit 2。
    - repo は script の場所から決める（`git -C <script の dir> rev-parse --show-toplevel`。check-health.sh と同じ）。repo の外から呼んでもよい。
    - head は `git rev-parse --verify --quiet <rev>^{commit}` で sha にし（既定 `HEAD`）、`repo_health.py --repo <root> --base <rev> --head <sha> --policy-version 1 --out <out>` を呼ぶ。作業ツリーは見ないので、report の `snapshot.head.kind` は commit、`summary.reviewable` は base に届けば true。
    - `--out` の既定は `${TMPDIR:-/tmp}/repo-health-ci.<pid>.json`。前の report を確かめないよう、呼ぶ前に書き先を消す。report の path を `ci.sh: report=<path>` で stdout に出す。
    - `validate_report.py` で report を確かめ、合わなければ exit 2。report が無い（使い方誤り・non-Git など）・report の `summary.exit_code` と repo_health.py の exit code が違う、も exit 2。
    - report の `snapshot.base.reachable` が false で `snapshot.shallow` が true なら、`git -C <root> fetch --deepen=<n>` か `git -C <root> fetch --unshallow` の後に再実行する手順を stderr に出して exit 2。base が branch 名なら remote と branch を付けて fetch する例も出す。
    - それ以外は repo_health.py の exit code（0/1/2。契約 D3）をそのまま返す。check-health.sh の D6.2（policy の file だけの変更を exit 0 にする扱い）はしない。
  - `scripts/dev/tests/repo_health_gate.sh`: 場合 (7) を足した。一時 repo で
    - `--help` が exit 0、`--base` なしが exit 2 で使い方を stderr に出す。
    - (7a) app.py の変更を commit し、未 commit の `BAD`（作業ツリーを見れば doc.links が落ちる file）を置いて `--base main` → exit 0。report は validate_report.py を通り、`reviewable` true、head は commit で sha は `git rev-parse HEAD`、dirty false、doc.links pass、構造の検査が走った。
    - (7b) `BAD` を commit → exit 1、doc.links fail。
    - (7c) `git clone --depth 1` の写しの ci.sh を repo の外から呼び、`--base` に写しに無い commit → exit 2、stderr に `git -C … fetch --deepen=`、report は validate_report.py を通り `shallow` true・`base.reachable` false。
    - 成功の行は `OK (7) ci.sh: ...`。
- report/1 の schema・rule・profile、`repo_health.py`・`validate_report.py`・`health/` は変えていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `dash scripts/dev/tests/repo_health_gate.sh` → exit 0。(1)〜(6) に続き `OK (7) ci.sh: commit head pass -> exit 0 (reviewable, ignores uncommitted), committed fail -> exit 1, shallow -> exit 2 with git fetch --deepen`。
- 試験が効くことの確認（手元で一時に ci.sh を書き換え、元に戻した）: `--head "$head_sha"` を `--head worktree` に変えると (7a) が exit 1 で落ちる。
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → 73 件 OK、exit 0。
- `sh scripts/dev/repo-health/ci.sh --base HEAD~1`（この repo の HEAD の commit を検査）→ exit 0、verdict pass、`reviewable=true`、head は commit。`--help` → exit 0、`--base` なし・未知の引数・値の無い `--base` → exit 2。
- `sh scripts/dev/repo-health/ci.sh --base main`（この branch の HEAD の commit を検査）→ exit 1、`reviewable=true`。fail 級の finding は `policy.changed`（`scripts/dev/repo-health/ci.sh`）の 1 件だけ。
- `bash scripts/dev/check-health.sh` → exit 1。必須 rule のうち pass でないのは `policy.changed`（fail）だけ（ほかの 14 件は pass）。fail 級の finding は `policy.changed` の 1 件で、`policy.files` のうち base と head の blob が違うのは `scripts/dev/repo-health/ci.sh`（base に無い）だけ。`fail_grade=1 new=1 worsened=0`。D6.2 は働かない（policy の file でない gate の試験と進捗も変えている）。
- `bash scripts/dev/test-affected.sh` → exit 1。最初に呼ぶ check-health.sh が同じ `policy.changed` だけで止まる（`--list` の対象は 0 件。crate と web は変えていない）。exit 1 は `policy.changed` だけによる（D6.6）。
- `bash scripts/dev/check-health.sh --base HEAD`（この branch の HEAD を base にする。branch を main に merge した後の main に当たる）→ exit 0、`policy.changed` pass、doc-only。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → この進捗、`scripts/dev/repo-health/ci.sh`、`scripts/dev/tests/repo_health_gate.sh` の 3 件だけ。`repo_health.py`・`validate_report.py`・`health/`・`schema/`・`rules/`・`profiles/` の差分は無い。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0（Rust は変えていない）。

## main への merge（WorkUnit `land`）
- 人は決定 ci-entry-policy-merge で「承認する。operator がこの branch を main に no-ff merge してから再開する」を選んだ。
- operator の merge commit の件名は「integrate 01M6Y8EG6Q: CI の入口 scripts/dev/repo-health/ci.sh（policy の branch、人の承認済み。ADR health-review-integration D6.6）」。main の前の先端（件名「integrate wu/reclose-docs (phase close3)」）とこの branch の先端（件名「integrate wu/ci-script (phase impl)」）の 2 親を持つ。
- `git merge-base --is-ancestor "$(git log -1 --format=%H -- scripts/dev/repo-health/ci.sh)" main` → exit 0（件名「G3 ci-entry: CI の入口 scripts/dev/repo-health/ci.sh とゲートの試験 (7)」の commit は main の祖先）。`git cat-file -e main:scripts/dev/repo-health/ci.sh` → exit 0。
- merge の時点で `git diff --stat HEAD main` は差分 0（この branch と main の検査器は同じ）。main は取り込んでいない。

## merge の後の検査（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `bash scripts/dev/check-health.sh`（test-affected.sh の最初の段）→ exit 0。base は `git merge-base HEAD main`（件名「integrate wu/ci-script (phase impl)」の commit）。変更はこの進捗だけなので doc-only の軽い経路: verdict pass、`policy.changed` は pass（report の `policy.changed` は false）、rule 21 件のうち pass でない 10 件は not_applicable（fail は 0 件）、`fail_grade=0 new=0 worsened=0`。この進捗を書く前（base との差分 0）の実行でも exit 0 で、rule 21 件がすべて pass だった。merge の前の exit 1（`policy.changed` fail）が解けた。
- `bash scripts/dev/test-affected.sh`（`CELERIS_WU_BASE` を外し、基準を `git merge-base HEAD main` にした）→ exit 0、`test-affected: 試験対象の変更なし（docs のみ等）`。`--list` の対象は 0 件（`CELERIS_WU_BASE` のままでも 0 件）。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/tests/repo_health_gate.sh` → exit 0、`OK (1)`〜`OK (7)` の 7 行と `repo_health_gate: all checks passed`（doc-only の check-health.sh はこの試験を省くので、別に走らせた）。
- 文書検査 3 本（`bash scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`・`sh scripts/dev/progress-index.sh --check`） → 3 本とも exit 0。

## final review の policy_mixed と親の branch の main 追従（WorkUnit `parent-sync`）
- 差し戻しの reason: final review の criterion 5 が `health:policy_mixed: 01M2XPE3NZ1FZE061FKDY4A9PK: policy files [scripts/dev/repo-health/ci.sh] changed together with other files [agent-docs/progress/2026-11-07-health-v2-g3/ci-entry.md, scripts/dev/tests/repo_health_gate.sh]; move the policy change into its own task` で fail になった（criterion 0〜4・6〜8 と criterion 5 のほかの 2 件は pass）。
- 原因: final review は review 前の同期の target と候補の diff から health を作る。この task の target は親 G3 task（01M6TYVJSWEY3ZE50S02ETYDNM）の branch `celeris/01M6TYVJSWEY3ZE50S02ETYDNM` である（`crates/task-dispatch/src/dispatcher/behind_target.rs` の `review_target_ref`）。親の branch は、ci.sh を main に入れる前の commit（件名「integrate wu/reclose-docs (phase close3)」）のままだった。そのため、main に入った ci.sh が gate の試験・進捗と同じ差分に出て、`crates/task-dispatch/src/health_review/verdict.rs` の `policy_mixed`（ADR health-review-integration D6.1）が fail を出した。main を基準にすれば差分は進捗 1 file だけだった。
- 人の決定 parent-follow-main: 「承認する。operator が親 G3 の worktree で `git merge --ff-only` を実行してから答える（推奨どおり）」。
- operator の操作: 親 G3 の worktree（作業ツリーは clean、run は無し）で `git merge --ff-only` を実行し、親の branch を main の merge commit（件名「integrate 01M6Y8EG6Q: CI の入口 scripts/dev/repo-health/ci.sh（policy の branch、人の承認済み。ADR health-review-integration D6.6）」）まで進めた。この unit は親の branch を merge せず、main も取り込んでいない。

### 検査（提出時の HEAD で実行。基準は `git merge-base HEAD refs/heads/celeris/01M6TYVJSWEY3ZE50S02ETYDNM`）
- 親の branch が ci.sh を含む: `git merge-base --is-ancestor "$(git log --format=%H --diff-filter=A -1 HEAD -- scripts/dev/repo-health/ci.sh)" refs/heads/celeris/01M6TYVJSWEY3ZE50S02ETYDNM` → exit 0（件名「G3 ci-entry: CI の入口 scripts/dev/repo-health/ci.sh とゲートの試験 (7)」の commit は親の branch の祖先）。`git cat-file -e refs/heads/celeris/01M6TYVJSWEY3ZE50S02ETYDNM:scripts/dev/repo-health/ci.sh` → exit 0。親の branch の先端は上の operator の merge commit で、main の先端と同じ。`git diff --quiet refs/heads/celeris/01M6TYVJSWEY3ZE50S02ETYDNM HEAD -- scripts/ crates/` → exit 0。
- 親の branch から見た差分: merge-base は件名「integrate wu/ci-script (phase impl)」の commit。`git diff --stat <merge-base> HEAD` は進捗 1 file（この file）だけ。
- review と同じ base..head の検査: `sh scripts/dev/repo-health/ci.sh --base "$(git merge-base HEAD refs/heads/celeris/01M6TYVJSWEY3ZE50S02ETYDNM)" --head HEAD --out "$TMPDIR/parent-sync-health.json"` → exit 0、`verdict=pass reviewable=true fail_grade=0 new=0 worsened=0`、doc-only の軽い経路。report の `policy.changed` は false、`policy.changed` rule は pass、validate_report.py を通った。
- `bash scripts/dev/test-affected.sh`（基準は `$CELERIS_WU_BASE`）→ exit 0。最初の check-health.sh は doc-only で verdict pass、`policy.changed` pass、`fail_grade=0 new=0 worsened=0`。続けて `test-affected: 試験対象の変更なし（docs のみ等）`。
- 文書検査 3 本（`bash scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`・`sh scripts/dev/progress-index.sh --check`）→ 3 本とも exit 0。final review の criterion 4 の `sh scripts/dev/check-adr-numbers.sh` → exit 0（410 files）。
- この unit が変えた file はこの進捗だけ（`crates/` と `scripts/` は変えていない）。

## 未解決
- 解けた: merge の前の「必須検査 `test-affected.sh` と `check-health.sh` は main に入るまで `policy.changed` で exit 1」（D6.6）は、上の merge で解けた。
- D6.2 は働かない: この branch は policy の file（ci.sh）のほかに、ゲートの試験（`scripts/dev/tests/repo_health_gate.sh`）とこの進捗を変える。どちらも policy の file でない（前例 review-policy-cli の「未解決」と同じ）。
- ci.sh は深さの不足を report の `snapshot.base.reachable` が false であることで知る。repo_health.py は base と head の tree 同士を比べるので、base の commit が clone にあれば、間の履歴が無くても深さを求めない。

## 提案
- 後続の `ci-guide` の文書では、CI の checkout の深さ（全履歴か、base まで届く深さ）を前提条件に書く。
- 子 task の policy の branch を main に merge したら、親の branch も同じ merge commit まで進める。手順: operator が main に no-ff merge した直後に、親 task の worktree が clean で run が無いことを確かめ、`git -C <親の worktree> merge --ff-only <main の merge commit>` を実行し、`git merge-base --is-ancestor <main の merge commit> <親の branch>` が 0 であることを確かめる。親の branch が main から分かれていて ff できないときは、人の決定を置いてから親の branch に main を merge する。この手順を operator の手順書（ADR health-review-integration D6.6 の節）に足す。
- 別案: dispatcher が子の review の差分から、main に入った policy の file の変更を除く。子の候補の policy の file の blob が main の blob と同じなら、`policy_mixed` の判定でその file を数えない。手順が要らなくなるが、review の差分の作り方（`review_target_ref`）と health の入力の両方を変えるので、ADR の付記で決めてから実装する。
