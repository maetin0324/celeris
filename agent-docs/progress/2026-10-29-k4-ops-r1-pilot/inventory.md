---
title: K4 ops-r1 pilot 開始前の棚卸し（版・資産・承認・計測経路の不足）
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-29
---
# K4 ops-r1 pilot 開始前の棚卸し

完了日: 2026-10-29

成果物（repo の外）: `/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/inventory.json` と `inventory.md`。
読み取り専用・無課金で調べた（本番 daemon には GET だけ。LLM・claude CLI は起動していない）。コードは変えていない。

## 結果

- 版: main HEAD と本番の稼働 release はどちらも 94f86928dc13（health.release・current symlink・稼働 process）。
  CoS が認識していた 7b7aafc713f6 は 2026-10-28T00:17Z に置換済みで、食い違いは CoS の認識の古さ。
- K4 資産: 公開入力 v2 の tgz・case.json・materials・grade-case・grader-private は、採用記録と旧 final-freeze-2 の写しと sha256 が全部一致。
  K4 採点修正の後、main で grader のソースは変わっていない。旧 pilot・台帳は読んだだけ。
- CoS: cos_policy_version 6（health と config）。cos-inbox-triage 版 6、cos-operator 版 5。tree digest は inventory.json。
- model: CoS は claude-opus-5-5 だけ（代替なし）。claude_code の既定は sonnet-5-5 先頭、coding は opus 先頭。
  Claude Code 2.1.303（旧 pin と同じ sha256）。claude-sonnet-5-5 は model catalog で available（実呼び出しはしていない）。
- 承認: 旧 2026-10-26 承認（A0/B0、sonnet 固定、40 試行、期限 2026-11-13、消費 0）は ops-r1 に使えない（`usable_for_ops_r1: false`）。
- 計測経路の blocking な不足: g-b-cos-intake・g-b-trace-export・g-b-trial-attribution・g-cos-measure-daemon・g-grader-private-isolation。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `curl /api/v1/health` → release 94f86928dc13、cos_policy_version "6"、schema 102
- `readlink -f /local/celeris/state/current` → releases/94f86928dc13
- `sha256sum` で資産・旧 bundle・承認・台帳の digest を取得（値は inventory.json）
- `python3 -c 'json.load(...)'` で inventory.json の 7 欄（release・assets・cos_skills・models・approval・tooling_gaps・blocking_gaps）を確認
- コードを変えていないので test-affected・clippy・fmt は対象なし（進捗ファイルだけの変更）

## 未解決

- GitHub 上の origin/main は gh を使えないため未照会（local ref の main だけ）。
- 本番の CoS run に実際に配られた skill の写しの digest は未確認。
- 旧 pin の celeris-campaign bin が main から build した bin と同じかは未確認。

## 提案

- blocking な 5 件の修正 task（inventory.json の tooling_gaps に task 案と condition）を有料試行の前に起票する。
- 人の決定: ops-r1 有料 2 試行の承認（d-ops-r1-approval）と、B の model 条件（d-ops-r1-models）。
