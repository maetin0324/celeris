---
title: closeout-evidence — review.json 回収の証拠と文書検査の確認
tasks: [01M6S325NYYJS9YWNHDH757E08]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# closeout-evidence — review.json 回収の証拠と文書検査の確認

完了日: 2026-11-06

## 結論
コードは変えていない。generalize・wire-review の進捗ファイルと実装を読み、task の受け入れ条件ごとの証拠がそろっていることを確かめた。
不足していた closeout の進捗ファイル（このファイル）だけを足した。

## 確認結果（受け入れ条件ごと）
- review.json の誤配置回収が result.json と同じ 1 つの仕組みであること
  - 確認: `grep -rn "misplaced_result" crates` — 関数 `misplaced_result` は残っていない。残りの一致は試験名（`a_misplaced_result_is_rejected_without_adoption` など）と
    `worker_finish.rs` の局所変数だけ。回収は `crates/task-worker/src/result_location.rs` の `misplaced_report(file_name, …)` 1 つで、
    result.json（claude_code・codex・worker_task）と review.json（`crates/task-dispatch/src/review.rs`）が同じ関数を呼ぶ。
  - 根拠: generalize.md「結論」、wire-review.md「結論」。
- ADR の場所: `agent-docs/adr/2026-11-06-misplaced-report-recovery-generalized.md`。
  review.json が回収されなかった理由（候補が file 名 `result.json` 固定・review.rs が回収を呼ばない）と、1 つの規則にした設計（D1〜D5）が書かれている。
  `git log` の順で、ADR の commit（件名 `adr: generalize misplaced report recovery to review.json`、`adr: wire review.json recovery and absolute paths in the reviewer prompt`）は
  対応する実装の commit より前にある。
- reviewer prompt の work_dir 注意: 確認したところ reviewer には出ていなかったため、wire-review で直した（ADR D5）。
  `artifacts_rel` が reviewer run でも絶対パスを返し、`work_dir_note` が reviewer run にも注意を出す。
  試験 `reviewer_prompt_states_absolute_review_and_result_paths`（claude_code・codex）で固定。
- 回帰試験: 修正前に落ち修正後に通る確認は wire-review.md「試験」に記録済み
  （配線だけ外して `cargo nextest run -p task-dispatch -E 'test(/misplaced|stale_misplaced/)'` を実行し 4 件中 2 件が落ち、戻すと 4 件とも通った）。
  0 件・複数件・run 開始より古い 1 件が従来どおり落ちる試験は review.rs の試験と result_location の単体試験にある。
- 本番の config 変更: 要らない（generalize.md・wire-review.md の「本番への移行」節に記載）。

## 証拠（提出時の HEAD で実行）
- `sh scripts/dev/check-doc-links.sh` — exit 0、`check-doc-links: ok`
- `sh scripts/dev/check-adr-numbers.sh` — exit 0、`check-adr-numbers: ok (398 files)`
- `sh scripts/dev/progress-index.sh --check` — exit 0、`progress-index --check: ok`
- 参考: `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — `check-doc-layout: ok`、
  `python3 -I scripts/dev/check-human-text.py --self-test` — `ok`
- コードの検査（test-affected・clippy・fmt）は generalize.md・wire-review.md に各 WU の提出時の結果がある。この WU はコードを変えていない。

## 本番への移行
本番の config 変更は要らない。

## 未解決事項
なし。

## 提案
なし。
