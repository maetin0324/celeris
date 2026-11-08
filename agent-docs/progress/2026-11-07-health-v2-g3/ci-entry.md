---
title: 健全性v2 G3 ci-entry — CI から呼ぶ入口 scripts/dev/repo-health/ci.sh
tasks: [01M6Y8EG6QN8S421XYJNVDCPTA]
status: running
updated: 2026-11-08
---
# 健全性v2 G3 ci-entry — CI から呼ぶ入口 scripts/dev/repo-health/ci.sh

完了日: 未完（policy の変更の人の承認と、operator によるこの branch の main への no-ff merge を待つ）

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
- `bash scripts/dev/check-health.sh` → exit 1。必須 rule のうち pass でないのは `policy.changed`（fail）だけ（ほかの 14 件は pass）。fail 級の finding は `policy.changed` の 1 件で、`policy.files` のうち base と head の blob が違うのは `scripts/dev/repo-health/ci.sh`（base に無い）だけ。`fail_grade=1 new=1 worsened=0`。D6.2 は働かない（policy の file でない gate の試験と進捗も変えている）。
- `bash scripts/dev/test-affected.sh` → exit 1。最初に呼ぶ check-health.sh が同じ `policy.changed` だけで止まる（`--list` の対象は 0 件。crate と web は変えていない）。exit 1 は `policy.changed` だけによる（D6.6）。
- `bash scripts/dev/check-health.sh --base HEAD`（この branch の HEAD を base にする。branch を main に merge した後の main に当たる）→ exit 0、`policy.changed` pass、doc-only。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → この進捗、`scripts/dev/repo-health/ci.sh`、`scripts/dev/tests/repo_health_gate.sh` の 3 件だけ。`repo_health.py`・`validate_report.py`・`health/`・`schema/`・`rules/`・`profiles/` の差分は無い。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0（Rust は変えていない）。

## 未解決
- 必須検査 `test-affected.sh` と `check-health.sh` は、この branch が main に入るまで `policy.changed` で exit 1 になる（D6.6 の想定どおり）。task の計画では、人が policy の変更を承認した後、operator がこの branch を main に no-ff merge する（unit `land` が merge の後の確認と進捗の done を受け持つ）。
- D6.2 は働かない: この branch は policy の file（ci.sh）のほかに、ゲートの試験（`scripts/dev/tests/repo_health_gate.sh`）とこの進捗を変える。どちらも policy の file でない（前例 review-policy-cli の「未解決」と同じ）。
- ci.sh は深さの不足を report の `snapshot.base.reachable` が false であることで知る。repo_health.py は base と head の tree 同士を比べるので、base の commit が clone にあれば、間の履歴が無くても深さを求めない。

## 提案
- 後続の `ci-guide` の文書では、CI の checkout の深さ（全履歴か、base まで届く深さ）を前提条件に書く。
