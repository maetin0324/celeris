---
title: K4 ops-r1 pilot 実行手順と承認要求
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 ops-r1 pilot 実行手順と承認要求

状態: 完了。人が A→B の 2 試行を実行する手順と、承認要求の文面を書いた。有料試行は 0 件。LLM・daemon は起動していない。

成果物（repo の外）: `/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/`

- `runbook.md`: 準備（照合・release dir の写しと current・人の principal の token・実試行用 manifest（t001）・両腕共通の seed・grade-case の隣の公開入力 tgz・canary 付き漏洩検査・transcript 保持）、A の起動と観測と保存、B の測定 daemon の起動・案件 CoS thread・ops-r1-intake による受付・cutoff までの観測、停止条件と `raw/stop.json`、human-op /2 の記録表、試行後の trace・attribution・漏洩検査・採点〜整合性 checker・operator-usage
- `approval-request.md`: 試行数 2・上限 tokens/wall・model・account・停止条件・有効期限の 1 画面の承認票

## 入れた preflight の gaps（blocking でないもの）

- g-measure-release-dir-copy: runbook §2.2（release dir の写しと current の向け先）
- g-intake-human-principal: §2.3（人の principal の token を足した daemon.toml の写し。差は 4 行だけと照合）
- g-ops-r1-manifest-not-in-freeze: §2.4（make-manifest.py の t001 版）
- g-grader-input-staging: §2.6（stage-grader-inputs）
- g-grader-private-same-uid: §2.7（canary 入りの写しと試行前後の漏洩検査）
- g-b-observed-versions-claude: §4.5
- g-b-cos-consumption-shared: §7.3 と承認票（人の判断待ち）

## 証拠

- `sha256sum runbook.md approval-request.md`（成果物 dir で実行）: exit 0
- 検査は提出時の HEAD で実行（結果は報告に書く）

## 未解決

- B の作業場所を project の workspace で渡す方法（固定票 open_conditions 2）は実の測定 daemon で確かめていない。runbook §4.2 で確かめ、駄目なら B を止める。
- A の Claude Code の引数は claude を起動して確かめていない（worker は LLM CLI を起動しない）。測定者が `claude --help` で確かめる。
- 有料 2 試行の承認は未取得。

## 提案

- integrate-prep で、§2.3 の daemon.toml の写しの sha256 と §2.4 の manifest の sha256 を固定票の改訂に載せる。
- B の CoS 消費を試行の tokens に含めるかを人が決める。
