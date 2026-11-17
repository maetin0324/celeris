---
title: repair-1: 許可の一覧の '\n' 以外の改行の文字の抜け道を塞ぐ（reviewer の指摘の修復）
tasks: [01M7KATBGCRQRZ5NWXHXVWYPZ8]
status: done
updated: 2026-11-17
completed: 2026-11-17
---

# repair-1（WorkUnit）

完了日: 2026-11-17。policy の差分の人の承認と operator の merge は未（task の進捗の「repair-1」）。

## 指摘
reviewer 01M7KJH9BREHTYP8GWC2M2ZWJW（受け入れ 5 が不合格）: `engine.py` の `_content_lines` は内容を `'\n'` だけで分け、列 0 が `#` の行を除く。`scripts/dev/check-human-text.py` の `allow_list` は text mode（universal newlines）で開くので、単独の CR も行の区切りにする。`#note<CR>slop_vocabulary<TAB>NEW-SNIPPET-NOT-IN-BASE<TAB>smuggled` を 1 行足すと `policy.changed=false`（exit 0）のまま新しい許可が入った。

## 実装前 brief
1. 根本原因: D2 の「内容の行」の分け方（`'\n'` だけ）が、読み手の分け方（universal newlines）と違う。症状は許可を足す変更が `policy.changed` から外れること。
2. 責務 owner: `scripts/dev/repo-health/health/engine.py` の `_content_lines`（判定の唯一の実装）。
3. 既存の再利用先: UTF-8 で読めない内容を None にして数える既存の経路。試験は `scripts/tests/test_repo_health_allowlist_shrink.py` の fixture と `assert_counted`。
4. 選択肢: (a) `'\n'` 以外の改行の文字を含む内容は比べずに数える。採用（どの読み手の分け方でも同じ行になる内容だけを比べる。読み手の一覧を検査器が持たない）。(b) universal newlines で分ける。`sed`・Rust の `str::lines` と行が食い違う。採らない。(c) 読み手ごとの分け方の全部で包含を確かめる。repo 固有の解釈が検査器に入る。採らない。
5. 新しい状態・分岐・fallback・port: 定数 `_OTHER_LINE_BREAKS` と `_content_lines` の 1 つの条件だけ。flag・互換分岐は無し。
6. 撤去対象: 無し（ADR D2 の「`'\r'` も比べる」の記述は付記 A1 で直した）。
7. 不変条件: `'\n'` 以外の改行の文字を base か head に含む許可の一覧の変更は `policy.changed=true`。既存の 10 件の試験の判定は変わらない。

## 変更
- `health-policy-allowlist-shrink: '\n' 以外の改行の文字を含む許可の一覧は比べずに数える付記 A1 を ADR に足す` — [ADR 2026-11-16-health-policy-allowlist-shrink](../../adr/2026-11-16-health-policy-allowlist-shrink.md) の D2 条件 5・読み手の表・付記 A1。実装より先の commit。
- `health-policy-allowlist-shrink: '\n' 以外の改行の文字を含む許可の一覧は比べずに policy.changed に数える` — `engine.py` の `_content_lines` と試験 2 件（`test_allowlist_shrink_other_line_breaks_are_policy_change`・`test_allowlist_shrink_crlf_content_is_policy_change`）。
- 成果物 `allowlist-shrink-policy-diff.md`（task の成果物の dir）に「追加の policy の差分」節を足した。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- 試験を先に足し、engine を直す前に `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_allowlist_shrink.py'` → FAILED（failures=10。CR など 9 文字の subTest と CRLF の 1 件）。直した後 → Ran 12 tests、OK。
- reviewer の再現（base = HEAD、head = HEAD の `human-text-lint.allow` に上の CR の 1 行を足した一時 commit。作業ツリーは変えない）: `repo_health.py --rules policy.changed` → exit 1、`policy.changed=True`、rule `policy.changed` は fail（直す前は exit 0・False）。
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → exit 0（Ran 111 tests、OK）。
- `sh scripts/dev/tests/repo_health_gate.sh` → exit 0（`repo_health_gate: all checks passed`）。
- task の受け入れ 4 の式 → `[]`・exit 0。`repo_health.py` 自体は exit 1 で、`policy.changed=true`（evidence は `engine.py` だけ）。policy の branch では想定どおり。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0（Rust は変えていない）。
- `bash scripts/dev/test-affected.sh` は走らせていない。policy の branch では冒頭の `check-health.sh` が `policy.changed` で exit 1 になる（先例 01M76BYGKY、WU allowlist-shrink と同じ）。
- 範囲外差分の検査のうち、main との差分を `agent-docs/progress/` だけに限る 3 つは exit 1（`agent-docs/adr/` と `scripts/` の 3 file が出る）。この修復は `engine.py` を変えるので、merge の前は満たせない。ほかの検査の結果は task の進捗の最終の記録と報告に書く。
