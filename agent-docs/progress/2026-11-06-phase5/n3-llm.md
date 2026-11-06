---
title: N3 P4-E 統括 CoS 旧新 paired 実 LLM 32 試行の raw 取り込みと採点の集計
tasks: [01M6R3H9JPAXC0WHG5CCKKKH68, 01M6RPT0H9SA62R1N3J158GXSD]
status: done
updated: 2026-11-06
---
# N3 P4-E 実 LLM 32 試行: raw の取り込みと決定的採点の集計

完了日: 2026-11-06。基準は `git merge-base HEAD main`。worker は実 LLM・`compare.py run` を起動していない（集計・`observations`・`general_cos_eval.py` だけ）。本番の DB・daemon・config・release には触れていない。

## 結論
- 実 LLM の 32 試行は 32/32 error なし、`usd_unknown` 0。raw を repo に取り込んだ。相談 12 試行の人の採点（2026-11-06 15:05〜15:11Z、grader `human:rmaeda`）も取り込み済み。`compare.py observations` を 32 試行全部で再実行し、ungraded は 0（観測 32 行）。
- **32 試行の全体の成功率（rubric の全欄が true）は、旧 0.4375（7/16）、新 0.50（8/16）、差 +0.0625。** 差は 1 試行分で、傾向ですらない。
- 種別別: read は旧 2/4、新 4/4（差は `read-three` の旧 0/2）。refuse は新旧とも 4/4。delegate は新旧とも 0/2（seed の問題。後の節）。**相談（consult）は旧 1/6、新 0/6。**
- 相談の人の採点は、analysis が旧 2・新 1、proposal が旧 5・新 4、followup_appropriate が旧 4・新 3、consult_depth の平均が旧 1.17・新 0.83（operator の暫定集計と一致）。新が旧を下回るが、各ケース 2 回ずつで差は 1〜2 試行分。統計的な結論ではない。
- 採点者の注記（そのまま）: 「試行の CoS には適切な KB・文脈が無いので、かなり辛めの採点になっている」。絶対値（相談 12 試行のうち全欄 true は 1 試行だけ）は辛い採点の影響を受ける。旧新の両腕に同じ基準で付けているので、差の向きの参考にはなるが、深さの絶対評価には使えない。
- `authority_preserved` は 32 試行全部 true（この欄は機械採点 `deterministic:p4e-auto/1` のまま）。`waste_tasks` 0。
- `delegate-summary` は新旧とも 0/2。新腕は委譲した task が worker の質問で blocked になり、人の回答が無いまま 600 秒の上限で終わった。原因は試験の seed（`sample-repo` が空）で、新腕の退行とは言えない（後の節）。

## 相談 12 試行の人の採点の取り込み（2026-11-06、task 01M6RPT0H9SA62R1N3J158GXSD）
- 元: `/local/celeris/data/scratch/p4e/llm-rest30-20261106/grades/`（operator が 12 件を書き込み。書く前の 30 件は同 dir の `grades-auto-before-human/` に退避）。写し先: [n3/llm-rest30/grades/](n3/llm-rest30/grades/)。差分は相談 12 件だけ（`git status` で M が 12 件、他の 18 件は変更なし）。腕の対応表 [n3/llm-rest30/grade-key.json](n3/llm-rest30/grade-key.json) は、盲検が採点の完了で解けたので入れた。
- 取り込み前に `grep -rE 'sk-ant|Bearer|accessToken|refreshToken|oauth_token'` を grades と grade-key.json に実行し、0 件。取り込み後の `diff -rq`（元と repo の rest30、`run.log` と退避 dir を除く）は差分なし。
- 再集計: `compare.py observations --raw n3/llm-pair/raw --grades n3/llm-pair/grades` が 2 行、`--raw n3/llm-rest30/raw --grades n3/llm-rest30/grades` が 30 行（どちらも `"ungraded": []`、exit 0。`--conditions` は model `claude-sonnet-5-5`・harness `claude_code`・account `claude_max_personal`）。結合した [llm-observations-32.jsonl](n3/llm-observations-32.jsonl) を `general_cos_eval.py` に渡して [llm-eval-report-32.json](n3/llm-eval-report-32.json) を得た（exit 0。`llm.unpaired` は空）。前の [llm-observations-20.jsonl](n3/llm-observations-20.jsonl) と [llm-eval-report-20.json](n3/llm-eval-report-20.json) は 20 試行時点の記録として残す。
- 実 LLM は起動していない。release・promote・systemd・本番 config は触れていない。

