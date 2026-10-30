---
title: K4 ops-r1 B-t002 の事後追補（事前固定と事後の区別、未確認の判断の分離）
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 ops-r1 B-t002 の事後追補

状態: 完了（WU addendum）。LLM・daemon・実試行・本番配備はしていない。親 task の
`R=/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1` と追試 task の
`P=/local/celeris/data/workspaces/01M6617H9V0TZGQKFA9J82141A/artifacts/ops-r1` は読んだだけ。

成果物（repo の外）: `/local/celeris/data/workspaces/01M669QAZE5WEQDJBQS8MRX1AK/artifacts/ops-r1/addendum/`

- `k4-b2-addendum.json`（ops-r1-k4-b2-addendum/1）と `k4-b2-addendum.md`: B-t002 の 6 項目
  （condition・manifest・採点 binary・承認 b2-approve・daemon.toml の state path 5 行・session-map v2）。
  各項目に origin（pre_fixed 4・post_hoc 2）・recorded_by・confirmed_by・fixed_at がある。
  人の確認の記録があるのは承認だけ（human:rmaeda、記録者は外部 operator、時刻は null）。
- `unconfirmed_judgements`: 報告第 2 版冒頭の (a)(b)(c) を unconfirmed として分けた。
- `comparability`: B-t001（B-celeris-ops）と B-t002（B-celeris-ops-r2）は condition が違う。合算の成功率は null。
- `trial_dedup`: 2 つの pipeline の 4 行を trial_id で数えると 3 試行。A-t001 は 2 つの pipeline で同じ run_id（json の trial_dedup に全桁）。
- `approval_frame`: approve-paid は残り 0、b2-approve は残り 0。KB の 2026-10-26 枠は A0/B0 系列が範囲なので ops-r1 には数えない。追加の試行は 0 件。
- `build-addendum.py`（生成。2 回生成して `cmp` で一致）、`check-addendum.py`（検査）、`kb-snapshot/`（KB の承認枠 page の写し）。

## 証拠（提出時の HEAD で実行）

- `python3 -I .../addendum/check-addendum.py .../addendum/k4-b2-addendum.json` → exit 0。
  file 参照 30 件と `R/freeze` の tree（5,815 file、tree の sha256 は json の freeze_unchanged.tree に全桁）が実物と一致。項目 6、unconfirmed 3、試行 3。
- 欠陥を入れた写し（origin 削除・sha256 改ざん・(a) を human に・合算率 0.5）では exit 1、4 件とも検出。
- `R/freeze/digests.sha256` の `sha256sum -c` → 一致。

## 未解決事項

- (a)(b)(c) と、親の authorization-addendum の問い（承認が approval-request.md の作成より前）は人が答えていない。
- 固定票（`R/freeze/fixed-ticket.json`）は B-t002 を載せない。この追補が sha256 で参照して補う。
- KB の承認枠 page は live の path も sha256 で照合する。KB が整理されると検査が落ちる。そのときは `kb-snapshot/` の写しを正とする。

## 提案

- B のような「固定票の外で試行前に決めた条件」は、試行前に固定票の追補として sha256 付きで残す手順を runbook に足す。
- 人の発言による承認は Celeris の decision として記録する（今回は外部 operator の human-ops 記録だけで、時刻も無い）。
