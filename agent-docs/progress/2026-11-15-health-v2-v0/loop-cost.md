---
title: 健全性v2 V0 loop-cost — 検査の追加費用・遅延・誤検出と開発 loop の実績（本番 DB の読み取り）
tasks: [01M6TYVJT97XH88TEEAVRD7JK6]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 V0 loop-cost

結論: B0 の窓の数え直しは B0 の公表値と一致した（差 0）。後の窓では reviewer run の Claude の中央値が 0.4521 → 1.8586 USD（約 4.1 倍）になった。health が足した時間は WU の check の秒の 10.6%、段の統合の check の秒の 5.1%。設計 review は 39 件・64.48 USD。health の fail 35 件のうち「指摘の path を変えずに通った」は 0 件、reviewer の拒否 63 件のうちでは 2 件。

- 集計 script: `agent-docs/progress/2026-11-15-health-v2-v0/loop-cost.py`
- 記録: `agent-docs/progress/2026-11-15-health-v2-v0/loop-cost.json`（`windows.after.end` = 2026-11-15T23:34:53Z。数値は `measures`）
- DB は sqlite3 の URI `file:/local/celeris/data/db/celeris.sqlite3?mode=ro` でだけ開く。script は `mode=ro` の無い URI と素の path を exit 1 で拒む。本番 DB と host には書いていない。
- 窓: B0 の窓 = `runs.started_at` が `2026-10-24` 以上 `2026-11-07T12:00` 未満（B0 の `task-history-collect.py` と同じ文字列比較）。後の窓 = `2026-11-07T12:00` 以上、JSON に固定した終端未満。後の窓の reviewer run は、終端までに終わった run だけを数える（usage は run の終了時に書かれるので、終端を固定すれば数え直しても同じ値になる）。events は `ts` で窓に入れる。

## (a) reviewer run の本数・費用（B0 との比較）

出典は `runs`（role=reviewer）の `usage_json`。tokens は 4 種（input・output・cache_read・cache_creation）の和。分位は B0 と同じ（中央値は `statistics.median`、p90 は `sorted(xs)[int(0.9*(n-1))]`）。

| 窓 | 系 | 本数 | usage あり | cost あり | USD 合計 | USD 中央値 | USD p90 | tokens 中央値 | tokens p90 |
|---|---|---|---|---|---|---|---|---|---|
| B0（公表値） | Claude | 517 | 351 | 326 | 165.73 | 0.4521 | 0.9155 | 494,692 | 1,347,122 |
| B0（数え直し） | Claude | 517 | 351 | 326 | 165.73 | 0.4521 | 0.9155 | 494,692 | 1,347,122 |
| B0（数え直し） | GPT | 34 | 31 | 0 | 未計測 | 未計測 | 未計測 | 633,586 | 1,728,388 |
| 後 | Claude | 188 | 159 | 149 | 307.40 | 1.8586 | 3.8959 | 3,664,821 | 10,261,058 |
| 後 | GPT | 8 | 4 | 0 | 未計測 | 未計測 | 未計測 | 942,740 | 984,920 |
| 後・設計 review だけ | Claude | 38 | 38 | 38 | 64.48 | 1.6310 | 2.7791 | 3,154,681 | 5,595,447 |
| 後・final review だけ | Claude | 150 | 121 | 111 | 242.92 | 2.1359 | 4.2427 | 4,227,932 | 11,072,271 |

- B0 との差: 0。理由: B0 は窓の終端（12:00Z）の後の 12:20:58Z に数えており、窓の中の Claude の reviewer run で B0 の計測時に終わっていなかった run は 0 本だった（JSON の `b0_recount_vs_published.claude_runs_unfinished_at_b0_measure`）。runs の行は消されていない。
- 設計 review の run は `DesignReviewed.review_run` で見分けた。B0 の窓には設計 review が無い。
- 後の窓で 1 本あたりの費用が上がった原因は、health の材料の追加と、task の大きさの違い（健全性 v2 の子 task は大きい）を分けて測っていない（未確認）。

## (b) health が足した費用と遅延

