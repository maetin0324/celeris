---
title: G7 1 回目（n1-g7-20261106-1）の raw の確認と cos_usage_null 3 件の原因の根拠
tasks: [01M6RGZRNZQMF4PG39HRQ81NRV]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# G7 1 回目（n1-g7-20261106-1）の raw の確認と cos_usage_null 3 件の原因の根拠

完了日: 2026-11-06

## 結論
- G7 の 1 回目は **exit 3**、judge の判定は **verdict unknown・g7 fail**。案件 CoS の chat run **5 本のうち 3 本の usage が null**（`cos_usage_null=3`）。
- USD が分かったのは **12 本の起動で 0.9802 USD**（C06 7 本 0.5794、C08 5 本 0.4008）。合計は judge が null にしている（不明を 0 として足さない）。
- wall は **234 秒**（C06 135 秒 + C08 99 秒。承認の上限は 1 試行 450 秒）。
- C06 は **135 秒・null の CoS は at 134 の 1 本・採点 pass**。C08 は **99 秒・null の CoS は at 72 と at 91 の 2 本・採点 fail**、案件は予算の guard（`pc/guard/budget`、質問 `budget:usd_unknown:2026-11-06`）で止まった。
- 原因は 2 種類と判断する（根拠は下の表）。ただし null の 3 本のどれがどちらの原因かは、一時 DB が残っていないので **一部が未確認**。
  1. CoS run が harness を起動する前に route を選べず失敗した（C08 の `cos_run_failures` に「no declared route candidate for harness claude_code is eligible」1 件）。この行は `started_at` と `finished_at` を持ち usage が無いので、`project_run_usage_between` が費用不明に数え、C08 の案件を `usd_unknown` で止めた。
  2. 試行の最後の操作の後に始まった CoS run が、driver が daemon を止める前に終わっていなかった（C06 の at 134。achieved が at 129、試行の終わりが 135 秒）。
- この観測で、[ADR 2026-11-05-phase4-cos-usage-empty-route](../../../adr/2026-11-05-phase4-cos-usage-empty-route.md) の「daemon 側の修正は不要と確定した」は誤りだったと分かった。訂正と決定は [ADR 2026-11-06-cos-run-launch-accounting-and-drain](../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md)。
- この task ではコードを変えていない。

## raw の取り込み
- コピー元: `/local/celeris/data/scratch/n1-g7/`（原本は読み取りだけ。変更・削除していない）。
- コピー先: [raw/](raw/)。対象と sha256 は [manifest.json](manifest.json)。
- 取り込んだもの（18 file）: `run.log`、`approval.json`、`facts/*.json`、`ops/*.json`、`ops/C06-new.log`・`ops/C08-new.log`、`grade/{grades.jsonl,summary.json,pilot-gate.json,pilot-facts.json}`、`arm-map/*.json`、`trials/*/repo/out/o{1,2}.txt`。
- 取り込まなかったもの: `trials/*/repo/README.md`・`minichunk.py`（case の入力で bench の fixture から作れる）、`approval-run2.json`（2 回目の承認で、1 回目の raw ではない）。sha256 は manifest の `not_copied` に記録した。
- 方法: `cp -p` で 1 file ずつ写し、写した後に原本と内容が一致することを python で確かめた（再帰的なコピーはしていない）。
- 秘密の検査: 次の 3 通りで原本を検査した。秘密の値は 0 件で、除去した箇所は無い（manifest の `secret_scan.redactions` は空）。
  - 秘密の形の正規表現（`sk-ant-`・`sk-`・`bearer`・長い token 値・`api_key`・`password`・`secret`・`authorization`・`oauth`・`refresh_token`・`access_token`・`credential`・`ghp_`・`PRIVATE KEY`）: 0 件。
  - 40 字以上の英数字の列: 全件が sha256 の digest（`init_digest`・`acceptance_sha256`・`bundle_sha256`・成果の evidence）と API の path。
  - 語ごとの件数: `token` 71 件はすべて usage の欄名（`input_tokens` など）、`key` 2 件は採点の述語名 `question_key_opened`。`secret`・`passw`・`cookie`・`auth`・`bearer`・`sk-`・`ghp_`・`PRIVATE` は 0 件。

## 数値表

### 全体
| 項目 | 値 | 出典 |
|---|---|---|
| 終了コード | 3 | `run.log` の最終行「wrote … but the measurement gate is not pass」。この行は `scripts/dev/long-horizon-pilot-llm.sh` で `exit 3` の直前にだけ出る（harness = config かつ judge の exit が 0 でない） |
| judge | verdict `unknown`、g7 `fail`、approval `approved` | `grade/pilot-gate.json`、`run.log` |
| 案件 CoS の chat run | 5 本（C06 2・C08 3） | `pilot-gate.json` の `cos_runs` と試行の `roles.cos` |
| usage が null の CoS run | 3 本（C06 1・C08 2） | `pilot-gate.json` の `cos_usage_null`、facts の `usage.missing` |
| task run の usage null・cache 欠落・USD 未算出 | 0・0・0 | `pilot-gate.json` の `totals.gaps` |
| USD が分かった起動 | 12 本、0.9802 USD | facts の `invocations` のうち usage のある行の `cost_usd` の和（このファイルで再計算） |
| USD の合計 | null（`cost_cap` は unknown） | `pilot-gate.json` の `totals.usd`、`checks.cost_cap` |
| cache creation token | 234,153 | `pilot-gate.json` の `totals.cache_creation_tokens`（C06 139,321 + C08 94,832 と一致） |
| wall | 234 秒（上限 450 秒/試行） | `pilot-gate.json` の `totals.wall_secs`・`checks.wall_cap` |
| quota | 測っていない（0 と書かない） | `pilot-gate.json` の `checks.quota` |

