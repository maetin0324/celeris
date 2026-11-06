---
title: reviewer 合格の再利用で配送の行に検査済み SHA が書かれず再 review を繰り返す問題（N1）
tasks: [01M6RG9RT6PQAXV1CRN93FVCVT]
status: running
updated: 2026-11-06
---
# reviewer 合格の再利用で配送の行に検査済み SHA が書かれず再 review を繰り返す問題（N1）

完了日: 2026-11-06（進捗ファイルの WorkUnit。統合は plan の integrate-record）

## 結論
- N1（01M6QW0GSG1AVARWC334AZCES1）が 2026-11-06 12:30:36Z から review_pass → done → rereview → reviewing → reviewer pass reused → review_pass を
  繰り返した原因は 2 つある。どちらも直した。
- ただし、operator が 13:20Z に 3cad6b9b を手で main へ ff 済みなので、**修正の release が入っても N1 の deliveries の行が
  自動で「配送済み」になる経路はコード上に無い**（読んだ範囲。下の「N1 の行の扱い」）。ループは上限で止まる見込みだが、行は残る。
  この点は実 DB と実 daemon では未確認。

## 原因（正本は ADR [2026-11-06-reuse-pass-delivery-review-run](../adr/2026-11-06-reuse-pass-delivery-review-run.md)）
1. `spawn_review`（`crates/task-dispatch/src/dispatcher/review_spawn.rs`）が `task_ops::delivery::begin` には「reviewer run が無ければ
   worker run id」を渡し、直後の行の照合には「無ければ空文字」を使っていた。reviewer の合格を再利用した回は照合が常に偽になり、
   target_sha・reviewed_sha・merge_candidate_sha が書かれない。MergeQueued の `check_candidate`
   （`crates/celeris/src/delivery.rs`）が Stale を返し、`request_rereview` が再 review に戻す。
2. 配送からの再 review の循環は `ReviewResyncHalted`（`MAX_TARGET_RESYNCS = 2`）に届かなかった。review 入口の
   `pre_review_stale_state` が、直後が `ReviewTargetAdvanced` でない review_pass の `Transitioned` で stale の数を 0 に戻していたため。
   `request_rereview` の attempt は別の数え方（`target_restale_count`）なので 336 まで増えた。

## 変更（commit は件名。基準は `$CELERIS_WU_BASE`、この WorkUnit の前に入った 3 件）
- `n1: reviewer 合格の再利用で配送の SHA が書かれない原因と直し方の ADR` — ADR を実装より先に commit
- `n1: reviewer 合格の再利用でも配送の行に検査済み SHA を書く（review_run を 1 か所で決める）` — `review_id` を 1 回だけ決め、`begin`・
  行の照合・`ReviewTargetSynced` に同じ値を使う。空文字との比較を廃止。回帰試験 `reuse_pass_records_candidate`・`reuse_pass_delivery_reaches_merge`
- `resync-cap: 配送からの再 review の循環でも ReviewResyncHalted で止める` — `pre_review_stale_state` は reason が `review_pass` の遷移では数を戻さない。
  回帰試験 `delivery_rereview_loop_halts_visibly_at_the_cap`・`delivery_rereview_below_the_cap_still_reviews`
- 旧い経路・互換分岐・切替 flag は置いていない（空文字比較は消した）。

## 証拠
前の WorkUnit が ADR に記録した事項（この WorkUnit では再確認していない）: 回帰試験 4 件のうち、`reuse_pass_*` 2 件と
`delivery_rereview_loop_halts_visibly_at_the_cap` は修正前のコードで落ち、修正後に通る。

この WorkUnit の提出時の HEAD で実行した結果は、末尾の「検査（この WorkUnit）」に書く。

## N1 の行の扱い（delivery.rs と周辺を読んだ結果）
読んだ箇所: `crates/celeris/src/delivery.rs` の `advance`・`check_candidate`・`validate_candidate`・`merge_reviewed`、
`crates/task-ops/src/delivery.rs` の `prepare`・`begin`、`crates/task-ops/src/stranded.rs` の merge、
`crates/task-core/src/store/task_integration_state.rs`。

- **deliveries の行を「main に含まれた」として閉じる処理は無い。** `advance` に「head が既定ブランチの祖先なら配送済み」という分岐は無い。
  Ready/Preparing へ進むのは `merge_reviewed`（`git merge --ff-only`）が成功した回だけ。operator が手で ff した commit では、行は遷移しない。
- 加えて `validate_candidate` は `d.base` が現在の既定ブランチと一致しないと失敗する。N1 の行は base = f4e293e2 のままで、main は 3cad6b9b なので、
  仮に行へ SHA が書かれて MergeQueued が Fresh になっても、`merge_reviewed` は「対象コミットまたは既定ブランチが変わりました」で Blocked になる。
  その後は delivery-repair（局所修復）の対象になる経路がある（未確認: N1 で実際に repair WU が作られるか）。
- review 入口の側では、`prepare` が「head が base の祖先」を `NoCommits` で見送る（`DeliverySkipped` を 1 件記録し、`begin` は行を書き換えず None を返す）。
  N1 は main == head なので、修正後の次の review でこの経路に入ると読める。このとき行の base（f4e293e2）と同期結果の target（3cad6b9b）が食い違い、
  `defer_stale_review` が `ReviewTargetAdvanced` を積んで SHA を書かずに review を延期する。今回の上限修正で数が戻らないので、
  `MAX_TARGET_RESYNCS` 回の後に `ReviewResyncHalted`（受信箱・通知・snapshot）で止まる、と読める。**この経路の実挙動は未確認**
  （N1 の状態を再現する試験は書いていない）。
