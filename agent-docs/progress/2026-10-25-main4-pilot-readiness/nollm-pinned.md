---
title: main-4 pilot readiness — pin した campaign binary で無評価 LLM の 12 段通しと照合を取り直す（nollm-pinned、mlsys-main-4-v4）
tasks: [01M5WJTMZE0YWRMZE900NDPYRG]
status: done
updated: 2026-10-26
---
# main-4 pilot readiness — pin した campaign binary で無評価 LLM の 12 段通しと照合を取り直す（nollm-pinned、mlsys-main-4-v4）

完了日: 2026-10-26。repin で pin した campaign binary そのもので、stub harness・fake model による 12 段
（prepare→start→cap-probe→preflight→env-lock→watch→collect→grade→check→export→human-op report→final-accounting）を
すべて exit 0 で通した。同じ out で checker・export・human-op report・final-accounting の件数を照合し、食い違いは 0 件だった。

以下のパス表記:
- N = `/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready`
- R2 = `N/bundle/final-freeze-2`（`N/repin/repin.json` の root）
- RP = `N/repin`（機械向け記録 `RP/nollm-pass.json`（schema `celeris.main4-nollm-pass/2`）・`RP/consistency.json`、作業記録 `RP/notes/{nollm-run,nollm-post,consistency}.md`、作業場所 `RP/nollm`）

## binary
- 使った binary: `R2/pinned/evidence/main-k2/bin/celeris-campaign-fixed`（写しでも再 build でもない。launcher.env の CAMPAIGN_BIN は R2 から変えていない）
- sha256: `2344d2c47b001e66560a778e964d9e5c2f2b9f5108c552016b727f5105b2b402`
  - `RP/repin.json` の campaign_bin_sha256、`RP/nollm-pass.json` の campaign_bin_sha256、`RP/consistency.json` の campaign_bin_sha256（campaign_bin_matches_pin: true）と一致。
  - close 時に `sha256sum` で取り直しても同じ値。
- measurement_id: mlsys-main-4-v4（repin.json と同じ）。測定 daemon は 127.0.0.1:7743（本番・repin の port 7741 と別）。

## 12 段の exit と ID（`RP/nollm-pass.json` の stages から）
共通 ID: measurement_id mlsys-main-4-v4、daemon 段の run 01M5WJZMYGDNW5SEPA7R4J8AQV、post 段の run 01M5WKNVY44TE0Y48SRMMJESKH、
daemon instance 01M5WK3KF0N0KRMPW70MFQGT8Z、project 01M5WK3V96X5EJW82DHCF3C4ZP。fixture はすべて「stub harness: no LLM」。

| # | 段 | exit | real_daemon | 段固有の ID |
|---|---|---|---|---|
| 1 | prepare | 0 | false | — |
| 2 | start | 0 | true | — |
| 3 | cap-probe | 0 | true | B probe task k2 01M5WK41PJ26VQ8PTZMYXE0HD5、k4 01M5WK47QHM3DHTB2J3HZY6YF7 |
| 4 | preflight | 0 | true | probe task 01M5WK4V5K1B2Q8T6ZYQHP975W、01M5WK514VB492AC7347G752DS |
| 5 | env-lock | 0 | true | lock sha256 `94e46d2a1567f89fe7db0a1c4ae52d6a43d31c15a409efed4d7b097b488046dd`、env-lock --verify: verified |
| 6 | watch | 0 | true | B/k2 task 01M5WK6TK2EKJ502YFZ2S5EXDY、最後の run 01M5WK8MVZ7XAWF884XGE3ZECN（15 run） |
| 7 | collect | 0 | false | ok、not_submitted 2・skipped_no_experiment 1・blocking 0 |
| 8 | grade | 0 | false | campaign_sha256 `2b52673637066bd0ac17bc36e817ca4cb10ee2230c64950df6bf054d5d68cb02` |
| 9 | check | 0 | false | ok=true、violations なし、ungraded A 0・B 0、complete=false（B: 1 試行未実行） |
| 10 | export | 0 | false | table.incomplete.json/csv・final-accounting.json、lock_verified true |
| 11 | human-op-report | 0 | false | trials 4、shared_overhead_records 2 |
| 12 | final-accounting | 0 | false | CLI 出力と export の final-accounting.json が一致 |

後始末: DB 写し（watch 後、sqlite backup API、integrity_check ok、sha256 `68d204ec4ab16605341000aa6ae5bfe2cc7ce4bf0c687a614a75bf7c966b547f`、post 段の後も不変）、
`measure-launcher.sh stop` exit 0（port 7743 listen なし、daemon process なし）。

