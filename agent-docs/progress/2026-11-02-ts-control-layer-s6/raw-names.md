---
title: S6 raw-names — 連結済み生データを受け入れ条件の file 名に改名
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S6 raw-names — 連結済み生データを受け入れ条件の file 名に改名

unit raw-names。task の受け入れ条件 2 は `policy`・`cos`・`policy-ts`・`cos-ts` の jsonl を求めるが、s6-report は測定 unit の名前のまま残していた。連結済みの 4 file を `git mv` で改名し、参照を直した。中身は 1 byte も変えていない。複製は作っていない。コードと script は変えていない。

## 改名の対応
| 改名前 | 改名後 | 中身 |
|---|---|---|
| `policy-warm.jsonl` | [policy.jsonl](policy.jsonl) | 旧 Rust 経路 policy warm、a+b 連結、20 試行（records 212） |
| `cos-digest-warm.jsonl` | [cos.jsonl](cos.jsonl) | 旧 Rust 経路 cos-digest warm、a+b+c 連結、20 試行（records 258）。scenario は `cos-digest` で、S0 の `cos`（`job_wait_extend`）とは別 |
| `policy-ts-warm.jsonl` | [policy-ts.jsonl](policy-ts.jsonl) | TS 経路 policy-ts warm、20 試行（records 22） |
| `cos-ts-warm.jsonl` | [cos-ts.jsonl](cos-ts.jsonl) | TS 経路 cos-ts warm、20 試行（records 22） |

名前を変えていない file: 部分 file `policy-warm-a.jsonl`・`policy-warm-b.jsonl`・`cos-digest-warm-a.jsonl`・`cos-digest-warm-b.jsonl`・`cos-digest-warm-c.jsonl` と `cos-ts-cold.jsonl`。

## 直した参照
- [S6 の進捗](../2026-11-02-ts-control-layer-s6.md): 生データの表・証拠の節の file 名、「file 名は S5-10 の表の名前に揃えた」の文（旧: 測定 unit の名前のまま使う）。
- m-policy-a・m-policy-b・m-cosd-b・m-cosd-c・m-ts-warm・m-ts-cold: link と記録のコマンドの file 名。各 file に「改名後の名前で書いた」旨を書いた。
- [境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S6-1: 旧い表は消さず、「2026-11-03 改める（raw-names）」の注記を足した。
- `2026-11-02-ts-control-layer-s3*` は S3 自身の file を指すので触っていない。

## 検査（提出時の HEAD で実行）
- 受け入れ条件 2 のコマンド（4 file の `test -s` と `bash scripts/dev/edit-activate-baseline.sh --validate`）→ exit 0。policy records 212・cos 258・policy-ts 22・cos-ts 22、problems はどれも 0。
- `git diff -M --summary $CELERIS_WU_BASE` → 4 件とも `rename … (100%)`。改名の前後で `sha256sum` の値が 4 file とも同じ（値は run の中で比べた）。
- `python3 -I scripts/dev/edit-activate-summary.py agent-docs/progress/2026-11-02-ts-control-layer-s6/*.jsonl` → exit 0。
- s6 の進捗・m-*.md に旧 4 名（`-warm.jsonl` で終わる連結 file 名）の参照は残っていない（この file の対応表と ADR の日付付きの旧い表を除く）。
- 文書の検査: `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → いずれも ok。

## 未解決
- 無し。