### 試行ごと
試行の id は `raw/arm-map/` の file 名（case と arm の対応はその中身）。

| 項目 | C06 | C08 |
|---|---|---|
| wall | 135 秒 | 99 秒 |
| 起動（worker・reviewer・cos） | 4・2・2 | 2・2・3 |
| CoS の起動時刻（秒） | 48（usage あり）、134（null） | 62（usage あり）、72（null）、91（null） |
| USD が分かった起動 | 7 本、0.5794 USD | 5 本、0.4008 USD |
| input + output token（分かった分） | 8,299 | 6,584 |
| 採点 | pass（述語 15 本すべて pass） | fail（`outcome_reached(o3)`・`outcome_reached(o4)` が fail。o1・o2 は done） |
| 案件の phase の最後 | achieved（at 129） | blocked（ops の at 93.5） |
| controller の規則 | outcome-open、root-in-flight、milestone-reached、outcome-open、milestone-reached、achieved | outcome-open、milestone-reached、outcome-open、**guard/budget** |
| 人への質問 | 無し | `budget:usd_unknown:2026-11-06` |
| `cos_run_failures` | 無し | 「no declared route candidate for harness claude_code is eligible」1 件 |
| 人の承認（公開） | at 49 once、at 97 denied | 無し |
| 最後の操作（ops） | after_off（mode off の後、pending_wakes 1） | after_off at 99.6（pending_wakes 1） |

## 原因の根拠
1. **起動前の失敗が費用不明に数えられた（C08）**
   - `cos_run_failures` の理由は `RouteFailure::NoEligible` の文面（`crates/task-dispatch/src/dispatcher/cos_chat/routes.rs` → `route_select.rs` の `RouteFailure::reason`）。この経路は `launch_claimed` に届かず、`fail_routes` が `chat_run_finish(…, Failed, …)` で usage 無しに行を閉じる。
   - chat run の `started_at` は claim の時点で入っている（`routes.rs` は `run.started_at` から経過時間を引く）。`project_run_usage_between`（`crates/task-core/src/store/project_guard.rs`）は chat run を `started_at` が範囲内で `finished_at` があれば全件返す。task run と違い、起動前に終わった行を除く条件が無い。よってこの行は ADR 2026-11-06-budget-usd-caps D2-3 の「費用不明」になり、`BudgetKind::UsdUnknown` で案件が止まる。C08 の `pc/guard/budget` と質問 `budget:usd_unknown` はこれと合う。
   - budget-usd-caps D2-3 は「harness を起動する前に終わった run は例外」と決めたが、chat run について「CoS chat run は `started_at` のある行だけを数える」としたため、claim 後・起動前の失敗が例外から漏れた。
2. **試行の終わりで CoS run の usage が切れた（C06）**
   - C06 の null は at 134 の CoS 1 本で、achieved（at 129）の後に始まり、試行は 135 秒で終わった。driver（`scripts/dev/long-horizon-pilot/driver.py`）の `wait_end` は phase が achieved・blocked・cancelled になると戻り、`after_end`（mode off）・`collect` の後に daemon へ SIGTERM を送る。CoS run の終わりと usage の書き込みを待つ段は無い。`collect_usage` は `started_at` のある chat run を、終わっていなくても usage null として数える。

## 未確認
- C08 の null 2 本（at 72・at 91）のうち、どちらが起動前の失敗（`cos_run_failures` の 1 件）で、どちらが別の原因かは分からない。もう 1 本は試行の終わり（99 秒）で切れた run と推定するが、未確認。
- 起動前の失敗で、どの候補がどの理由（実 account の CoS +1 の枠、暗黙 account の枠、source の cooldown、CoS の 5 時間枠の予約など）で除かれたかは分からない。route の trace は一時 DB にあり、一時 DB（`/tmp/long-horizon-pilot.*`）は残っていない。
- operator が渡した `--llm-config` の中身（`[accounts]`・`max_runs_per_account`）は raw に無い。driver の config は `max_concurrency = 2` で、`[accounts]` が無ければ既定は 2（CoS は +1 で 3）。
- C06 の at 134 の CoS run の行の state（running のままだったか、SIGTERM で interrupted になったか）は分からない。
- stage の題の「13:21Z」は raw では確かめていない。C06 の公開承認の `created_at`（13:24:17.8Z）と ops の at 48 から、C06 の開始は 13:23:29Z ごろと逆算できる。

## 提案
- ADR 2026-11-06-cos-run-launch-accounting-and-drain の実装（起動の印、枠待ち、driver の終了待機、judge の欄）は別の task で行う。2 回目の承認（`approval-run2.json`、max_usd 2.7）はこの修正が main に入った後の実行を前提にしている。