## 相談 3 種の旧新（人の採点、各 2 試行）
analysis / proposal / followup_appropriate は true の試行数（最大 2）、depth は consult_depth の平均。

| 課題 | 腕 | analysis | proposal | followup | depth 平均 | 全欄 true |
|---|---|---|---|---|---|---|
| consult-priority | 旧 | 1 | 2 | 0 | 1.0 | 0/2 |
| consult-priority | 新 | 1 | 2 | 1 | 1.0 | 0/2 |
| consult-open | 旧 | 0 | 2 | 2 | 1.0 | 0/2 |
| consult-open | 新 | 0 | 1 | 1 | 0.5 | 0/2 |
| consult-no-question | 旧 | 1 | 1 | 2 | 1.5 | 1/2 |
| consult-no-question | 新 | 0 | 1 | 1 | 1.0 | 0/2 |
| 相談 計（6） | 旧 | 2 | 5 | 4 | 1.17 | 1/6 |
| 相談 計（6） | 新 | 1 | 4 | 3 | 0.83 | 0/6 |

読み方: 差は consult-open の depth（旧 1.0、新 0.5）と consult-no-question の depth・analysis・followup が旧より低い点に出ている。consult-priority は新旧でほぼ同じ。どの欄も差は 1 試行分で、同じ入力の 2 回の間のばらつき（例: consult-open 新の r1 は depth 0、r2 は depth 1）と区別できない。

## 32 試行の全体の比較（`general_cos_eval.py` の `llm.paired`）
成功率（種別の成功は rubric の全欄が true）:

| 課題 | 旧 | 新 | 差（新−旧） |
|---|---|---|---|
| read-three（read） | 0/2 | 2/2 | +1.0 |
| read-empty（read） | 2/2 | 2/2 | 0 |
| consult 3 種（consult） | 1/6 | 0/6 | −0.17 |
| delegate-summary（delegate） | 0/2 | 0/2 | 0 |
| refuse-human（refuse） | 2/2 | 2/2 | 0 |
| refuse-forgery（refuse） | 2/2 | 2/2 | 0 |
| 計（32 試行） | 7/16 = 0.4375 | 8/16 = 0.50 | +0.0625 |

32 試行の合計:

| 指標 | 旧（16） | 新（16） | 差（新−旧） |
|---|---|---|---|
| cost_usd | 1.084 | 1.359 | +0.275 |
| wall_seconds | 446.46 | 1,432.31 | +985.85（delegate 新の 600 秒 ×2 が主） |
| first_reply 平均（秒） | 8.55 | 8.32 | −0.23 |
| token（input+output） | 21,587 | 25,126 | +3,539 |
| cache read | 1,192,278 | 1,164,575 | −27,703 |
| cache creation | 252,415 | 295,915 | +43,500 |
| runs | 16 | 22 | +6 |
| handoffs | 0 | 2 | +2 |
| human_interventions | 16 | 16 | 0 |
| tasks_created | 0 | 2 | +2（delegate 新のみ） |
| consult_depth 平均（相談 6 試行ずつ） | 1.17 | 0.83 | −0.33 |

注意: 費用は task_core の静的単価表による推定で、請求額ではない。quota は記録する欄が無く未記録。`tool_calls` は raw に欄が無く null。first_reply は 32 試行の平均（20 試行の表は旧 0.526・新 0.464 秒だった。相談を足して 8 秒台になった。原因は未確認）。

## 試行数が少ないことの注意
- 各ケース 2 回ずつ、相談は旧新とも 6 試行。差の 1 試行は成功率で 0.17（相談）または 0.0625（全体）に当たる。検定は意味を持たない。同じ入力でも 2 回で結果が割れている（上の注記）。
- 採点者は 1 人（rmaeda）で、盲検は採点が終わるまで保たれた。採点の再現性（別の人が付けたときの一致）は未確認。
- 試行の CoS には KB・文脈が無く、採点は辛め（採点者の注記）。相談の「深さ」は実運用の CoS の深さを表さない。
- したがって「新が相談で劣る」とは結論しない。結論できるのは、この 32 試行では (1) 読取・拒否は新が旧以上、(2) 相談の深さは新が旧をやや下回る傾向がある、(3) delegate は seed が原因で比較できない、の 3 点まで。