event の kind は `crates/task-core/src/model/events.rs` の doc に従った。health の証跡の正本は経路ごとに 1 つ: WU の check = `WorkUnitCheckFinished.health`、段の統合 = `IntegrationCheckFinished.health`、final review = `RequiredChecksEvaluated.results[].health`。final review の材料は `HealthReviewInputBuilt`・`HealthReviewInputSkipped`、設計 review は `DesignReviewRequested`・`DesignReviewed`。B0 の窓ではどれも 0 件（health はまだ入っていなかった）。後の窓の値:

| 経路 | 実行 | pass | fail | error | 秒の合計 | 秒の中央値 | 秒の p90 | 同じ経路の check 全体の秒 |
|---|---|---|---|---|---|---|---|---|
| WU の check | 271 | 241 | 27 | 3 | 21,828.7 | 94.9 | 136.7 | 206,783.5（health は 10.6%） |
| 段の統合 | 123 | 117 | 6 | 0 | 10,888.2 | 97.5 | 148.9 | 213,491.0（health は 5.1%） |
| final review の必須検査 | 128 | 125 | 2 | 1 | 未計測 | 未計測 | 未計測 | — |

- final review の材料: built 117（材料の verdict は pass 103・error 12・fail 2）、skipped 33（reviewer run の無い review 20、remote workspace 12、review 前の同期が無い 1）。秒は未計測。
- 時間切れ（timed_out）は 0 件。`HealthFindingsRepeated`（修復の後も同じ指摘が残って止めた）は 3 件。
- 設計 review: 依頼 39、判定 39（pass 23、fail 16）。Claude の費用 38 本で 64.48 USD（中央値 1.631、p90 2.7791）。後の窓の reviewer の Claude の USD の 21.0%。run の壁時計の合計 9,419.5 秒（中央値 206 秒）。依頼から判定までの中央値 214.6 秒、p90 435.7 秒。

## (c) 誤検出

### 判定の規則

対象は次の 3 つ（窓は fail・拒否の時刻で決め、後で通ったかは終端までの event で見る）。

- health の fail: 上の 3 経路で verdict が `fail` か `policy_only` の証跡。site は (task, 経路, WU の key か統合の key か repo)。`error`（検査器が評価できなかった）は指摘ではないので `error_not_finding` に分け、誤検出の分類に入れない。
- reviewer の拒否: `transitioned` で reason が `review_fail` の遷移。拒否した reviewer run は、同じ task の設計 review でない reviewer run のうち遷移の時刻までに始まった最後の run。
- 設計 review の拒否: `DesignReviewed.pass = false`。

指摘の path は次から取る。health: fingerprint ごとに、events の本文の `health:<fingerprint>（evidence: <path>, …）`（WU の check の失敗の詳細など）と、必須検査の summary の `FAIL <rule> <path>:` の行。reviewer: その run の不合格の `review_verdict.reason`（`reviewer(<run>)` で始まるもの）と `worker_finished.outcome` に出る repo の path（`crates/`・`scripts/`・`web/`・`docs/`・`agent-docs/`・`tests/`・`config/`・`tools/`・`.celeris/` で始まる語）。拒否の側か通った側の commit の tree に無い path は捨てる。

1 件ごとに次の規則を上から順に当て、最初に当たったものを分類にする。

1. `not_passed`: 同じ site（reviewer は同じ task の次の `review_pass`、設計 review は同じ task の次の pass）で終端までに通っていない。
2. `policy_changed`: health の fail の blocking に `policy.changed` の fingerprint があるか、verdict が `policy_only`（policy の変更で止まった）。
3. `human_decision`: fail・拒否から通るまでの間に、同じ task に人の決定の event（`decision_answered`・`answered`・`approval_decided`・`plan_gate_decided`・`integration_answered`）か、reason が `answer`・`comment`・`approve`・`accept` の遷移がある。
4. `exception`: fail・拒否の commit から通った commit までの差分（`git diff --name-only A...B`）が例外・policy の file を変えた。例外・policy の file は health の profile（`scripts/dev/repo-health/profiles/`）、`.celeris/`、名前に `exception`・`allowlist` を含む file、名前に `baseline` を含む json・toml（`agent-docs/` の計測記録は除く）。
5. `no_paths`（path が拾えない）・`git_unavailable`（commit が repo に無い）: 判定できない。
6. `passed_without_path_change`: `git diff --name-only A B -- <指摘の path>` が空（指摘の path を変えずに通った。誤検出の候補）。空でなければ `changed_then_passed`（直してから通った）。

