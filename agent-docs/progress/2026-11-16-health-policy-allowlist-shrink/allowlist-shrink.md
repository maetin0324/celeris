---
title: 許可の一覧の行を消すだけの変更を policy.changed に数えない（ADR・profile・engine・試験）
tasks: [01M7KATBGCRQRZ5NWXHXVWYPZ8]
status: blocked
updated: 2026-11-16
completed: 2026-11-16
---

# allowlist-shrink（WorkUnit）

完了日: 2026-11-16

人の決定（2026-11-16）: 許可の一覧（allowlist）の file から行を消すだけの変更は repo health の `policy.changed` に数えない。この branch は policy の file・その試験・ADR・進捗だけを変える policy の branch（先例 01M76BYGKY7R8SBZ22AZGJ72SP、[ADR 2026-11-07-health-review-integration](../../adr/2026-11-07-health-review-integration.md) D6.6）。`crates/`・`web/` は変えていない。

## 変更
- `health-policy-allowlist-shrink: 許可の一覧から行を消すだけの変更を policy.changed に数えない ADR を書き、契約 ADR D2 に付記する` — [ADR 2026-11-16-health-policy-allowlist-shrink](../../adr/2026-11-16-health-policy-allowlist-shrink.md)（7 項目の brief、判定 D2、許可の一覧の 3 つの読み手の確認、Rust を変えない理由 D4）と、[ADR 2026-11-07-repo-health-contract](../../adr/2026-11-07-repo-health-contract.md) の付記 2026-11-16。実装より先の commit。
- `health-policy-allowlist-shrink: profile の policy_allowlists に当たる許可の一覧の行を消すだけの変更を policy.changed に数えない` —
  - `scripts/dev/repo-health/health/profile.py`: top-level の任意の欄 `policy_allowlists`（`_TOP_KEYS`・docstring・`Profile.policy_allowlists`・`policy_paths`/`exclude_paths` と同じ正規表現の一覧の検証）。既定は空、組み込み generic には無い。
  - `scripts/dev/repo-health/health/engine.py`: 判定 `policy_changed_files`（補助 `_only_removes_lines`・`_content_lines`）の 1 か所。`build_report` の `policy.changed` と `Context.policy_changed` がこの結果を使う。`policy.files` は前と同じ全部の policy の file と両側の blob（schema は変えていない）。
  - `scripts/dev/repo-health/rules/policy_changed.py`: 自分で持っていた blob の比較を消し、`ctx.policy_changed` だけを読む。
  - `scripts/dev/repo-health/profiles/celeris.toml`: `policy_allowlists = ['^scripts/dev/(adr-allowed-duplicates\.txt|human-text-lint\.allow)$']`。
  - `scripts/tests/test_repo_health_allowlist_shrink.py`: 試験 10 件（下）。

## 試験の一覧（`scripts/tests/test_repo_health_allowlist_shrink.py`）
一時 dir の git repo に profile（`policy_paths = ['^scripts/dev/repo-health/', '^lists/']`、`policy_allowlists = ['^lists/allow\.txt$']`）と `lists/allow.txt`・`lists/other.txt` を置き、本物の検査器を `--rules policy.changed` で走らせ、毎回 `validate_report.py` に通す。
- 消すだけ → `policy.changed=false`・finding 無し・exit 0。`policy.files` の行の形は `path`・`base_blob`・`head_blob` のまま（`test_allowlist_shrink_removing_lines_is_not_policy_change`）
- `#` の注釈の書き換え・空行の追加・末尾の改行の削除 → false（`..._comment_and_blank_lines_are_not_content`）
- 1 行足す → true（`..._adding_a_line_is_policy_change`）
- 1 行書き換える → true（`..._rewriting_a_line_is_policy_change`）
- 列 0 でない `#` の行を足す → true（`..._indented_hash_line_is_content`）
- 許可の一覧でない policy の file（profile）から行を消す → true、evidence は profile（`..._removing_lines_from_non_allowlist_policy_file_is_policy_change`）
- head が profile の `policy_allowlists` に `lists/other.txt` を足し、その file から行を消す → true、evidence は profile と `lists/other.txt`（base 側の profile で判定。`..._head_profile_cannot_add_allowlist`）
- 許可の一覧の file を消す → true、`head_blob` は null（`..._deleting_the_allowlist_file_is_policy_change`）
- UTF-8 で読めない内容 → true（`..._non_utf8_content_is_policy_change`）
- `policy_allowlists` の不正な正規表現 → exit 2・`profile_invalid`・report を書かない（`..._profile_rejects_invalid_regex`）

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE` と `git merge-base HEAD main`）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_allowlist_shrink.py'` → exit 0（Ran 10 tests、OK）
- 判定を外した確認（手元で 1 回だけ。commit していない）: `engine.py` の `policy_changed_files` の戻り値を `[f for f in policy_files if f["base_blob"] != f["head_blob"]]`（前の blob だけの判定）に書き換えて同じ試験を流した → exit 1、`FAILED (failures=2)`。落ちたのは `test_allowlist_shrink_removing_lines_is_not_policy_change` と `test_allowlist_shrink_comment_and_blank_lines_are_not_content`。`git checkout -- scripts/dev/repo-health/health/engine.py` で戻し、再実行で OK。
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → exit 0（Ran 109 tests、OK）
- `sh scripts/dev/tests/repo_health_gate.sh` → exit 0（`repo_health_gate: all checks passed`）
- `python3 -I scripts/dev/repo-health/repo_health.py --repo . --base $(git merge-base HEAD main) --head HEAD --policy-version 1 --out <tmp>` → exit 1。fail は `policy.changed` だけ（evidence は `engine.py`・`profile.py`・`celeris.toml`・`rules/policy_changed.py`。policy の branch なので想定どおり）。task の受け入れ 4 の式（`policy.changed` 以外の必須 rule が pass か not_applicable）→ `[]`、exit 0。
- 本物の repo での模擬（branch は変えず、HEAD を親にした一時 commit を `git commit-tree` で作った）: `--base HEAD --rules policy.changed` で、`scripts/dev/human-text-lint.allow` の 1 行を消した commit → `changed=false`・exit 0。1 行足した commit → `changed=true`・exit 1・evidence `scripts/dev/human-text-lint.allow`。両方の report は `validate_report.py` を通った。
- 検査器のコードに repo 固有の path が無い: `grep -rn 'adr-allowed-duplicates\|human-text-lint' scripts/dev/repo-health/health scripts/dev/repo-health/rules scripts/dev/repo-health/repo_health.py` → 一致なし（exit 1）
- `crates/`・`web/` を変えていない: `git diff --name-only $CELERIS_WU_BASE HEAD -- crates web` → 空
- `sh scripts/dev/progress-index.sh --check` → exit 0、`sh scripts/dev/check-doc-links.sh` → exit 0、`sh scripts/dev/check-adr-numbers.sh` → exit 0（453 files）
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0
- `bash scripts/dev/test-affected.sh --list` → 対象なし（crate の変更が無い）。`bash scripts/dev/test-affected.sh` → exit 1。先頭の `check-health.sh` が `policy.changed`（必須）の fail で止まった。ほかの必須 rule は全部 pass。この branch は policy の file に ADR・進捗・試験を加えて変えるので、D6.2 の「変更 path が全部 `policy.files`」に当たらず exit 0 にならない（ADR health-review-integration の実装の記録「進捗 file と検査器の試験は policy の file に入らない」と同じ。先例の policy の branch も人の承認と operator の merge で通した）。