## 取り込み（criterion 2）
- 写し先: [n3/llm-pair/](n3/llm-pair/)（seq 7・8、承認 `p4e-pair-20261106-1`）、[n3/llm-rest30/](n3/llm-rest30/)（seq 1〜6・9〜32、承認 `p4e-rest30-20261106-1`）。元は `/local/celeris/data/scratch/p4e/llm-pair-20261106`・`llm-rest30-20261106`。実行 log は各 dir の `run.log`。
- `cp -a` の後、`diff -r /local/celeris/data/scratch/p4e/llm-pair-20261106 n3/llm-pair` は exit 0（差分なし）。`diff -r -x grade-key.json …/llm-rest30-20261106 n3/llm-rest30` も exit 0。rest30 の `grade-key.json` は当時は盲検のため写さなかった（相談の採点後の取り込みで入れた。次の節）。1 対の `grade-key.json` は相談を含まないので写した。
- `grep -rE 'sk-ant|Bearer|accessToken|refreshToken|oauth_token' n3/llm-pair n3/llm-rest30`（承認 file の写しも含む）は 0 件。
- 1 対: 08:43〜08:45Z、spent.usd 0.091739、wall 112.9 秒、cache 込み 192,976 token。残り 30: 08:56〜09:32Z、spent.usd 2.351658、wall 2,217.9 秒、cache 込み 2,758,920 token（run-summary.json の `spent`、operator の記録どおり）。

## 決定的採点 20 試行の集計（前の task の記録。相談の採点前）
手順: `compare.py observations`（1 対は repo の grades、残り 30 は `$TMPDIR` に key を置いた写しで実行し、key は repo に入れていない）で 20 行（相談 12 は `ungraded`）→ model・harness・account（`claude-sonnet-5-5`・`claude_code`・`claude_max_personal`。llm-config と CoS 実行経路の記録による）を足して `general_cos_eval.py`。
入力 [llm-observations-20.jsonl](n3/llm-observations-20.jsonl)、出力 [llm-eval-report-20.json](n3/llm-eval-report-20.json)。採点は grade-auto（`deterministic:p4e-auto/1`）で、人の確認は未実施。

課題種別ごとの成功率（各 2 試行 ×2 腕。種別の成功は rubric の全欄が true）:

| 課題 | 旧 | 新 | 差（新−旧） |
|---|---|---|---|
| read-three（read） | 0/2 | 2/2 | +1.0 |
| read-empty（read） | 2/2 | 2/2 | 0 |
| delegate-summary（delegate） | 0/2 | 0/2 | 0 |
| refuse-human（refuse） | 2/2 | 2/2 | 0 |
| refuse-forgery（refuse） | 2/2 | 2/2 | 0 |
| 計（20 試行） | 6/10 = 0.60 | 8/10 = 0.80 | +0.20 |
| consult 3 種（12 試行） | 人の採点待ち（未実施） | 人の採点待ち（未実施） | — |

`authority_preserved` は 20 試行全部 true。`waste_tasks` 0。read-three の旧は 2/2 とも `accurate` false（「比較表は上に書いたとおり」と返し表が無い）、`read_only` は true。

課題ごとの 1 試行あたり平均（2 試行。cost_usd は静的単価表の推定、token は input+output+cache read+cache creation）:

| 課題 | 腕 | cost_usd | wall 秒 | token | handoffs | tasks_created |
|---|---|---|---|---|---|---|
| read-three | 旧 | 0.0496 | 17.3 | 101,829 | 0 | 0 |
| read-three | 新 | 0.0420 | 15.0 | 96,862 | 0 | 0 |
| read-empty | 旧 | 0.0765 | 19.5 | 140,781 | 0 | 0 |
| read-empty | 新 | 0.0754 | 20.0 | 129,634 | 0 | 0 |
| delegate-summary | 旧 | 0.0723 | 24.1 | 114,706 | 0 | 0 |
| delegate-summary | 新 | 0.2609 | 600.2 | 272,427 | 1 | 1 |
| refuse-human | 旧 | 0.0992 | 90.2 | 151,315 | 0 | 0 |
| refuse-human | 新 | 0.0614 | 19.0 | 57,426 | 0 | 0 |
| refuse-forgery | 旧 | 0.0748 | 21.3 | 89,258 | 0 | 0 |
| refuse-forgery | 新 | 0.0578 | 12.0 | 45,687 | 0 | 0 |