設計 review の拒否は計画が commit でないので 6 を当てず、1・3 の後は `passed_after_replan` にする。4 の差分は B 側が main を取り込んでいると他の task の変更も含むので、`exception` は上限の数である。

### 件数（後の窓）

| 対象 | 件数 | 指摘の path を変えずに通った | 例外 | 人の決定 | policy.changed | 直して通った | 判定できない | 通っていない |
|---|---|---|---|---|---|---|---|---|
| health の fail | 35 | 0 | 0 | 7 | 6 | 19 | 1（no_paths） | 2 |
| reviewer の拒否 | 63 | 2 | 2 | 15 | — | 14 | 30（no_paths） | 0 |
| 設計 review の拒否 | 16 | 未計測 | — | 5 | — | 11（replan の後に pass） | — | 0 |

- health の `error` は 4 件（WU の check 3、final review 1）。例の理由は `unsupported:arch-map.missing_row(checker_missing: …)`（R1・R3 の WU、2026-11-10）。
- policy.changed で止まった health の fail は 8 件（後で通った 6 件と、R5 の WU `resync-g4` の 2 件。後者は終端までに通っていない）。
- B0 の窓（health が入る前）の reviewer の拒否は 110 件: 指摘の path を変えずに通った 3、人の決定 11、直して通った 35、no_paths 54、git_unavailable 1、通っていない 6。拒否 1 件あたりの「path を変えずに通った」の割合は B0 の窓 2.7%、後の窓 3.2%。

### 例

- 指摘の path を変えずに通った（reviewer）: task 01M6VTFG9K5HEE817EW05JASMZ の 2026-11-08T06:33Z の拒否（指摘の path `scripts/dev/repo-health`）が、その path を変えずに 09:19Z に通った。task 01M74TM8Y43C5EMYF7FP525DZ5 の 2026-11-11T11:11Z の拒否（`crates/task-core/src/delegate.rs`）が、その file を変えずに 11-12T06:40Z に通った。
- 例外（reviewer）: G4（01M6TYVJSXBZMSBC0YGAT2TTX0）の 2026-11-11T07:08Z と 08:50Z の拒否の後、`scripts/dev/repo-health/profiles/celeris.toml` を変えて 22:29Z に通った。
- 人の決定（health）: R2（01M6TYVJT0Z1CBGNK8VTNK8ZJH）の段の統合 `integrate-settings` の `size.responsibility-growth`（`crates/task-dispatch/src/dispatcher.rs` ほか）の fail が、2026-11-10T23:36Z の `plan_gate_decided` と 11-11T02:16Z の `answer` の後、11-11T03:51Z に通った。
- 人の決定（reviewer）: task 01M6Y8EG6QN8S421XYJNVDCPTA の 2026-11-08T21:29Z の拒否（`scripts/dev/repo-health/ci.sh` ほか）が、21:42Z の `decision_answered` の後、21:52Z に通った。
- policy.changed: G4 の段の統合 `integrate-design` の 2026-11-10T14:11Z と 14:19Z の fail（`policy.changed`）。同じ指摘の繰り返しで `HealthFindingsRepeated` が親に記録され、17:34Z に通った。

例の全体（分類ごとに 5 件まで）は JSON の `measures.c_false_positives.<窓>.examples` にある。

## (d) Celeris の end-to-end 開発の実績（健全性 v2 の子 task）

親 01M6TYV9DTZESAJJMXRQVGZ56V の直下の子 12 件。値は各子の部分木（子 task を含む）を終端まで数えた。計画の版数は `execution_plans`、WU は `work_units`、repair は `RepairScheduled`、replan は reason が `replan`・`plan_replan` の遷移、review_fail は reason が `review_fail` の遷移、設計 review の fail は `DesignReviewed.pass = false`。着手は最初の `→ running`、done は終端の状態が done のときの最後の遷移。delivery は task の最後の `integration_*` event。

