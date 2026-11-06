---
tasks: [01M6QW0GSG1AVARWC334AZCES1]
---
# ADR 2026-11-06: 縮小 pilot の測定の契約を fail closed にし、承認を記録の file で受ける（第5フェーズ N1）

- 状態: 採用・実装済み。
- 決めた人: N1（01M6QW0GSG1AVARWC334AZCES1）の worker。[N0 の runbook](../progress/2026-11-06-phase5/runbook.md) の共通の規則
  1・2・5 と、[ADR 2026-11-06-phase5-n0-usd-accounting-contract](2026-11-06-phase5-n0-usd-accounting-contract.md) D2（欠測は fail closed）
  の範囲で決めた。
- 関係: [ADR 2026-11-05-phase3-p0f-usage-counting](2026-11-05-phase3-p0f-usage-counting.md)（usage の数え方。D1 で cache・USD の欠測を足す）、
  [ADR 2026-11-06-phase4-p4g-shadow-rollout](2026-11-06-phase4-p4g-shadow-rollout.md)（G7 の入力を D5 で決める）、
  [ADR 2026-11-04-project-controller](2026-11-04-project-controller.md) D11（実 LLM の門を D2 で置き換える）。

## 背景
N1 で測定器を読み直すと、次の欠測が成功に数えられ得た。
1. `driver.collect_usage` は usage が null の起動だけを欠測にし、cache 欄（`cache_read_tokens`・`cache_creation_tokens`）が無い
   起動と `cost_usd` が無い起動を 0 として足していた。
2. facts の `events_append_only` は常に `true`、`cos_launch_rejected` は常に 0 の固定値で、採点器（`long_horizon::grade`）の
   `events_append_only` の述語は必ず通った。
3. 採点の verdict は usage を見ない。P0-F 前の driver の実 LLM raw（`agent-docs/progress/2026-11-05-phase3/llm-v2/`）は、CoS の
   chat run 14 本の usage が null なのに `missing` が null で、2/2 が pass と採点された。
4. `pc_rollout.py` の G7 は `{cos_runs, missing}` の形の JSON なら何でも受け、stub の pilot でも pass になり得た。
5. 一時 home の KB（`knowledge/`）が空で、案件 CoS の chat run は起動前に「CoS skills unavailable: cos-operator, cos-inbox-triage」で
   失敗していた（usage は残らない）。config-stub で再現した。P0-F の実 LLM pilot の `cos_usage_null:14` も同じ原因と推定する（未確認。
   当時の DB は残っていない）。`--cos-route` を足しただけでは G7 は通らない。
6. daemon の USD 予算は費用の分からない run があると止める（`pc/guard/budget`）。stub・config-stub の agent は usage を出さないので、
   最初の root の後に止まり、C08 は oracle を満たせなかった。

## D1 driver は欠測を数え、固定値を書かない
- `collect_usage` は (invocations, tokens, cache_tokens, usd, missing) を返す。実 LLM の harness で、usage null（`run_usage_null`・
  `cos_usage_null`）、cache 欄の欠落（`cache_missing`）、`cost_usd` が数でない（`usd_missing`）が 1 件でもあれば、`missing` に理由と
  件数を書き、tokens・cache_tokens・usd を null にする。0 として足さない。
- `events_append_only` は実測にする。driver は試行中の毎秒の待ちで、新しく追記された events の行（id → json の SHA-256）を覚え、
  終わりに全行を読み直して、消えた行・書き換わった行を数える（`events_checked` に seen・gone・changed）。1 秒の窓の中の書き換えは
  見えない（限界）。
- `cos_launch_rejected` は案件 CoS thread の `cos_operations` の `rejected` の行のうち、reason に controller が起票を持つための拒否の
  detail（`launches its next root`）を含む数。`cos_ops_rejected` は拒否の総数。`cos_run_failures` は失敗した CoS の chat run の理由ごと
  の数（usage が null の原因を残す）。
- `quota` は測らない。facts は `quota: null`・`quota_missing: "not_measured"` と書き、0 と書かない。

