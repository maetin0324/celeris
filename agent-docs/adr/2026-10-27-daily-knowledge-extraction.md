# 日次整理の前に langmem で報告を抽出する

日付: 2026-10-27

## 決定（ADR-0131 付記 D10 の更新）

D10 が予定した報告からの抽出は実装されていなかった。整理 worker は候補の振り分けを続け、その前に daemon が langmem 支援 task を起こす。LLM は langmem の Python process だけで呼ぶ。既存の harness=langmem の llm_routes と独立した llm_proxy.token_file を使い、汎用 harness への切替はこの抽出には使わない。

1. cron 由来の日次整理 task は、daemon の入力準備完了を SQLite の knowledge_runs に記録するまで dispatch しない。抽出 task は元の日次 task を示す label と入力 JSON を持つ。再起動時は同じ label の task を回収し、新しい抽出を重ねない。
2. reports の期間は直前の created cron run から今回の発火時刻まで。報告・結果・コメントの抜粋、既存 KB の題名と関連ページを渡す。抽出済み・抽出中の元 task は knowledge_runs の主キーで除外する。失敗も記録し、自動で何度も抽出しない。
3. 出力は knowledge-candidates.json。apply_candidates の検証と配置規則を通すが、専用 Inbox policy で全候補を _inbox に置く。元 task の出典を必須とし、最大 40 件を超える出力は全体を拒否する。候補のファイル名は run と候補の内容から決定し、適用中断後の再実行で複製を作らない。
4. 抽出終端後に KB をコピーし、D12 の 40 件上限を維持して整理する。失敗時も入力を用意し、日次要約と判断待ちに理由を記録する。抽出失敗の判断待ちは整理 run を妨げないタイミング（整理終端）で出す。
`dry_run` は整理による正本ページの変更を止めるが、前段の抽出候補は _inbox に保存する。

5. 遡りは cron 雛形の extraction_date（YYYY-MM-DD、UTC）を発火時の label に固定する。一度の抽出は未処理の元 task 最大 10 件。_inbox が 40 件以上なら新しい抽出を控え、整理だけを走らせる。同じ日を再実行して未処理分を順に拾う。日付が無ければ通常の日次処理。source task の既存 knowledge_runs は遡りでも除外する。失敗の再試行は人が `celerisctl knowledge rerun <元task-id>` で指定した場合だけ許す（resetが消すvia/applied_atで区別する）。

## 本番への移行

本番操作は人が行う。既存日次 job の雛形を複製した disabled job を作り、extra.extraction_date を 2026-10-03 から 1 日ずつ指定して手動発火する。各回の整理完了と候補数を確認してから次回を発火し、同じ日の未処理が無くなるまで繰り返す。通常の日次 job と同時に発火しない。終了後に遡り用 job を削除する。具体的な CLI と確認方法は本 task の進捗に記す。knowledge_runs は消さず、失敗分の再抽出は原因確認後の別操作とする。

具体的な実行コマンドは [本番への移行](../progress/2026-10-27-daily-knowledge-extraction.md#本番への移行) を参照。
