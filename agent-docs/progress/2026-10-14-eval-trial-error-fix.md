---
task: 01M4W3PTCHJD4XQPZGT81WBTRM
status: running
---
# 評価試行（eval trial）が全部 error になる件の修正

設計判断は [ADR 2026-10-14-eval-trial-result-placement](../adr/2026-10-14-eval-trial-result-placement.md)（D1〜D4）。
WorkUnit ごとの記録は `2026-10-14-eval-trial-error-fix/<wu-key>.md`（fix-run・aggregate）。

## redo（WorkUnit）: やり直しの正規手順

- `crates/routing-eval/src/run.rs`: `celeris-routing-eval run --trial-offset <n>`（既定 0）。各候補の trial_no を
  `n+1..=n+k` にして投入する。旧い行は消さない（ADR D4）。`main.rs` の usage にも載せた。
- 試験 `run::tests::run_trial_offset_shifts_trial_no`: `run_args` に `--trial-offset 2` を渡し、k=2 の候補
  （claude-opus-5-5）の POST body の trial_no が `[3, 4]`、summary も 3・4。負の値は `invalid --trial-offset`。
- 証拠: `cargo test -p routing-eval --lib run::tests::run_trial_offset_shifts_trial_no -- --exact` → `test result: ok. 1 passed`。

## 本番への移行

実行は運用セッション（人）。本番 config・DB の手での書き換えは無い（eval_trials への行の追加は API 経由）。

1. この task の修正（fix-run・aggregate・redo）を含む release を本番に昇格する。fix-run の置き場修正が無い
   release でやり直すと、また全部 error になる。
2. 旧 pilot の trial_no を確かめる（読み取りのみ）:
   ```sh
   curl -sS -H "Authorization: Bearer $(cat <token-file>)" \
     "<api>/api/v1/routing-eval/trials?batch_id=01M4W35QEMQGVV5ZSD0JJ61ZDD" \
     | python3 -c 'import json,sys; t=json.load(sys.stdin)["trials"]; print(len(t), max(x["trial_no"] for x in t), {x["status"] for x in t})'
   ```
   期待: 10 件、全部 `error`。2 列目（最大 trial_no）を `M` とする（pilot の候補が k=2 なら M=2）。
3. 同じ manifest・同じ候補で、offset `M` を付けて投入する:
   ```sh
   celeris-routing-eval run --api <api> --token-file <token-file> \
     --manifest <pilot と同じ frozen manifest> --candidates <pilot と同じ候補> \
     --repo <pilot と同じ repo> --trial-offset <M> --wait --out <artifacts>/eval/pilot-redo.json
   ```
4. 確認: 出力の `batch_id` が旧 batch（01M4W35QEMQGVV5ZSD0JJ61ZDD）と異なり、`trials[].trial_no` が全部 `M` より大きい。
   旧 batch を手順 2 と同じく読み直し、10 件がそのまま残っている。error の行があれば `error` 欄に
   `run ended as <kind>: <本文>` の理由が入っている（ADR D2）。
5. aggregate を流すと旧 pilot の 10 件は除外数（error）に出て、合否はやり直した試行で数えられる。
6. もう一度やり直すときは、offset をその時点の最大 trial_no にする（例 2 回目は `M+k`）。

戻し方: 新しい batch の行は残したまま、旧 release に戻せばよい（`--trial-offset` は CLI だけの変更で、DB の形は変えていない）。

## 未解決事項

- 全体検査（test-parallel・clippy）と完了日の記入は closeout の WorkUnit が行う。
