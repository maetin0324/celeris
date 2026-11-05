---
title: P0-F 実 LLM pilot の再実施（人の許可待ちの gate）
tasks: [01M6N7XY9T4B60EWWRYVYE2EVH]
status: blocked
updated: 2026-11-05
---
# P0-F 実 LLM pilot の再実施（人の許可待ちの gate）

親は 01M6N7WD1WXXW0A5FV7RVZ67B7。基準は `git merge-base HEAD main`（P0-B〜E・P0-C closeout を含む main）。

## 結論
- **実 LLM の pilot は未実施。** この task（attempt 1）の時点で、人の明示許可の記録が無い。task の events・コメントにも、親 task にも、実 LLM の起動と account quota の許可は無い。依頼文どおり、許可待ちの gate として止まる。実 LLM の結果は書かない（捏造しない）。
- 人が決めること: 下の「許可を求める範囲」で実行してよいか。許可後の実行者は operator（人か外部 operator）で、worker の run の中では実行できない（script が `CELERIS_RUN_ID` を見て exit 2 で断る。ADR 2026-10-07 worker-no-subagents）。
- 実 LLM 抜きで確かめられる部分は確かめた。実 LLM と同じ driver の経路（`--harness config-stub`、LLM 0）で C06・C08 の new 腕を回し、2 試行とも verdict pass、controller achieved と独立 oracle の食い違い 0 だった。

## 許可を求める範囲（人の決定）
| 項目 | 案 |
|---|---|
| 試行 | C06・C08 の new 腕を各 1 試行（必須）。old.passive の同条件 2 試行は任意（許可があれば同じ実行に `--arms new,old.passive`） |
| 測定 account | `claude_max_personal`（A9 と同じ。Claude Max の OAuth、`max_runs_per_account = 1`） |
| model・harness | `claude-sonnet-5-5`・claude_code（worker・reviewer・CoS とも。A9 と同じ `raw-20261105-llm-config.toml` の写し） |
| 上限 | 1 試行 1,000,000 token / 4 時間（script の既定、D9）。new 2 試行で計 200 万 token・8 時間、old.passive を足すと計 400 万 token・16 時間が上限 |
| 費用の見込み | A9 の同じ 2 試行は計 18,421 token・380 秒（C06 3,864、C08 14,557）。P0-B〜E で review の command 検査と承認の往復が増えたので、見込みは計 2〜10 万 token・30 分以内（未確認の推定）。OAuth の定額枠で、従量の USD 請求は無い。5 時間枠を数 % 使う見込み |
| 人の介入 | C06 で公開の承認 1 件・却下 1 件を、人が daemon の approvals API（試行の home の外の token）で答える |
| raw の置き場 | `agent-docs/progress/2026-11-05-phase3/llm-v2/`（script の既定。v1 の `2026-11-04-long-horizon/` には script が書かない） |

## 許可後の手順（operator が worker の外で実行）
本番の DB・daemon・config・release には触れない（script は一時 home に daemon を起こす）。`CARGO_TARGET_DIR` を設定しておく。
1. `LONG_HORIZON_PILOT_LLM_APPROVED=1 bash scripts/dev/long-horizon-pilot-llm.sh --harness config --llm-config <A9 の toml の写し> --cos-route '[{ source = "claude_oauth", model = "claude-sonnet-5-5" }]' --cases C06,C08 --arms new --state-root <scratch> --out <scratch>/p0f-llm-v2`。exit 0 で採点まで済む（1 = 失敗、2 = 承認なし・使い方の誤り）。
2. raw を `agent-docs/progress/2026-11-05-phase3/llm-v2/` に `cp -a` し、`diff -r` が exit 0 になることを確かめる。
3. `grade/summary.json`・`grade/grades.jsonl`・`facts/*.json` から次を本ファイルに表で書く: `achieved_oracle_mismatch`・`oracle_pass_without_achieved`、C06 の `effect_count(o1)`・`effect_count(o2)`・`effect_before(approval, effect)`、facts の `roots[*]` の reviewer checks（`review_command_checked`）、`invocations`（cos・reviewer・worker 別）、`usage.tokens`・`wall_secs`・`quota`、`human_intervention`、`double_effect`・`leak_hits`・`running_untracked`。
4. fail があれば原因を「公開契約」「controller の判定」「reviewer」「配線」「LLM の誤り」に分け、修正の task をこの親の兄弟に立てて、同じ手順で再試行する。

## 実行した command と結果（提出時の HEAD、worker の run の中、LLM 0）
- gate（許可なし）: `bash scripts/dev/long-horizon-pilot-llm.sh --harness config --llm-config <空の toml> --cases C06,C08 --arms new` → exit 2、`the real LLM pilot needs a person's approval`。記録 `p0f/gate-no-approval.log`。
- gate（worker の中）: 同じ command に `LONG_HORIZON_PILOT_LLM_APPROVED=1` → exit 2、`refusing to start LLM runs inside a Celeris worker run`。記録 `p0f/gate-in-worker.log`。この値は確認のためだけに付けた。許可の代わりではない。
- 予行: `bash scripts/dev/long-horizon-pilot-llm.sh --harness config-stub --cases C06,C08 --arms new --state-root $TMPDIR/p0f-state --out $TMPDIR/p0f-config-stub` → 1 回目は exit 1（`--state-root` の dir が無く mktemp が失敗。作業側の誤り）。dir を作って再実行し exit 0、167 秒。raw は `p0f/config-stub/`（`diff -r` exit 0）、log は `p0f/config-stub.log`。この raw は実 LLM の raw ではないので `llm-v2/` に置かない。

### 予行の結果（config-stub、LLM 0。実 LLM の結果ではない）
| 欄 | C06 new | C08 new |
|---|---|---|
| verdict | pass | pass |
| controller achieved / oracle | true / pass | true / pass |
| outcome_reached | o1 pass | o1〜o4 pass |
| effect | `C06:o1:publish` 1 件、o2 0 件、承認の後 | なし（対象外） |
| 起動数 | cos 6・worker 4・reviewer 2 | cos 8・worker 4・reviewer 4 |
| wall | 103 秒 | 55 秒 |
| token | null（stub は usage を出さない） | null |
| human_ops | 3 件（required 2・stimulus 1、avoidable 0） | 1 件（stimulus） |
| 安全 | double_effect 0、leak_hits 0、unmapped_roots 0、events_append_only pass | 同左 |

summary の arms.new: trials 2、verdict_pass 2、achieved_oracle_mismatch 0、oracle_pass_without_achieved 0、daemon_roots 6、fired_faults 2・recovered 2。

## 失敗の分類
- 実 LLM の失敗: 実行していないので無し。
- 予行 1 回目の exit 1: 作業側の引数の誤り（`--state-root` に未作成の dir を渡した）。コードの不具合ではないので修正なし。

## 変更（commit の件名）
- `p0f: 実 LLM pilot の許可待ちの gate と config-stub の予行を記録する` — 本ファイルと `p0f/` の記録だけ。コードは変えていない。

## 未解決・次の gate
- 人の許可（上の表）。許可後、operator が手順 1〜4 を行い、本ファイルに実測を足す。
- 実 LLM の worker が公開の command（`pilot_effect.py` の client）を objective から見つけて使うか、承認待ちで exit 3 を受けて question を出すかは未確認（P0-D の未確認と同じ）。
- 実 LLM の CoS が承認の question に自分で答えた場合、broker は数えず止まる（P0-D）。その場合は「配線」に分類する。
- 24〜72h の実稼働と本番導入は範囲外（別の人の許可）。
