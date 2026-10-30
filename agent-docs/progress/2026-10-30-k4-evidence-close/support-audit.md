---
title: K4 追試の外部 operator 支援の区分の照合（session-map v3）
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 追試の外部 operator 支援の区分の照合（WU support-audit）

完了日: 2026-10-30

状態: 完了。P/report.md §6 の区分（prep・observe・shared・shared_unattributable）を、T/evidence/operator の確定写し（main.jsonl と測定区間の subagent 2 本）と R/run・R/raw の記録に照合した。区間ごとに区分と根拠（transcript の行番号・時刻）を付け、session-map v3 と集計 script を作った。LLM・daemon・実試行・本番配備はしていない。R・P は読むだけ。repo の code は変えていない。

成果物（repo の外、T=/local/celeris/data/workspaces/01M669QAZE5WEQDJBQS8MRX1AK/artifacts/ops-r1）:
- `T/operator/support-audit.json`・`support-audit.md`: 20 区間の区分・v2 の区分・変更の理由・結果への影響（yes/no/unknown）・根拠
- `T/operator/session-map.v3.json`: celeris-campaign operator-usage がそのまま読める配列（v2 からの差分理由は support-audit.json の `v2_category`・`change`・`reason`）
- `T/operator/recount.py`・`recount.json`、`check-rust-ingest.sh`、`SHA256SUMS`

## 結果
- 測定区間（06:38:16Z 以降）の計は 29,639,899 tokens。v3 の区分の和はこれと一致した。shared_unattributable は 2,557,510 から 0 になった。
- v3: prep A-t001 253,962、observe A-t001 1,665,199、prep B-t001 1,667,459、observe B-t001 3,358,892、prep B-t002 6,194,057、observe B-t002 3,971,690、unknown 3,092,622（試行なし）と 820,278（B-t002）、shared 8,615,740、intervene 0。
- shared から試行へ移したもの: B-t001・B-t002 の停止後の保存（CoS 作業場・DB・trace の写しで、採点の入力）。A の実行中に読んだ B の手順は prep B-t001 に、A だけの準備（run-a.sh）は prep A-t001 に移した。
- unknown にしたもの（結果に効いたかを記録から決められない）: 07:01:47 の K4 決定への答え（option=stopped と note）、08:12 の後片付け（§2.8 を事後に設定、operator の worktree を削除、A の残りを B-t002 で隠した dir へ退避）、09:10 の採点 task 01M6617H9V の起票（drift としない判断を伝えた）。
- shared のまま残したが結果に影響したもの: 06:45:30〜06:50:45 の 2 試行共通の準備（4,589,359）。A の account の指定、manifest、daemon.toml、canary を含む。
- 人の発言: transcript に origin.kind=human の行が 4 つある（06:38:16.427Z、06:44:33.864Z、08:12:06.051Z、08:56:17.168Z）。P/report.md は「Bも測り直して…」を「08:20Z ごろ（概算）」としたが、記録の時刻は 08:56:17.168Z（enqueue 08:56:17.115Z）。能動時間と個人の識別子は記録に無い。推測で補っていない。
- WU snapshot の未解決事項（R/raw の 0 byte の -wal を作った process）: B-t002 の operator subagent が db-copy.sqlite を sqlite3 で開いた時刻と mtime が合う（raw/B は 143 行 09:00:30.673Z と mtime 09:00:30.716Z、raw/B-t002 は 356 行 09:08:24〜09:08:43 と mtime 09:08:42.985Z）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 -I T/operator/recount.py T/operator/session-map.v3.json T/operator/recount.json` を 2 回 → 2 回とも exit 0、`cmp` で byte 一致。recount.json の `ok`・`v3.sum_check.ok`・`support_audit_segments_match_v3` は true
- 同じ script の v2 の再計算は P/operator/operator-recount.json と区分ごと・計とも一致（`v2_vs_p_operator_recount.ok` true）
- `cargo build -p celeris-campaign` の後に `sh T/operator/check-rust-ingest.sh <binary> <空の dir>` → exit 0。Rust の取り込みは 10,517 行で、区分の和が recount.json と一致（mismatch なし）
- R 全 file と P/operator 全 file の sha256 一覧を取り直し、T/evidence/inputs-guard の after と `cmp` で一致
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check` の結果は報告本文に書く

## 未解決
- unknown の 3 区間（3,912,900 tokens）が採点や成功の数え方に効いたかは決めていない。人か WU report が扱いを決める。
- P/report.md §1・§8-4 の「08:20Z ごろ」は transcript の 08:56:17.168Z と合わない。WU report で直す。human-ops の b2-approve の時刻欄（null）は書き換えていない。
- session-map は sessionId で区切るので、observe 区間の親の途中報告（06:55:29、09:05:27）を subagent と分けられない。recount の parent/subagent の内訳で示した。

## 提案
- session-map に agent 単位の区切り（agent_id）を足すと、同じ sessionId の親と subagent を別の区分にできる。
- 外部 operator が人の発言の時刻を書くときは、概算でなく transcript の origin.kind=human の行の timestamp を写す。