## D2 実 LLM の門は承認の記録の file（`--approval`）
`LONG_HORIZON_PILOT_LLM_APPROVED=1` の環境変数を消し、`long-horizon-pilot-llm.sh --harness config` は `--approval <json>` を必須にする。
`pilot_gate.py check-approval` が次を確かめ、どれかが欠ければ exit 2 で起動しない。
- `approval_id`・`approved_by`（`human:<名前>`）・`approved_at`・`expires_at`（zone 付きの ISO 8601、今より後）・`account`・`model`・`harness`。
- `scope.cases`・`scope.arms` が `--cases`・`--arms` を含み、`scope.environment` が `non-production`。
- `limits.max_usd`・`limits.max_wall_secs` が正の数。`max_tokens` など他の上限は受けない（USD 予算の決定）。`max_quota_pct` は書けるが、
  driver が quota を測らないので、書くと judge は unknown になる。
- `--max-wall-secs` は承認の `max_wall_secs` 以下。既定は承認の値。daemon の `max_usd_per_day` は承認の `max_usd`。
- `--cos-route` が空（`[]`）なら断る（01M6PYNMP4 の driver の検査より前に shell が断る）。
- `CELERIS_RUN_ID` がある（worker の run の中）なら断る（従来どおり）。

## D3 一時 KB に CoS の skill を置く
driver は repo の `config/skills/cos-operator`・`config/skills/cos-inbox-triage` を一時 home の `knowledge/skills/` に写す。無ければ
driver は落ちる。

## D4 stub の agent は usage を出す
`stub_harness.py` は done・question に固定の usage（token・cache 欄・`cost_usd`）を付け、driver と config-stub の TOML は model-1 の
`[budget_rates."model-1"]` を書く。daemon の USD 予算は stub の run も数えられる（LLM は呼ばない）。

## D4b judge（`pilot_gate.py judge`）
採点の後に `long-horizon-pilot-llm.sh` が `pilot_gate.py judge` を呼び、`<out>/grade/pilot-gate.json` と `<out>/grade/pilot-facts.json`
を書く。verdict は pass・fail・unknown で、測れていない項目が 1 つでもあれば unknown、測れて規則に反すれば fail。
- 試行ごと: 実 LLM（`harness_label` が config）、usage の完全さ（facts の `missing` を信じず、invocations を数え直す）、CoS の run ≥ 1
  （0 は fail）、worker・reviewer の run ≥ 1、追記専用（実測が無ければ unknown）、`cos_launch_rejected` = 0、`leak_hits` 空、
  承認の待ちが全部決まっている（期限切れ・未決は fail）、oracle の verdict と `controller_achieved` が揃っている（併記。不一致でも
  fail にしない）、`double_effect` = 0。
- 全体: 承認の記録（無ければ「承認待ち」で unknown）、USD の和 ≤ `max_usd`（USD の欠測があれば unknown）、wall の和 ≤ `max_wall_secs`。
- `g7` は全試行が実 LLM のときだけ pass・fail。stub・config-stub は `pending` で、`cos_usage_null` は null（0 と書かない）。
- `--harness config` で verdict が pass でなければ script は exit 3。stub・config-stub は採点できれば exit 0。

## D5 G7 は judge の出力だけを受ける
`pc_rollout.py preflight --pilot-facts` は `schema` が `celeris.long-horizon-pilot-gate/1` の JSON だけを受ける。それ以外は fail。
`g7` が `pending` なら G7 は pending、`g7` = pass かつ `cos_usage_null` = 0 かつ `cos_runs` ≥ 1 のときだけ pass。

## 検討した案
- 採点器（Rust の `long_horizon::grade`）の verdict に usage を混ぜる: replay・scripted の比較（P0-C・P4-H）の committed baseline が
  変わる。測定の契約は pilot の道具の側の別の判定にした。
- 1 時間 TTL で USD を数え直す列を judge に足す: Python に単価表が無い。N7 の費用対効果の表で扱う（提案）。