- sweep・`celerisctl integration merge` は `task_integrations` の表を `Integrated` にする（head が既定ブランチの祖先なら `AlreadyIntegrated`）。
  deliveries の行には触らない。`task_integration_upsert` が弾くのは、稼働中の delivery がある間の `Unintegrated` への書き込みだけ。
- したがって、N1 の task の「取り込み状態」が Integrated になることと、deliveries の行が Reviewing / MergeQueued のまま残ることは別である。
  deliveries の行を直接閉じる `celerisctl` の命令は、読んだ範囲では見つからなかった（未確認。task の cancel が行に及ぼす効果も未確認）。

## 未解決
- N1 の deliveries の行の最終的な閉じ方。上の通り自動では閉じないと読めるが、実 DB での確認と、行を閉じる方法（命令があるか、人の判断で放置するか）は未確認。
- main == head の task が review 入口に来たとき、`NoCommits` で見送った後に行を閉じる処理を足すかは、この task の範囲外にした
  （足すなら別 task。下の「提案」）。

## 提案
- `advance` に「head が既定ブランチの祖先（手で ff された場合を含む）なら、行を Ready（取り込み済み）にする」分岐を足すか、
  `prepare` の `NoCommits` 見送りのときに稼働中の行を閉じる。N1 のような手動 ff の後始末が不要になる。別 task で起票する価値がある。

## 本番への移行（人が実行する手順。worker は本番 DB・本番 config・daemon を触らない）
前提: この task が main に入り、`scripts/selfdeploy/release.sh` の gate を通った release が promote 済みであること。N1 の
3cad6b9b は operator が 13:20Z に review 合格済みの commit を手で main に ff 済み。

1. **promote 前の確認（読み取りだけ）。** 本番 DB は sqlite3 の読み取り専用で開く。`$DB` は本番の DB の path。
   ```
   sqlite3 -readonly "$DB" "select state, base, head, review_run, worker_run, target_sha, reviewed_sha, merge_candidate_sha, substr(detail,1,80) from deliveries where task_id='01M6QW0GSG1AVARWC334AZCES1';"
   git -C <取り込み先 repo> merge-base --is-ancestor 3cad6b9b main && echo "main に含まれる"
   ```
   期待: 3cad6b9b が main の祖先であること。行は base f4e293e2・head 3cad6b9b、SHA 3 欄が NULL のまま。
2. **loop の回数を記録する。** promote の前後で rereview の累計が増えるかを見る。
   ```
   curl -s -H "Authorization: Bearer $TOKEN" "$API/api/v1/tasks/01M6QW0GSG1AVARWC334AZCES1/events" | grep -o ReviewTargetAdvanced | wc -l
   ```
   （`GET /api/v1/tasks/{id}/events` は handlers.rs に経路がある。応答の形式と認証の付け方は未確認なので、必要なら調整する。）
3. **promote は通常の手順（`promote.sh`、`SD_ACTOR` を付ける）。** この修正は config 欄を消さないので、本番 config の書き換えは要らない。
4. **promote 後に N1 を観察する（2〜3 tick 分、数分）。**
   - 期待（コードを読んだ結果。実挙動は未確認）: `ReviewTargetAdvanced` の増加が止まり、`ReviewResyncHalted` が 1 件出て、受信箱と通知に項目が出る。
     LLM の reviewer は起動しない。
   - 行が `merge_queued`・`reviewing` を往復し続け、1 周あたり約 17 件の events が増え続けるなら、修正が効いていない。promote を rollback する
     （`rollback.sh`）か、N1 の task を止めて task 側に報告する。
5. **N1 の行の後始末。** 自動では「配送済み」にならないと読めるので、次のいずれかを人が決める。worker はやらない。
   - (a) `celerisctl integration list` で N1 の取り込み状態が `Integrated` であることを確認する。そうでなければ
     `celerisctl integration merge <task>`（head が main の祖先なら `AlreadyIntegrated` で `task_integrations` を Integrated にする）を実行する。
     これは task の取り込み状態だけを直し、deliveries の行は変えない。
   - (b) deliveries の行（Reviewing / MergeQueued / Blocked）をそのまま残す。`ReviewResyncHalted` で止まっていれば害は無い。
     行を閉じる命令が見つからないので、閉じたい場合は「提案」の別 task を待つか、人が DB を直接更新する
     （本番 DB の直接書き込みなので、daemon 停止中に backup を取ってから。手順はこの task の範囲外）。
   - 3cad6b9b の release 準備（release.sh の gate・verify）は、この配送では走っていない。手で ff した commit を release に出すかは operator が決める。
6. **確認コマンド（後始末の後）。**
   ```
   sqlite3 -readonly "$DB" "select state, detail from deliveries where task_id='01M6QW0GSG1AVARWC334AZCES1';"
   celerisctl integration list | grep 01M6QW0GSG1AVARWC334AZCES1
   curl -s -H "Authorization: Bearer $TOKEN" "$API/api/v1/tasks/01M6QW0GSG1AVARWC334AZCES1/events" | grep -o -E 'ReviewTargetAdvanced|ReviewResyncHalted' | sort | uniq -c
   ```
   期待: ReviewTargetAdvanced の数が止まっていること、取り込み状態が Integrated であること。

## 検査（この WorkUnit）
この WorkUnit は文書だけを変えた（コードは変えていない）。提出時の HEAD で実行:
- `sh scripts/dev/progress-index.sh --check` → ok（exit 0）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok
- `sh scripts/dev/check-doc-links.sh` → ok
- `python3 -I scripts/dev/check-human-text.py --self-test` → ok
