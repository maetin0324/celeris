---
title: K4 ops-r1 pilot 固定票（freeze）
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-29
completed: 2026-10-29
---
# K4 ops-r1 pilot 固定票（freeze）

状態: 完了。固定票を作った。有料試行は 0 件で、承認はまだ無い。

成果物（repo の外）: `/local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/freeze/`

- `gap-resolution.json`: 阻害していた不足 5 件がすべて resolved=true（道具・commit 件名・試験名・確認の exit）
- `fixed-ticket.json`・`fixed-ticket.md`: measurement_id `mlsys-main-4-ops-r1-k4p1`、condition A・B、trial 各 1
- `bin/celeris-campaign`: この branch から build した binary の写し
- `measure/daemon.toml`・`measure/state/`: B の測定 daemon の config（`ops-r1-measure-config.sh --models cos-opus`、port 17731）。daemon は起動していない
- `digests.sha256`: 上の各 file の sha256
- 前回の停止記録 `stop.json` は書き換えていない

## 経緯

attempt 1（03:09Z）は inventory の blocking_gaps が 5 件で止めた。fix 段（task 01M62TJ5V2・01M62TJ5WB・01M62V0FQM・01M62VP61K）の後、道具がある tree で確かめ直した。

attempt 2 の run は、この WU の branch に統合先 branch（celeris/01M62S8XWCZDCY2ZBVQ35176MH）の fix 段を merge した。そのため計画の check 2 件（`crates/ web/ scripts/ config/` の差分 0、範囲は進捗ファイルだけ）が落ちた。attempt 3 で merge を外し、この branch の変更を進捗ファイルだけに戻した（統合先に merge すると fix 段の削除になるので revert は使わなかった）。試験と build は、統合先 branch の tree（fix 段を含む。crates/campaign の git tree の id は fixed-ticket.json の tool_binaries.celeris-campaign.built_from）と同じ中身で走らせた。外した merge commit と統合先の差は、この進捗ファイル 1 つだけだった。

## 証拠（試験・build は統合先 branch と同じ tree で実行。範囲の check は提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `python3 -m unittest scripts/campaign/test_check_ops_r1_leak.py` → Ran 24 tests OK（exit 0）
- `python3 -m unittest scripts/campaign/test_ops_r1_measure_config.py` → Ran 9 tests OK（exit 0）
- `cargo nextest run -p celeris-campaign -E 'binary(ops_r1_intake) | test(ops_r1_intake)'` → 6 passed（exit 0）
- `cargo nextest run -p celeris --test ops_r1_trace --test ops_r1_attr --test ops_r1_measure` → 11 passed（exit 0）
- `cargo build --release -p celeris-campaign --bin celeris-campaign` → exit 0。sha256 a7f7655e5984926b9e30d5b7b21109e6179d995aa02e058e6ada2e921bf2c76a
- `sha256sum -c digests.sha256`（freeze dir で）→ 6 件 OK（exit 0）
- `git diff --quiet 94f86928 <統合先 branch> -- crates/task-* crates/celeris/src config web` → 0（daemon 側の差なし）
- `git diff --quiet cece8633 <統合先 branch> -- <grader のソース・grade-case・private>` → 0

## 未解決

- 本番 release は 2026-10-29T14:48Z に main の最新の commit から作った release（id は fixed-ticket.json の release.production_release_observed）へ替わった。固定票は Objective と決定 b-env の時点の 94f86928dc13 にしたので、本番と一致しない。その release で測るなら新しい condition_id にする。
- 測定 daemon の token の principal は operator:ops-r1 で、ops-r1-intake は author が human: の message だけを記録する。人の principal で送る経路が未決定（preflight・runbook）。
- B の worker への公開入力・kb/snapshot.md の渡し方、A の作業 dir・account、起動直後の DB・org の digest は未確定（fixed-ticket.json の open_conditions）。
- ops-r1 の有料 2 試行の承認は無い。

## 提案

- runbook WU で、承認の decision（試行 2・上限 1,500,000 tokens・1800 秒・model・停止条件・期限）と、本番 release の不一致の扱いを人に問う。