## 照合（`RP/consistency.json`）
- 実行: `python3 scripts/campaign/check-out-consistency.py --out RP/nollm/out --manifest RP/nollm/final.json --campaign-bin <上の binary> --human-ops-report RP/nollm/human-ops-report --final-accounting RP/nollm/out/export/final-accounting.json --check-arg=...`（launcher と同じ資産引数・--measurement-id・--cap-env を渡す。全文は consistency.json の cmd）→ exit 0
- 結果: consistent true、mismatches []、sources は checker・export・human_op_report・final_accounting の 4 つすべて produced（exit_stage 0）。
- cell の分類: A/k2・A/k4 は model_failure（grader_failed 2 は checker・export・raw で同数）、B/k2 executor_failed、B/k4 executor_cancelled（未実行）。
- final-accounting との照合: arm ごとの n・ran・not_run・tokens 欠測 cell（A 2/2/0/0、B 2/1/1/1）、missing_trials 1、observed_trial_tokens 210、manifest_hash・campaign_id が raw と一致。

## 欠測と wait 未導出
- 欠測は null のまま数え、0 で埋めていない。final-accounting の total（tokens・cash/list cost）は null、complete はすべて false。export の cost も A・B とも null（cost_status missing）。token 欠測試行は A 0・B 1。
- consistency.json の missing は 25 項目（評価者記録なしの human_time・interventions・rework、stub の cost・tokens・executor_reason 等）。
- wait 区間で導出できたのは B/k2 の celeris_wait（event 由来）だけ。human_wait・internal_wait・external_queue_wait・external_run は全 cell、celeris_wait は A/k2・A/k4・B/k4 で未導出（評価者記録なし・cluster 投入なし・B/k4 未実行）。cluster_* の時間は decision-log 15 run 全行で欠測。

## 共有 setup の計上
- 1 回だけ計上した（counted_times 1）。human-op の shared scope 1 op（logical op `shared/setup-prepare-to-env-lock`、record 呼び出し 1 回、jsonl 2 行、mode automatic・actor script）。前回の rerun では 0 回だった。
- human-op report の shared_overhead は campaign mlsys-main-4-v4 に 1 度だけ載り、arm には配分しない（corrections 0）。automatic なので人の時間は 0。
- final-accounting の shared 行は 0。cap-probe の probe 8 run は campaign tag が無く records に入らない（stub で tokens null のため金額差は無い）。

## 不変と費用
- R2 の全 file と実台帳 `/local/celeris/data/measure/main-4/global-ledger`（計 97 行）の sha256 一覧は、通しの前・daemon 段の後・post 段の後で差分 0 行。台帳は写し `RP/nollm/global-ledger` に ledger-init した。
- paid_calls_made 0（nollm-pass.json・consistency.json とも）。A・B は stub、llm source は fake の measure。測定 token の値は記録・log に 0 件（grep -F）。
- pilot・主測定・有料 LLM は実行していない。本番の systemd・`~/.config/celeris`・本番 DB は触っていない。

## 受け入れ条件ごとの結果
- 全段 exit 0、paid_calls 0: 上の表の 12 段すべて exit 0、paid_calls_made 0。
- 通しの binary が pin と同じ: path は R2 の pin そのもの、sha256 は repin.json と一致（`sha256sum` で再確認）。
- checker・export・report・final-accounting が同じ out で一致: consistency.json consistent true、mismatches []。
- 進捗 file がある: この file。

## 変更（この task branch の commit、件名）
- `consistency: check-out-consistency.py に final-accounting の照合を足し、共有 human-op を identity で判定する`
- `phase 1: restore consistency scope and record pinned recheck`
- `nollm-pinned: 12 段通しと照合の進捗を記録する`（本 unit。進捗 file だけ）
- nollm-run・nollm-post は repo を変えていない（成果は RP にある）。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sh scripts/dev/check-doc-links.sh` → exit 0
- `sh scripts/dev/check-adr-numbers.sh` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 未解決
- pin した binary は task branch から build した物で、collect 修正はまだ main・release gate を通っていない。release の binary に差し替えるなら repin（build-pin → lock-probe → live → pilot）と本通しのやり直しが要る。
- cap-probe の probe run が campaign tag を持たず final-accounting に入らない。実測では probe の消費が欠測になる。
- R2 の final.json の approved_trials は 2 のまま（refreeze.md・repin.md の未解決と同じ）。

## 提案
- cap-probe・preflight の probe task にも campaign tag を付け、final-accounting の shared 行に計上する。