## WU の health check（`celeris:repo-health/1`）が通らない（attempt 2 で確認）
- 前回の run の後、celeris の WU の check `celeris:repo-health/1`（expected 0）が `health:policy.changed:<fingerprint>`（evidence: `engine.py`・`profile.py`・`celeris.toml`・`rules/policy_changed.py`）で落ちた。
- 再現（提出時の HEAD で実行）: `$CELERIS_WU_BASE` の検査器を `git archive` で一時 dir に出し、`repo_health.py --repo . --base $CELERIS_WU_BASE --head HEAD --policy-version 1` → exit 1。必須 rule の不合格は `policy.changed` だけ、fail 級の finding も rule `policy.changed` の 1 件だけ。
- 同じ検査器の `engine.policy_review_files(report, changed)`（gate が PolicyOnly〈合格〉を決める D6.2 の判定）は、WU の変更 path 全部では `None`、policy の 4 file だけなら 4 file を返す。`None` にしているのは policy でない 4 file（`agent-docs/adr/2026-11-07-repo-health-contract.md`・`agent-docs/adr/2026-11-16-health-policy-allowlist-shrink.md`・この進捗・`scripts/tests/test_repo_health_allowlist_shrink.py`）で、どれもこの WU の受け入れ条件 0・3・4 が求める file。
- このため、受け入れ条件を満たしたままこの check を exit 0 にする変更は無い。計画の問題として replan を申告した（`plan_issue`）。コードは変えていない。

## 未解決
- 判定は main に入った後の branch から効く（契約 D2: policy は base 側で評価する）。この branch 自身は `policy.changed=true` で、D6.4 の人の承認で通す。LoC 削減 v3 E（01M7J3NKVAH9NG53C5FEFNQJYK）の `human-text-lint.allow` の 3 行の削除は、この branch が main に入った後の base で `policy.changed` に数えられなくなる。
- `HealthPolicyState.files`（`crates/task-dispatch/src/health_review/input.rs`）は blob の違う policy の file の一覧なので、数えなかった許可の一覧も reviewer の prompt に `changed=false` と並んで出る。D6.1・D6.3・D6.4 は `changed=false` では働かない（ADR D4）。Rust は task の指示どおり変えていない。

## 提案
- 計画の直し方（どちらかを planner か人が選ぶ）: (a) policy の 4 file だけを変える WU を、ADR の WU（前の段）と試験・進捗の WU（後の段）から別の段に分け、その WU と段の統合の diff を policy の file だけにする（D6.2 の PolicyOnly で合格）。(b) 先例 01M76BYGKY と同じく、この WU の health の不合格を人が承認し、operator が merge する（ADR health-review-integration の実装の記録「policy の branch は人の承認と operator の merge で通した」）。task の最終 review でも同じ理由で D6.1 の `policy_mixed` になるので、(a) を選ぶなら task も policy の file だけの task とほかの task に分ける必要がある。