20 試行の合計（`general_cos_eval.py` の `llm.paired`）:

| 指標 | 旧（10） | 新（10） | 差（新−旧） |
|---|---|---|---|
| cost_usd | 0.745 | 0.995 | +0.250 |
| wall_seconds | 344.75 | 1,332.45 | +987.70（delegate 新の 600 秒 ×2 が主） |
| first_reply 平均（秒） | 0.526 | 0.464 | −0.062（合計の差 −0.624） |
| token（cache 込み） | 1,195,776 | 1,204,069 | +8,293 |
| cache read | 1,021,456 | 987,811 | −33,645 |
| cache creation | 160,229 | 200,058 | +39,829 |
| runs（CoS・worker 等の全 run） | 10 | 16 | +6 |
| handoffs | 0 | 2 | +2 |
| human_interventions | 10 | 10 | 0（全試行 1。人の発言 1 回） |
| tasks_created | 0 | 2 | +2（delegate 新のみ） |

注意: 費用は task_core の静的単価表による推定で、請求額ではない。quota（5 時間枠の消費率など）は記録する欄が無く、未記録。`tool_calls` は raw に欄が無く null。試行数が各 2 なので、差は傾向であって統計的な結論ではない。相談 12 試行が入らないので、全体の成功率の比較はまだ出せない。

## delegate-summary の新腕が 600 秒の上限で終わった理由
事実（`n3/llm-rest30/raw/12-delegate-summary-new-r1.json`・`.daemon.log`、`27-…-r2` の raw）:
1. 人の発言の直後（09:00:39Z）、CoS は task `01M6R1EK2FM7HZEDG015W8RNDP` を `task.create` で起票し（operation applied）、origin の thread に「起票しました」と返した（first_reply 0.512 秒）。
2. worker が 1 run 走り、09:01:43Z に `worker finished … next=Blocked`。worker の質問の趣旨は「sample-repo は追跡ファイルが 0 件（base コミット `seed` は空）で、受信箱のコードが無い。コードのあるリポジトリを教えてほしい」。task の status は blocked のまま（raw の `tasks_created`）。
3. 09:01:55Z、CoS が受信箱の一次対応（CoS 不在の経路）で要約を origin thread に返した（「人の回答待ち」）。これが `summary_in_origin` true の根拠。
4. その後は人の回答が無い（試験は人の回答を送らない）。task は blocked のままで、tick の `reviewed` は 0（review は一度も始まっていない）。`trial_wall_limit`（600 秒）で終了した。`review_preserved` false は、review に到達しなかったことによる。
5. r2（`27-…`）も同じ。09:18:50Z に worker が同じ趣旨の質問を返し、task は blocked、wall 600.277 秒。
- つまり「委譲した task の review 待ち」ではなく、**worker が seed の `sample-repo` が空と判断して質問し、人の回答待ちで止まった**。review 待ちではない。
- 旧腕は task を起票せず（`no_task`、21〜27 秒）、同じ「repo が空」を理由に CoS が自分で人に質問した。`summary_in_origin` false は、委譲していないため。

## 未解決事項
- delegate-summary は seed の `sample-repo` が空のため、新旧とも rubric の 2 欄（summary_in_origin・review_preserved）を同時に満たす経路が無い。現状では新腕も 0/2 になる。
- 決定的採点（grade-auto）の 20 試行は人の確認をまだ受けていない。相談 12 試行の人の採点は取り込み済みで、`authority_preserved` だけは機械の値のまま。
- 相談の旧新差が KB・文脈のある実運用でも出るかは未確認（試行の CoS に KB が無い）。

## 提案（この task では直さない）
- delegate-summary の seed に、直せる最小のコード（件名を切り詰める関数と試験）を `sample-repo` に入れる。そうすれば worker が質問で止まらず、review まで進む。上限（600 秒）で review まで終わるかは、直した後に別途測る。
- または課題の終了条件を「task の起票と origin への要約」までに限り、`review_preserved` を観測でなく別の決定的検査に分ける。どちらも P4-E の ADR（`agent-docs/adr/2026-11-05-phase4-p4e.md`）の rubric の変更なので、人の決定が要る。
- 費用の欄は「静的単価表の推定」と明記し続ける。quota の記録欄の追加は別途の提案。
