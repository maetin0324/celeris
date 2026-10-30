---
title: K4 ops-r1 pilot 消費の区分（内部 task・案件 CoS・統括 CoS・外部 operator）
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 ops-r1 pilot 消費の区分（内部 task・案件 CoS・統括 CoS・外部 operator）

完了日: 2026-10-30

状態: 完了。A-t001・B-t001 の raw と外部 operator の transcript から、役割別・区分別の消費を
`/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/post/consumption.json`（schema `ops-r1-consumption/1`）に書いた。
LLM・daemon は起動していない。DB は写し（`raw/B/db-copy.sqlite`）を `mode=ro&immutable=1` で、本番 DB は `mode=ro` で読んだだけ。

## 結論（token は 4 欄の和）

| 役割 | 試行 | 状態 | tokens | 時間 | 費用 |
|---|---|---|---|---|---|
| A の Claude Code（claude-sonnet-5-5） | A | 観測 | 194,226 | 56 s | 申告額 0.1875 USD（list は null、cash は null） |
| 内部 task（planner/worker/reviewer） | B | not_called（task 0・runs 0） | 0 | 0 | 0 |
| 案件 CoS（claude-opus-5-5） | B | 観測 | 550,684 | 165,976 ms | list 0.612557 USD（pricing_estimate）、cash は null |
| 統括 CoS | B | not_called（thread は project の 1 本） | 0 | 0 | 0 |
| 外部 operator（親 session＋subagent、opus） | 測定区間 | 観測（試行に足さない） | 16,226,360 | null | null |

外部 operator の区分（session-map の窓、試行鍵は ledger の idempotency_key）:
prep A 0・prep B 1,395,559、observe A 1,808,492・observe B 2,593,947、intervene 0（窓を置かなかった）、
shared 7,870,852、shared_unattributable 2,557,510（区間をまたいだ subagent の 1 応答 128,607 と、06:38〜06:45Z の親 session の監督作業 2,428,903）。
和は 16,226,360 で役割の値と一致する。外部 operator の消費は B の測定対象（550,684）の約 29 倍、A（194,226）の約 84 倍で、ほとんどが cache_read（15,798,216）。

結果に影響した外部支援（support_ids）: B に `sup-ops-r1-b-inputs-staging`（CoS が operator の staging から公開 tgz を写した）と
`sup-ops-r1-b-stop-condition-drift`（operator が condition_drift で止めた。人は未確認）。A は無し（依頼文以外の入力の記録が無い。記録が無いことだけで支援なしと断定はしていない）。

監督側（本番 daemon の review chat run 1 件、この task の post 段の worker 3 件（実行中の途中値）、親 operator の監督作業）は `supervision` と
`categories.*.parent` に分け、試行に足していない。

## 作ったもの（repo の外、`artifacts/ops-r1/` の下）

- `post/consumption.json` — trials・roles・categories・supervision・support_ids・known_missing・missing・inputs（各入力の path と sha256）
- `post/consumption/build-consumption.py` — 上を raw から作る script（2 回実行して bytes 一致）
- `post/consumption/operator/session-map.json`・`operator-usage.jsonl` — `celeris-campaign operator-usage` の入力と出力
- `post/consumption/assemble-for-keys/` — 試行鍵を得るためだけに `ops-r1-assemble` を流した scratch（grade-pipeline の `raw/pipe` とは別）
- `raw/operator/<session>/post-2026-10-30T070418Z/` — 外部 operator の transcript の新しい写し（親 main.jsonl は interim、subagent は完了後の final）。interim の写しは変えていない

## 欠測

known_missing（既知）: `trial_cash_unknown`（両試行の cash）、`a_list_cost_not_implemented`（A の list。trials.jsonl の cost_micro_usd は Claude Code の申告額）、
`operator_cost_not_computed`（operator-usage に費用欄が無い）。

missing（token/role/trial の対応の欠落）:
- m-cos-trial-link: ops-r1-attribution は root task が無いと CoS の行を trial に結ばない（unattributed.trial_roots_unknown）。thread の一致で B に結んだのはこの表だけ
- m-b-ledger-tokens-zero: ledger・trials.jsonl の B の tokens は 0 で、CoS の 550,684 を含まない
- m-b-trial-model: trials.jsonl の B の model は manifest の claude-sonnet-5-5、観測は claude-opus-5-5（案件 CoS）
- m-b-trial-wall: trials.jsonl の B は wall 1800 s・outcome timeout。CoS run は 166 s、停止は condition_drift
- m-operator-parent-subagent-same-session: 親と subagent が同じ sessionId で session-map を共有するため、親の無関係な作業（BenchFS の調査）や受信箱の回答が shared・observe(B) に入る（parent 欄で分けた）
- m-operator-boundary-row・m-operator-parent-interim・m-a-prep-window

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `operator-usage`（固定票の celeris-campaign）: main.jsonl added 10347、subagent added 85。同じ入力の再取り込みは両方 added 0
- `ops-r1-assemble`（runbook §7.3 と同じ引数、scratch の out）: exit 0、trials 2
- 検算: A の transcript の再集計 = raw/A/raw.json の 4 欄、B の CoS transcript の再集計 = chat_runs.usage_json = attribution の行（script の assert）
- `python3 -I build-consumption.py` を 2 回: 出力の sha256 一致
- repo の変更はこの進捗ファイルだけ。検査コマンドの結果は報告本文に書く

## 提案

- B の CoS の消費を試行に結ぶ: root task が無い通常運用（CoS が自分で答える）では attribution が trial に結べない。intake の thread_id で結ぶ規則を ops-r1-attribution に足すか、人が決める（g-b-cos-consumption-shared と同じ判断）
- 外部 operator の消費が測定対象の数十倍ある。次の試行では runbook を別 session（親の監督会話と分けた専用 session）で動かし、session-map を session ごとに書けるようにする
- B の trials.jsonl の model・wall を観測値（CoS の model・停止時刻）で書くよう ops-r1-assemble を直すかを report 段で判断する
