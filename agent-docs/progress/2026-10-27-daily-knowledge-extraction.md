---
title: 日次整理前の報告抽出と10月3日以降の遡り
tasks: [01M5Z79FN06T6ZM434H4MHZS12]
status: done
updated: 2026-10-27
---
# 日次整理前の報告抽出と10月3日以降の遡り

完了日: 2026-10-27

## 変更

- `phase 1: 日次整理の前にlangmemで報告を抽出し抜けを遡れるようにする` — 整理前の langmem 支援 task、SQLite の元 task ごとの抽出記録、全候補を _inbox に送る検証経路、失敗の要約・受信箱表示を追加。
- 日次整理は入力準備の永続化まで dispatch しない。再起動後は同じ抽出 task を回収する。抽出の待機は注入した時計で30分までとし、超過した task は取り消して整理を続ける。
- 候補は最大40件。整理のD12上限も40件のまま。遡りは同じUTC日を最大10 taskずつ処理し、_inboxが40件以上なら抽出を持ち越して整理を先にする。
- `dry_run` は整理による正本ページの変更を止める。前段の抽出は候補を _inbox に置くので、KBディレクトリ全体が不変という意味ではない。
- 設計は [日次抽出ADR](../adr/2026-10-27-daily-knowledge-extraction.md)。dispatcher/storeはLLMを呼ばず、langmemのPythonだけがLLMを呼ぶ。既存のllm_routesを使い、汎用harnessへの切替はこの抽出では行わない。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- 条件0（抽出→検査→候補→整理、失敗継続）: `bash scripts/dev/test-affected.sh` → exit 0。抽出の前後関係、再起動時の再利用、候補の冪等性、失敗・不正出力・出典不足・上限超過、注入時計による待機期限、同日遡りの分割と重複防止、満杯時の持ち越し、明示再試行を決定的に検証。本物のLLMは使っていない。
- 条件1（設計・遡り手順）: ADR-0131 D10を更新し、日付を固定した10 task/回の遡り手順を下に記載。langmemのproxy鍵は、独立したoperator tokenを用意した設定試験に合格。dispatcher/storeにLLM呼び出しは追加していない。
- 条件2: `bash scripts/dev/test-affected.sh` → exit 0。Rust 5,326 passed / 21 skipped、対象crateのdoc test合格、Web tsc合格・unit 682 passed、gateway 99 passed。初回の旧前提の2件を修正して再実行した結果。
- 条件3: `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0（差分なし）。
- `sh scripts/dev/progress-index.sh --check` と文書・source-size・人向け文面の検査は test-affected の health 段で合格。

## 本番への移行

本番操作は人が行う。このrunでは本番DB・KB・設定を変更せず、本物のLLMも呼んでいない。

1. 新版を配備する。`[llm_proxy] token_file` はA1で切替済みの `llm-proxy.token` を使う。langmemのアダプタとdispatch設定がこの鍵を解決することは、operator用tokenを異なる値にした試験で確認した。`[[llm_routes]] harness="langmem"` のqwen→claude_oauth/codex_oauthの経路は変更しない。実際の到達性は配備後の抽出taskのWorkerStartedと終端報告で確認する（鍵は表示しない）。
2. `celerisctl cron list` で日次整理job名を確認する。以下は `daily-curation` の例。日次整理が走っていないことを確認し、遡りの操作中は一時停止する。既存雛形のobjective・acceptance・担当をそのまま複製する。

```sh
celerisctl cron pause daily-curation
celerisctl cron show daily-curation > daily-curation.saved.json
python3 - <<'PY'
import json
job = json.load(open('daily-curation.saved.json'))['job']
template = job['template']
template['extraction_date'] = '2026-10-03'
template['mode'] = 'apply'
json.dump(template, open('knowledge-backfill-template.json', 'w'), ensure_ascii=False)
PY
celerisctl cron create --name knowledge-backfill-20261003 \
  --schedule '0 0 * * *' --timezone UTC --enabled false \
  --template "$(cat knowledge-backfill-template.json)"
celerisctl cron run knowledge-backfill-20261003
celerisctl cron history knowledge-backfill-20261003 --limit 10
```

JSONの `extraction_date` はtemplate直下に置く（Rustの `extra` はserdeのflatten）。日付はtask作成時のlabelに固定され、後のjob更新は既存taskを変えない。

3. GUIで今回の整理taskと「日次の知識候補抽出」の終端、`daily-summary.md`、_inboxの残り件数を確認する。抽出の出典は元taskのIDを持つ。抽出が失敗しても整理は走る。失敗・持ち越し・除外の理由は日次報告と判断待ちに出る。
4. 整理が終わってから同じ `cron run` を実行する。毎回、未処理の元taskを最大10件だけ拾う。「対象 task 0」で、失敗・持ち越しが無い回になったらその日を終える。「候補0」だけでは終了と判断しない。_inboxが40件以上の回は抽出せず整理だけを行うので、整理後に同じ日を再実行する。既存 `knowledge_runs` の行を消さない限り、同じ元taskを再抽出しない。10月3日に旧経路ですでに抽出したtaskも除外する。
5. 次の日を指定して同じjobを更新し、手動発火する。2026-10-03から配備日まで、UTC日ごとに同じ手順を繰り返す。一日分を複数日に分けて処理してよい。各操作の区切りで通常の日次jobを再開し、遡りを再開するときに通常jobを停止して同時発火を避ける。

```sh
python3 - <<'PY'
import json
p = 'knowledge-backfill-template.json'
t = json.load(open(p))
t['extraction_date'] = '2026-10-04'  # 次に処理するUTC日
json.dump(t, open(p, 'w'), ensure_ascii=False)
PY
celerisctl cron update knowledge-backfill-20261003 \
  --template "$(cat knowledge-backfill-template.json)"
celerisctl cron run knowledge-backfill-20261003
# 遡りの操作を中断・終了するとき
celerisctl cron resume daily-curation
```

6. 失敗分は自動再抽出しない。原因を直した後、人が `celerisctl knowledge rerun <元task-id>` で失敗した元taskだけを再指定し、該当UTC日の遡りjobを発火する。成功した元taskや日次整理task自体をresetしない。resetのない同じ日の再実行は失敗分も含めて重複を作らない。取り込み済み候補の重複生成を避けるため、除外が一部だけの回は成功分をresetせず、除外理由を個別に確認する。
7. 最後に通常の日次jobの再開を確認し、GUIの定期実行画面で遡り用のdisabled jobを削除する。必要な報告と抽出記録は保持する。

## 未解決

本番の遡り実行、実LLMの到達性、実際の候補件数は未確認。上の手順は人の本番操作として残す。

## 提案

なし。日次整理の40件上限は変えない。