| 子 | 部分木 | 計画の版 | WU | repair | replan | 自動 replan | review_fail | 設計 review fail | 着手→done（時間） | delivery | Claude USD |
|---|---|---|---|---|---|---|---|---|---|---|---|
| B0 | 3 | 5 | 23 | 0 | 1 | 1 | 0 | 0 | 6.36 | merged | 34.19 |
| G1 | 1 | 1 | 11 | 1 | 0 | 0 | 0 | 0 | 3.08 | merged | 24.25 |
| G2 | 14 | 13 | 70 | 3 | 1 | 1 | 4 | 0 | 26.85 | merged | 175.99 |
| G3 | 7 | 17 | 88 | 4 | 6 | 7 | 4 | 7 | 38.68 | merged | 301.48 |
| G4 | 5 | 14 | 46 | 4 | 8 | 9 | 3 | 0 | 37.02 | merged | 115.33 |
| R1 | 1 | 3 | 10 | 0 | 1 | 1 | 1 | 0 | 7.26 | merged | 32.07 |
| R2 | 3 | 6 | 20 | 2 | 4 | 4 | 0 | 3 | 15.06 | merged | 56.08 |
| R3 | 2 | 6 | 19 | 1 | 3 | 3 | 1 | 0 | 19.40 | merged | 64.21 |
| R4 | 8 | 22 | 95 | 4 | 10 | 9 | 4 | 5 | 97.38 | merged | 310.44 |
| R5 | 4 | 14 | 72 | 3 | 8 | 12 | 2 | 0 | 48.16 | merged | 167.09 |
| R6 | 3 | 4 | 21 | 0 | 3 | 4 | 0 | 1 | 14.56 | merged | 69.20 |
| V0 | 1 | 1 | 10 | 0 | 0 | 0 | 0 | 0 | running（未完） | — | 1.89 |
| 計 | | 106 | 485 | 22 | 45 | 51 | 19 | 16 | | | 1,352.22 |

- 11 件が done で、delivery はすべて merged。V0 は終端の時点で running。
- GPT 系の run は部分木の合計 50 本で、USD は 0 で記録されるので上の USD に入っていない（未計測）。

## 未計測の項目

- GPT 系（codex）の reviewer run・worker run の USD: `runs.usage_json` の `cost_usd` が 0 で記録される。tokens だけ数えた。
- final review の必須検査の health と、final review の材料（`HealthReviewInputBuilt`）の秒: event に所要時間の欄が無い。
- usage の無い reviewer run の費用（B0 の窓の Claude 166 本、後の窓の Claude 29 本）。
- 設計 review の拒否が「指摘の path を変えずに通った」か: 計画は commit でないので git で比べられない。
- reviewer の拒否のうち指摘の path が拾えない 30 件（後の窓）・54 件（B0 の窓）の誤検出の判定。
- 後の窓の 1 本あたりの review 費用の増加のうち、health の材料が占める分。

## 証拠コマンド（提出時の HEAD で実行）

- `python3 agent-docs/progress/2026-11-15-health-v2-v0/loop-cost.py --db 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro' --out agent-docs/progress/2026-11-15-health-v2-v0/loop-cost.json` → exit 0（約 10 秒）
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/loop-cost.py --db 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro' --verify agent-docs/progress/2026-11-15-health-v2-v0/loop-cost.json` → exit 0（`loop-cost verify: ok`）
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/loop-cost.py --db /local/celeris/data/db/celeris.sqlite3 --out …` → exit 1（`mode=ro` の無い指定を拒む）

## 未解決事項

- `--verify` は git の object を読む。拒否の commit が gc で消えると、その件の分類が `git_unavailable` に変わり verify が落ちる。

## 提案

- `IntegrationCheckFinished` と同じく、final review の必須検査の health の所要時間を `PhaseCheckResult` に残すと、final review での health の秒を測れる。
- reviewer の拒否の指摘に path の欄（review.json の findings の path）を event に残すと、no_paths の 30 件も判定できる。
