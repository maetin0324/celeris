---
title: K4 ops-r1 pilot 固定票の追補（有料 2 試行の承認を events と照合）
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 ops-r1 pilot 固定票の追補（auth-addendum）

状態: 完了。有料 2 試行の承認（人の決定 approve-paid）を本番 DB の events と読み取りだけで照合し、固定票を参照する追補を書いた。試行はやり直していない。課金は 0。

成果物（repo の外）: `/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/freeze/`

- `authorization-addendum.json`（sha256 `ab21d4ee7370eee0dde5b50e368429f8bb67b0de9040c4873a315a6b2e583aac`）と同じ内容の `authorization-addendum.md`（人向け）
- `../report.md` の「承認」の行を照合結果と追補の path に書き換えた（「未照合」の記述を消した）
- `fixed-ticket.json` は変えていない（sha256 `98494634b98ef8efa45828a835471627f6b66f0ef702c1a5ace64225f7869c82`、追補が参照）

## 照合の手順

1. `sqlite3 "file:$CELERIS_RUN_DB?mode=ro"` で decisions（task_id=01M62S8XWCZDCY2ZBVQ35176MH）を読み、key=approve-paid の行を特定した（1 件だけ）
2. 同じ DB の events から decision id 01M62SF81ZH7K8Y911TDB05TZS を含む行を読んだ: decision_requested（event 747682・seq 31、2026-10-29T02:57:49.119Z）と decision_answered（event 747753・seq 43、2026-10-29T03:00:34.024761313Z、option approve、by human:web）
3. その後の decision_answered・answered・plan_gate_decided を読み、approve-paid の再回答・人の再承認が無いことを確かめた。prep の途中確認（seq 1363、2026-10-30T06:05:34Z）は統括 CoS の回答で承認ではない。trials-done（seq 1365）は operator:claude-code の回答
4. A の開始は run/a-start.started（2026-10-30T06:51:06Z）、approval-request.md の作成は stat の mtime（2026-10-30T04:40:18Z）で取った

## 結果

- events_verified: true。decisions 行の answered_at と event の ts は一致した
- 承認は最初の有料試行より前（true）
- report.md の 2026-10-29T03:00:34Z は events と一致していた。食い違いは順序で、問いが参照する approval-request.md は承認の約 25 時間後に作られた。承認の時点で人が見られた上限は依頼文の参考上限だけ
- 残枠: 承認 2・消費 2・残り 0。A 194,226 tokens、B（案件 CoS）550,684 tokens で、1 試行・合計の上限内、期限 2026-11-01T03:00:34Z 内
- 逸脱: A の account（承認票の欄は空欄、外部 operator が claude_max_lab を選んだ）、B の送信者（欄は空欄、外部 operator が human:rmaeda の token を作って送った）。どちらも人が埋めた記録は無い

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `sha256sum /local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/freeze/fixed-ticket.json` → 98494634…9c85（作業の前後で同じ）
- `python3 -I -c 'import json;json.load(open(".../authorization-addendum.json"))'` → exit 0
- `grep -c 未照合 /local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/report.md` → 0
- repo の変更はこの進捗ファイルだけ（`git diff --name-only $CELERIS_WU_BASE`）。Rust のコードを変えていないので test-affected の対象 crate は 0

## 未解決

- approval-request.md より前に出た承認を、その文書の範囲の承認として扱ってよいかは人が確かめていない（未確認）
- A の account・B の送信者を人が決めた記録が無い

## 提案

- 有料試行の承認の決定は、上限を書いた文書ができた後に出す（計画の段で決定を要求するなら、文書の sha256 を問いに入れ、文書が変わったら決定を出し直す）
- 承認票の空欄（account・送信者）は、試行の前に人の決定か human-op 記録で埋める。runbook に「空欄のままなら止める」を停止条件として足す
