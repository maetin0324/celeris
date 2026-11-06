---
tasks: [01M6JW1EFPBCJX8J7D2ZACHS3D, 01M6JW1EFT0NX6HP89S55ECNXE, 01M6JW1EFW86JH3VKZ274J9GT5, 01M6JW1EFYFTSPMT080KHKW9EY, 01M6JW1EG41D198Y5CP65MQJM4, 01M6N7XY9E70AH4476VQTSFVJY, 01M6N7XYA31CMJV6RR1KXSQMKH, 01M6PAMC3S9QN740JWEQTXV60J, 01M6PAMC44JQ6DB90E9HQ076DW, 01M6QW0GSWRGP85RE263TWTM01]
---
# Project Controller の運用手順（人と operator）

案件を長期に自律運営する Project Controller（[ADR 2026-11-04-project-controller](../../agent-docs/adr/2026-11-04-project-controller.md)）を、
人が本番で有効にする・止める・戻すための手順。既定は全案件で無効で、この手順を人が実行するまで本番の動きは変わらない。
本番への導入の順と確認項目は末尾の「導入の checklist（P2-J）」にまとめる。

## 仕組みの要点
- 判定は TS（`control-ts/src/domains/project_controller.ts`）。状態・検証・上限・効果は Rust
  （`crates/task-dispatch/src/dispatcher/project_decider_ts.rs`）。LLM を呼ぶのは `replan` のときの既存の案件 CoS run だけ。
- 案件 mode（`off` → `dry_run` → `shadow` → `enabled`）と領域 mode（`rust` → `compare` → `canary` → `ts`）の 2 段。
  `dry_run`・`shadow` は領域 `compare` 以上、`enabled` は領域 `ts`（または `canary` で選ばれた案件）を要する。足りないときは
  判断の記録が `pc/script-unavailable` になり、起票も LLM も起こさない。
- 進捗の増えない `replan`・`launch_root` が 5 回続くと、Rust が `escalate` に置き換えて phase を `blocked` にする。
  `escalate` は notice（`bad_news`、source key `project_controller:escalate:<案件>:<question_key>`）を 1 件出し、
  同じ問いは 2 回目を出さない。止まった理由が解ける（問いの回答など）と自動で 1 回だけ再評価する（下の「長時間の運用」、P1-G）。
  解けない理由のときは人が mode を置き直すまで判断しない。
- 達成（phase `achieved`）は TS の申告（`gap`）では決まらない。Rust が Outcome ごとに、条件を満たす root（done・review 合格・
  取り込み済み）と到達の記録（Roadmap の `reached` か適用済みの `mark_milestone`）の両方を数え直し、欠けていれば
  `pc/achieved-unverified:<id>` の `wait` に置き換える（[ADR 2026-11-05-phase3-achievement-invariants](../../agent-docs/adr/2026-11-05-phase3-achievement-invariants.md)）。
- 案件 CoS は `/cos/operations` で次を独断でできない（422）: 依頼文の変更（`project_request_human_only`）、
  draft でない task の objective・acceptance の変更（`task_goal_human_only`）、`enabled` の案件の root の起票
  （`project_controller_owns_launch`）、controller が `off` でない案件での本番 release の promote（`production_requires_human`）。人の発言を
  `instructed_by` に引いたときだけ通る。secret・人だけの操作は registry の EXCLUDED が断る。

## 人の手順
| 場面 | 誰が | 何を | 確認 |
|---|---|---|---|
| 機能を本番に入れる | 人 | release を promote し、config に `[project_controller] enabled = true` と `allowed_project_ids = ["<案件 id>"]` を書いて daemon を再起動する | `GET /api/v1/daemon` が controller を示す。journal に `[project_controller] needs the CoS chat pipeline` が無い |
| 判定の領域を上げる | 人 | 同じ世代（`bundle_sha256`）で `rust` → `compare` → `canary` → `ts` の順。`celerisctl script generations register <sha256>` の後、`celerisctl script mode project_controller compare --expected-version <v> --generation <sha256> --mode-reason "…"`、`canary` は `--canary-bps <1..9999>` を足す。`canary`・`ts` は人の token だけ（他は 403 `script_mode_requires_human`）。`<v>` は `celerisctl script domains` の `version` | `celerisctl script domains` の `project_controller` の mode・世代・canary の比 |
| 効果なしで判定を見る（dry-run） | 人 | `PUT /api/v1/projects/{id}/controller` `{"mode":"dry_run","expected_revision":<rev>,"reason":"…"}`（daemon の `[project_controller] enabled = true` と領域 `compare` 以上が要る。`allowed_project_ids` は不要）。置いた直後の tick で 1 回判定する。もう一度見るときは `POST /api/v1/projects/{id}/controller/wake` `{"external_ref":"dry-run","idempotency_key":"<毎回違う値>"}` | `GET /api/v1/projects/{id}/controller/decisions` の判断（mode `dry_run`、効果 `suppressed`）の `proposal_json` に proposal・explanation・violations・分類。event（cluster job・browser の待ちの解除を含む）・cron では判定せず、起票・CoS run・timer は 0 |
| 案件を観察だけにする | 人 | `PUT /api/v1/projects/{id}/controller` `{"mode":"shadow","expected_revision":<rev>,"reason":"…"}` | `GET /api/v1/projects/{id}/controller/decisions` の判断が `suppressed`、CoS run 0 |
| 案件を有効にする | 人 | 同じ API で `mode: enabled` | root が label `project-controller` で起票される |
| 止める・戻す | 誰でも（人・operator・CoS） | 同じ API で `mode: off`。作成済みの root は残る（cancel は別の操作） | 以後 wake が増えない |
| `blocked` から再開する | 人 | notice の問いに答える（許可・却下）。問いの答えで解けない理由（下の「長時間の運用」）は、原因を確かめてから同じ API で mode を置き直す（`revision` が進む） | phase が `active` に戻る。自動の再開は判断の記録の `pc/resume/*` |
| 機能を外す | 人 | config の `[project_controller]` を消して再起動。release を戻すときは先に欄を消す（`Config` は `deny_unknown_fields`） | `GET /api/v1/daemon` |

本番の DB・config・systemd の変更は人が行う。worker の run・試験は本番の DB・daemon に触れない
（`scripts/dev/test-project-cos-shadow.sh` は一時 DB と `$TMPDIR` だけを使う）。

## Roadmap（付記 A5）
案件の途中目標は、人が承認した Roadmap の版があればそれが正本になる（無ければ依頼文の ```` ```celeris-outcomes ```` block）。
版を進めるのは人の決定だけで、CoS は提案まで。brief（状態の要約）の書き込みは Roadmap を変えない。

| 場面 | 誰が | 何を | 確認 |
|---|---|---|---|
| 途中目標の版を提案する | 人か CoS | `POST /api/v1/projects/{id}/roadmap/proposals` `{"base_version":<今の版か null>,"change_reason":"…","outcomes":[{"id":"o1","title":"…","acceptance":["…"],"priority":1,"depends_on":[]}]}`。CoS は `project.roadmap_propose` | 201 の `status` が `proposed`。`GET …/roadmap` の版は変わらない |
| 提案を承認・却下する | **人だけ** | `POST /api/v1/projects/{id}/roadmap/proposals/{version}/decide` `{"decision":"approve","note":"…"}`（`reject` も同じ）。人の token で | `GET …/roadmap` の `version` が進み、`GET …/roadmap/history` で前の版が `superseded` |
| Outcome を取り下げる | 人の承認 | `dropped: true` を付けた版を提案して承認する（id は消せない） | `GET …/roadmap` の `status` が `dropped` |
| Outcome の到達を記録する | 人 | `POST /api/v1/projects/{id}/roadmap/outcomes/{oid}/status` `{"status":"reached","evidence_refs":["task:<root id>"]}`。root が done・合格・取り込み済みでなければ 422 | `GET …/roadmap` の `status`・`evidence`・`root_task_ids` |
| 本番の DB を新しい release で開く | 人 | promote で migration 0113 が表を 2 つ足す（既存の行は変えない）。戻し方は末尾の「schema migration の戻し方」 | schema version（下の節の確認） |

## 許可の範囲（authority ledger。A7）
controller が `off` でない案件では、CoS と controller が人に聞かずに行える範囲を、人が作る許可（期限・回数・元の根拠つき）で決める
（[ADR 付記 A7](../../agent-docs/adr/2026-11-04-project-controller.md)）。許可が無い・切れた・根拠が読めない・前の操作の結果が分からない
ときは、どれも行わない。

| 場面 | 誰が | 何を | 確認 |
|---|---|---|---|
| controller を有効にする | 人 | `PUT …/controller` `{"mode":"enabled",…}`。同じ操作で続きの root の起票の許可が 1 件できる（既定 30 日・10 回。`launch_grant_expires_at`・`launch_grant_budget` で変えられる） | `GET /api/v1/projects/{id}/authority` の `grants` に `launch_root`・`state: live` |
| CoS の提案を許可する | 人 | 案件の画面の「許可」（API は `POST …/authority/grants` に提案の `proposal_id`・期限・回数）。根拠は CoS が引いた人の発言 | 提案が `accepted`、許可が `live` |
| 提案を断る | 人 | `POST …/authority/proposals/{p}/dismiss` | 以後 CoS が同じ問いを出すと 409 |
| 許可を取り消す | 誰でも | `POST …/authority/grants/{g}/revoke` | 許可の `state` が `grant_revoked` |
| 結果が分からない操作を確かめる | 人 | 外部の結果を見てから `POST …/authority/audit/{a}/outcome` `{"state":"applied"\|"failed","note":"…"}` | 監査の行の `outcome_state`。`failed` は回数が戻る。確かめるまで同じ種類の操作は止まる |
| 許可が足りずに止まった（notice「起票する許可がありません」） | 人 | 許可を作る。作ると controller は `active` に戻り次の評価で起票する | 判断の記録が `pc/authority-required` の後に起票 |

許可は人の token だけが作れる（operator・MCP・CoS は 403 か 422）。`human_required` は許可にできない。

## 長時間の運用（予算・guard・phase。A6）
controller は event（root の終了・失敗・人の操作など）で起き、cron の `project_reconcile` は event を取りこぼしたときの照合として使う。
判定（TS）の前に Rust の guard が掛かり、当たると判定を呼ばずに人へ 1 回だけ知らせて止まる（受信箱の通知 `bad_news`、phase `blocked`）。

| guard | 止まる条件（設定 `[project_controller.budget]`、既定） | 人の通知の key |
|---|---|---|
| 同じ失敗 | root が同じ理由で `max_same_failure_repeats`（3）回失敗した（最後の人の操作の後で数える） | `same_failure:<署名>` |
| TS の案の検証落ち | 判定の案が `max_invalid_decisions`（3）回続けて検証に落ちた | `invalid_proposals:<判断 id>` |
| 1 日の予算（UTC） | CoS 入力 `max_cos_inputs_per_day`（10）、起票 `max_roots_per_day`（5）、換算費用 `max_usd_per_day`（100.0。USD 相当） | `budget:<種類>:<日>` |
| TTL | phase `waiting` で外からの応答（cron 以外の wake）が `ttl_secs`（259200 秒＝3 日）来ない | `ttl:<最後の入力の時刻>` |
| 停滞 | 進捗なしの効果つきの判断が `max_no_progress_decisions`（5）回続いた | `no_progress:<epoch>` |

- 案件を一時停止・中止すると controller も評価を止める（phase `paused`・`cancelled`。止まっている間の wake は捨てる）。再開すると 1 回だけ評価する。
- 全途中目標が達成済み（phase `achieved`）の案件は、root の終了や cron では評価しない。人が案件を変えたときだけ評価する。
- 止まった理由が解けると controller は自動で 1 回だけ再評価する（P1-G）。根拠は判断の記録の `pc/resume/<原因>` の行に残る。
  - 人への問い: 問いの key で許可するか、提案を断る（`…/dismiss`）。依頼文の誤りの問いは依頼文を直しても解ける。
  - 同じ失敗: 人が案件を操作するか、失敗した root 自身が完了する。解けた後にまた同じ失敗が上限まで続くと、新しい通知を 1 回出して止まる。
  - TTL: 外からの応答（cron・CoS の要約の更新は数えない）。
  - 1 日の予算: 次の UTC 00:00 を期限にし（`next_wake_at`）、その日の使用量が危険な超過でなく、直近 7 日の自動の再開が 3 回未満で、起票の予算なら有効な起票の許可があるときだけ再評価する。危険な超過（換算費用が上限の 2 倍以上、または費用不明の run があるなど）・回数切れ・許可なしでは、通知（`budget_hold`）を 1 回出して止まったままにする。
  - TS の案の検証落ち・判定や効果の失敗・案件の一時停止・中止は自動で解けない。人が原因を確かめてから案件の画面で controller の mode を置き直す（`PUT /api/v1/projects/{id}/controller`）。
- 人の回答を待つだけの案件は、cron の照合で毎時は評価しない。人の回答は wake で届き、取りこぼしの照合は 1 日に 1 回（`next_wake_at`）。
- 日次の費用は案件の task run と案件 CoS の chat run の使用量を、静的単価表（`task_core::pricing`）で USD 相当に換算して合計する（cache read は cache read の単価、cache creation は単価表の行なら 1 時間 TTL の書き込み単価（input の 2 倍）と表の cache write の単価の大きい方で数える。Claude Code は 1 時間 TTL で書き、`Usage` は TTL を分けないため。ADR `2026-11-06-phase5-n0-usd-accounting-contract` D3）。予算判定は保存済みの `usage.cost_usd` を使わず判定のたびに再計算するので、実験の道具（`compare.py`・pilot の集計）が足す保存値とは一致しない。単価表に無い model（local の llama.cpp・opencode の Qwen など）と使用量の記録が無い終わった run は「費用不明」で、1 件でもあれば上限に達したものとして止める（0 USD としては数えない）。local model を数えるには config に `[budget_rates."<model の接頭辞>"]` を書く（`input_per_million`・`output_per_million`・`cache_read_per_million`・`cache_write_per_million` の 4 欄すべて必須。単価表で 4 種とも単価がある model の接頭辞は読み込みエラー）。木の上限は `[execution.tree] max_tree_usd`（任意）。判断は ADR `2026-11-06-budget-usd-caps`。案件 CoS の起動数は `BudgetUsage.chat_starts` で数える。controller の CoS 入力数と chat 起動数のいずれかが `max_cos_inputs_per_day` に達したら止まる。今日の使用量は `GET /api/v1/projects/{id}/controller` の `budget_today` と案件の画面の「今後の作業」に出る。
- brief の内容変更は保存と同じ transaction で wake を書き、変更前の判断を入力 epoch で無効にする。同じ内容の再送では wake を増やさない。brief の更新だけで blocked・paused・cancelled は解除しない。
- 値を変えるときは本番 config の `[project_controller.budget]` を人が書き換え、daemon を再起動する（この依頼では実行しない）。0 は設定の誤りで、
  daemon は起動しない。
- 測った値（`agent-docs/progress/2026-11-04-long-horizon/a6/resource.jsonl`）: 本物の TS の判定 1 回の p50 は約 1.5 ms、100 案件を 1 周する時間は約
  0.19 秒で、isolate は 1 個のまま増えない。複数日の実稼働の確認は A9 の pilot で行う。

## 画面と非本番の pilot（A9）
- 案件の詳細画面の「自動運営」に、長期方針（CoS の要約の目的と人の依頼文）、途中目標の到達（P2-J）、関連する root task、
  今後の作業（要約の next_actions、次の再評価の時刻、今日の使用量と上限、guard の上限）、待機と人の要件（待機の理由・
  承認の範囲・要約の waiting_on・受信箱の判断待ち・許可の提案）、過去の判断と根拠（最後の判断の追跡、判断の規則・効果・参照）、
  Roadmap（A5）と許可（A7）が出る。何を読むかは下の「GET の要約と画面（P2-J）」。mode は同じ画面の「観察だけにする」「有効にする」「止める（従来の
  CoS に戻す）」で切り替える（理由が必須。上げるのは人の token だけ）。`dry_run` は API だけ。
- `POST /projects` で作った直後の案件は `proposed` で、これを `active` へ移す API は無い。人が controller を `enabled` に上げると、同じ
  transaction で案件を `active` にする（`proposed` のままだと判定と検証が起票を断り続ける）。
- repo を持つ案件の root は、最後の取り込み（`IntegrationMerged`）の後に `IntegrationReopened` が無いときだけ取り込み済みに数える。
  `delivery_skipped` の `not_git`・`no_commits` と `IntegrationNotApplicable` は取り込み済みに数えない（`not_git` は受信箱に理由付きで
  出る）。取り込みの event で controller を起こすのは `IntegrationMerged` だけ（root の done でも別に起こす。repo の無い案件の root も
  同じ）。repo を持たない案件の root は、done だけでは取り込み済みに数えない。成果物（`result.json`・`checkpoint.json`・
  `delegate.json`・`followups.json` を除く `ArtifactProduced`、または合格した `artifact_exists`・`knowledge_page` の条件）が要る
  （[A9 pilot の不具合の付記](../../agent-docs/adr/2026-11-05-a9-llm-pilot-findings.md) D1、
  [達成の不変条件](../../agent-docs/adr/2026-11-05-phase3-achievement-invariants.md) I4）。
- root の review 合格は、最後の終端遷移が `review_pass` の done で、最後の review の判定に不合格が無く、`Check::Command` の条件が
  全部合格のときだけ数える（I3。reopen で前の合格を捨てる）。Outcome の block が `checks` を書くと、Rust が root の acceptance に
  command の条件として足す（[P0-E の ADR](../../agent-docs/adr/2026-11-05-phase3-p0e.md)）。
- Roadmap の acceptance・checks を改版すると、旧条件の root は到達の根拠から外れる。人・CoS の root も起票時の承認版と実際の command を照合する。人・CoS の旧 root で起票時の版の記録が無いものは根拠にならず、新しい root の実行が必要になる。題名だけの改版では根拠は失効しない。再達成のときは新しい root を evidence に持つ reached を追記し、古い reached は履歴に残す（[N5 ADR](../../agent-docs/adr/2026-11-06-n5-evidence-integrity.md)）。
- 実 LLM の pilot は `scripts/dev/long-horizon-pilot-llm.sh` を人が sandbox の外から実行する。人の承認の印
  `LONG_HORIZON_PILOT_LLM_APPROVED=1` と、人が書いた LLM の設定（`--llm-config <toml>`。`[llm_sources]`・`[harnesses]`・
  `[route_defaults]`・`[harness_selection]`・`[accounts]`）が要る。Celeris の worker の run の中では起動しない。一時 home・一時 DB・
  loopback の daemon で動き、本番に触れない。raw は `agent-docs/progress/2026-11-04-long-horizon/a9/` の下の `llm` に書く。
  24〜72 時間の実稼働は人の別途の承認が要る。手順と未実施の記録は `agent-docs/progress/2026-11-04-long-horizon/a9.md`。
- pilot の C06（公開と承認）では、driver が worker の外で公開の broker（`scripts/dev/long-horizon-pilot/pilot_effect.py`）を
  起こし、worker には公開の command（`<試行の home>/bin/pilot-effect publish <logical_id>`）だけを案件の依頼文で渡す。承認は
  daemon の承認（`GET /approvals`・`POST /approvals/{id}/decide`）で、人の token の決定だけを数える。公開の記録は HMAC の鎖で、
  worker が書き換えると試行は observation_failed で落ちる。API の token は試行ごとの乱数で、試行の home の外に daemon と broker の
  起動の間だけ置く（worker は人の token を読めない）。daemon に driver の決めていない承認の決定があれば、試行は observation_failed
  で落ちる。外部への公開はしない
  （[P0-D の ADR](../../agent-docs/adr/2026-11-05-phase3-p0d.md)）。



## GET の要約と画面（P2-J）
`GET /api/v1/projects/{id}/controller` の `summary` と、案件の画面の同じ欄で次を読む（[ADR](../../agent-docs/adr/2026-11-05-phase3-p2j.md)。
組み立ては `crates/task-ops/src/project_controller_view.rs`）。要約は画面の写しで、判定はこれを読まない（判定は毎回 snapshot から）。

| 知りたいこと | `summary` の欄 | 画面（`data-testid`） | 読み方 |
|---|---|---|---|
| 未達・達成 | `outcomes[]`（`reached`・`recorded`・`evidence_root_ids`）、`outcomes_invalid` | 途中目標の到達（`controller-outcomes`） | `reached` は根拠の root と到達の記録の両方があるときだけ真。判定の I1 と同じ規則。`shadow`・`dry_run` では `suppressed` の `mark_milestone` も記録に数える（enabled なら何が起きるかの写し） |
| 待機の理由 | `waiting`（`kind`・`rule_id`・`details`） | 待機の理由（`controller-wait-reason`） | phase が `waiting`・`blocked`・`paused`・`cancelled` のときだけ出る。`kind` は `guard`・`authority`・`achievement_unverified`・`script_unavailable`・`invalid_proposal`・`escalation`・`wait`・`paused`・`cancelled`・`other` |
| 承認の範囲 | `authority`（`launch_root_allowed`・`launch_root_reason`・`grants[]`） | 承認の範囲（`controller-authority-scope`） | 生きている許可だけ（期限内・回数が残る・取り消されていない）。回数は `budget_remaining` |
| 次の wake | `next_wake`（`at`・`backoff_until`・`pending_kinds`） | 次の再評価（`controller-next-wake`） | `at` が無ければ event が来るまで評価しない |
| guard・予算 | `budget_limits` と `budget_today` | 今日の使用量 / 上限（`controller-budget-today`）、guard の上限（`controller-guard-limits`） | 上限は daemon の起動時の `[project_controller.budget]`。値を変えたら再起動まで古い値が出る |
| 追跡 | `trace`（`decision_id`・`rule_id`・`effect_state`・`based_on_revision`・`snapshot_sha256`） | 最後の判断（`controller-trace`） | 詳細は `GET …/controller/decisions` の同じ id の `proposal_json` |
| 記録の行数（P4-D） | `retention`（`wakes_total`・`wakes_pending`・`decisions_total`・`decisions_effect_none`・`decisions_pending`） | 記録の行数（`controller-retention`） | GC が有効なら `wakes_total`・`decisions_total` は保持の窓の分で頭打ちになる。`wakes_pending`・`decisions_pending` は GC で減らない |

試験: `crates/task-api/tests/project_controller.rs` の `project_wake_api_summary_shows_outcomes_wait_authority_and_next_wake`、
`crates/task-dispatch/src/dispatcher/tests/project_achievement.rs` の `project_controller_summary_*` 2 件、
`web/features/projects/project-controller-panel.test.tsx` の `P2-J:` 2 件。

## 記録の保持と GC（P4-D）
`project_wakes`・`project_controller_decisions`・`script_compare_log` は毎時の cron の照合で増える
（[ADR](../../agent-docs/adr/2026-11-05-phase4-p4d-retention.md)、migration 0117、実装は `crates/task-core/src/store/project_retention.rs`）。
`[project_controller.retention]` を `enabled = true` にすると、dispatcher が `interval_secs`（既定 3600 秒）ごとに 1 回、
条件を満たす行を archive（JSONL）に書いてから消す。既定は無効で、人が config を書くまで本番の動きは変わらない。events は消さない。

消す行と残す行（DB の trigger が判断と compare の記録について同じ条件で二重に守る。許可の行の無い `DELETE` は拒まれる）:

| 表 | 消す行 | 必ず残す行 |
|---|---|---|
| `project_wakes` | 消費済みの `cron` の wake で `wake_keep_secs`（既定 7 日）より古いもの | 未消費の wake、`cron` 以外の wake（同じ失敗の数え・人の変更・TTL が読む） |
| `project_controller_decisions` | 効果 `none` で `decision_keep_secs`（既定 30 日）より古く、案件ごとの新しい `decision_keep_latest`（既定 256）件の外にあるもの | pending・applied・suppressed・failed の判断、案件の最後の判断（`last_decision_id`）、案件の最初の判断、`pc/resume/%`・`pc/authority-required` の判断 |
| `script_compare_log` | TS が適用していない行（`applied_by` ≠ `ts`）で `compare_keep_secs`（既定 30 日）より古いもの | TS が適用した行（見回りの違反の根拠） |

どの表も、表の最大 id の行は保持の期間より古くても消さない。`script_compare_log` の id は AUTOINCREMENT の無い
`INTEGER PRIMARY KEY` なので、最大 id の行を消すと次の行が同じ id を使い直し、script guard の見回り（読み終えた id より後だけを
読む）が新しい違反の行を見落とす。migration 0118 の trigger は permit があっても最大 id の DELETE を拒む。最大 id の行を残すと、次の id は GC の前の最大 id より必ず大きい（ADR D9）。

config の例（`archive_dir` は絶対 path で必須。無い・相対なら起動時の設定エラー）:

```toml
[project_controller.retention]
enabled = true
archive_dir = "/local/celeris/state/retention-archive"
```

回収の条件と確かめ方:
- 1 回の GC は表ごとに最大 `max_rows_per_pass`（既定 5,000）行、1 transaction に `batch_rows`（既定 2,000）行。初回に古い行が多いと、
  数時間かけて少しずつ消える（100 万行の計測で 1 回の中央値 0.5〜1.1 秒・最大 1.2〜5.6 秒、合計 42〜97 秒。release build）。
- 記録は `project_retention_runs`（追記専用。`archived`・`deleted`・`cutoff`・`archive_path`・`archive_sha256`）。archive の file は
  `sha256sum` で `archive_sha256` と照合できる。`deleted` が `archived` より少ないのは、並行の GC が先に同じ行を消したとき。
- 消した行の page は空き page になり、DB の file は縮まない（次の書き込みが使う）。file を縮めるのは人が daemon を止めて
  `celerisctl db backup` の後に `VACUUM` する操作で、この GC はしない（100 万行の計測で 200 MB → `VACUUM` 後 77 MB、約 1 秒）。
- archive の file は GC が消さない。置き場の容量は人が見る（100 万行の計測で約 270 MB の JSONL）。
- 止める: `enabled = false` にして再起動する。消した行は archive から読める（戻す道具は無い。判定は残した行だけで同じになることを
  `project_retention_soak_gc_bounds_rows_without_changing_decisions` が確かめる）。

## 導入の checklist（P2-J）
本番への導入は人が行う。worker・operator は下の手順を実行しない（promote・`systemctl`・本番 config・本番 DB・人だけの API の承認）。
各段で前の段の確認がそろってから次へ進む。戻すときは「rollback」の表を上から使う。

### 1. release gate（main で全体）
- [ ] 導入する commit が `main` にあることを確かめる（`git merge-base --is-ancestor <sha> refs/heads/main` が 0）。
- [ ] `scripts/selfdeploy/release.sh <sha>` を人が回し、gate の全段が通る（`cargo-fmt-check`・`control-ts-install`・`control-ts-bundle`・
  `script-bundle-verify`・`cargo-test`（全体の試験。日頃の `test-affected.sh` では回さない）・`script-bundle-tests`・`cargo-clippy`・
  `cargo-build`・`web-pnpm-typecheck`・`web-pnpm-test`・`web-pnpm-release`）。確認は `<release>/gate.json` の `failed_step` が無いこと。
- [ ] `scripts/selfdeploy/verify.sh <sha12>` が通る（staging の煙試験と件数一致）。
- [ ] release の `notes.md` と `celerisctl release preview <sha12>` で、migration の有無と変わる config 欄を読む。

### 2. schema migration の戻し方
- controller が足した migration は 0111（script の領域）・0112（controller の表）・0113（Roadmap）・0114（authority ledger）で、
  どれも表を足すだけで既存の行を変えない。P2-J は migration を足さない。その後 P4-B が 0115（wake `evidence_changed`）、P4-D が 0117（記録の保持）を足し、N5 は 0118（起票時の承認版と最大 ID を保護する trigger）を足し、SCHEMA_VERSION は 118（0116 は予約のまま未使用）。
- 古い binary は新しい schema の DB を開けない（`StoreError::SchemaTooNew`。何も書かずに起動を止める）。down migration は無い。
- [ ] promote の前に、`promote.sh` が `sqlite3 .backup` で昇格前の写しを `$CELERIS_BACKUPS_DIR` に作ったことをログの
  `backup ok:` で確かめる。
- 戻すとき（人）: `scripts/selfdeploy/rollback.sh --restore-db` で昇格前の写しに戻す（写しの後に書かれた行は失われる。
  controller の判断・wake・Roadmap・許可の行も含む）。写しに戻さずに古い release だけ戻すことはできない（`SchemaTooNew`）。
  データを残したいときは、DB を戻さず新しい release のまま controller を `off` にする（下の rollback 1〜2）。
- 消した config 欄は読み込みエラーになる（`deny_unknown_fields`）。古い release に戻すときは、古い release に無い欄
  （`[project_controller]` とその下）を先に消す。

### 3. CoS の KB skill の版
- [ ] repo の `config/skills/cos-operator`（`metadata.version` 7、`project-controller.md` を含む）と `config/skills/cos-inbox-triage`
  （policy_version 11）を、本番の KB に人が取り込む: `celerisctl skills import config/skills --name cos-operator --name cos-inbox-triage --root <kb_root>`。
- [ ] 本番 config の `[cos.triage] policy_version` が skill の版（11）と同じ。違うと CoS の resolve が 422 `cos_policy_version_mismatch`。
- [ ] 確認: `test -s <kb_root>/skills/cos-operator/project-controller.md`。web の案件スレッドで一往復し、`CoS skills unavailable` が出ない。
- P2-J は skill を変えない（版は上のまま）。

### 4. 本番の限定 rollout（shadow → canary → enabled）
| 段 | 誰が | 何を | 進む条件 |
|---|---|---|---|
| 0. 入れる | 人 | release を promote し、本番 config に `[project_controller] enabled = true`・`allowed_project_ids = ["<案件 1 件>"]` を書いて daemon を再起動 | `GET /api/v1/daemon` が controller を示す。journal に `[project_controller] needs the CoS chat pipeline` が無い。全案件の `GET …/controller` が mode `off` |
| 1. 試しの評価 | 人 | 領域を `compare` にし（同じ世代）、対象案件を `dry_run` | `…/controller/decisions` の効果が全部 `suppressed`、起票・CoS run 0。`summary.outcomes` が人の見立てと合う |
| 2. 観察（shadow） | 人 | 対象案件を `shadow` にし、24 時間以上置く | 判断が `suppressed` のまま、CoS run 0。`summary.waiting` の規則に `pc/invalid`・`pc/script-unavailable` が続かない。daemon の RSS と dispatch の遅れが導入前と同じ程度（P1-I の観測と同じ式で比べる） |
| 3. canary | 人 | 領域を `canary --canary-bps <小さい値>`。`enabled` にした案件のうち選ばれたものだけが TS で動く | 起票された root が label `project-controller`、`summary.authority.grants` の残り回数が起票の数だけ減る。guard の通知が想定外に出ない |
| 4. enabled | 人 | 領域を `ts`、対象案件を `enabled`（画面の「有効にする」）。対象を広げるのは `allowed_project_ids` を人が足して再起動 | 同じ確認を案件ごとに 72 時間。`summary.outcomes` の達成が reviewer の合格・取り込みと食い違わない |

### 5. rollback（上から順に、効くところで止める）
| 順 | 誰が | 何を | 確認 |
|---|---|---|---|
| 1 | 誰でも（人・operator・CoS） | 対象案件を `PUT …/controller` `{"mode":"off",…}`（画面の「止める」） | 以後 wake が増えない。作成済みの root は残る（中止は別の操作） |
| 2 | 誰でも | 領域を戻す: `celerisctl script mode project_controller compare`（または `rust`）。guard が止めているときは `rust` だけ通る | `celerisctl script domains` の mode |
| 3 | 人 | 本番 config の `[project_controller]` を消して再起動 | `GET /api/v1/daemon` に controller が無い |
| 4 | 人 | release を戻す: 先に 3 を済ませ、`rollback.sh`。schema が進んでいれば `--restore-db`（上の 2） | `celerisctl` の schema version と稼働版 |

### 4a. 1 案件の限定導入の道具（P4-G）
上の段 0〜4 の「進む条件」を `scripts/dev/pc-rollout/pc_rollout.py` で判定し、記録する
（[ADR](../../agent-docs/adr/2026-11-06-phase4-p4g-shadow-rollout.md)）。`preflight`・`snapshot`・`monitor` は GET と file の
読み取りだけで、何も変えない。`apply`・`undo` は API で mode を変える（本番では人が実行する）。

承認は 2 種類の file に分ける。観察（`dry_run`・`shadow`）の承認では canary・enabled に上げられない。

| 種類 | `kind` | `stages` | ほかに要る欄 |
|---|---|---|---|
| 観察の開始 | `observe` | `["dry_run","shadow"]` の一部 | 共通の欄だけ |
| 効果の開始 | `effects` | `["canary"]` か `["enabled"]` | `shadow_report_sha256`（本番の 24 時間以上・違反 0 の shadow の監視の報告の sha256）、canary は `canary_bps` |

共通の欄: `approved_by`（`human:<名前>`）・`scope`（`production`）・`project_id`・`api`（`--api` と同じ文字列）・
`generation`（認めた bundle の sha256、64 桁）・`expires_at`（ISO 8601）。

```json
{"approved_by": "human:<名前>", "kind": "observe", "stages": ["dry_run", "shadow"], "scope": "production",
 "project_id": "<案件 id>", "api": "http://127.0.0.1:7721", "generation": "<bundle_sha256>",
 "expires_at": "2026-11-08T00:00:00Z"}
```

人の手順（本番。`T=<人の token の file>`、`P=<案件 id>`、`A=http://127.0.0.1:7721`、`O=<記録の dir>`）:

| 段 | 何を | go の条件 |
|---|---|---|
| 0 | config に `[project_controller] enabled = true`・`allowed_project_ids = ["$P"]` を書いて再起動。**再起動の後に** `celerisctl script generations register <sha256>`（再起動は、どの領域も指していない `loaded` の generation を retire する） | `python3 -I scripts/dev/pc-rollout/pc_rollout.py preflight --api $A --token-file $T --project $P --stage dry_run --config ~/.config/celeris/config.toml --approval observe.json --pilot-facts <pilot の facts.json> --out $O/preflight-dry_run.json` が `verdict: go` |
| 1 | `pc_rollout.py apply --stage dry_run --approval observe.json --reason "…"`（領域を `compare`、案件を `dry_run`）。`POST …/controller/wake` を 1 回 | 判断が 1 件以上、効果は `suppressed` か `none` |
| 2 | `preflight --stage shadow …` が go なら `apply --stage shadow …`。`monitor --stage shadow --secs 86400 --interval 60 --samples $O/shadow.jsonl --approval observe.json --undo-on-violation --out $O/shadow-report.json` | 報告の `violations` が空・`observed_secs` ≥ 86400 |
| 3 | 効果の承認（`effects`、`shadow_report_sha256 = sha256sum $O/shadow-report.json`）を人が書く。`preflight --stage canary --shadow-report $O/shadow-report.json …` が go なら `apply --stage canary …` | 段 3 の表の条件 |
| 戻す | `pc_rollout.py undo --api $A --token-file $T --project $P --reason "…" --domain-to compare`（`off` への変更は誰でも行える）。`--domain-to rust` で領域も戻す | 出力の `ok: true`、以後 判断が増えない |

監視の違反（`diff`）: 効果を伴う判断（`pending`・`applied`・`failed`）の増加、controller の root（label `project-controller`）の
増加、controller の CoS 入力の増加、案件 mode・領域 mode・generation・他の案件の mode・daemon の config の sha256 の変化、
待機の規則の `pc/invalid`・`pc/script-unavailable`、API の失敗。daemon の再起動は違反でなく `daemon_restarted` に記録する。
標本には RSS（`/proc/<pid>/status` の VmRSS・RssAnon。daemon と同じ host で実行したときだけ）と `GET …/controller` の時間が入る。

go/no-go の条件（G1〜G7）の定義は ADR の D2。G7 は実 LLM の縮小 pilot の再実行の facts（`--pilot-facts`）で案件 CoS の
run が数えられ `cos_usage_null` が 0 であること（01M6PYNMP4 の修正の確認）で、facts が無ければ `pending`（no-go）になる。

非本番の予行: `bash scripts/dev/pc-rollout.sh --out <絶対 path>`（一時 home・fake harness・127.0.0.1、約 2 分）。raw は
`agent-docs/progress/2026-11-05-phase4/p4g/e2e/`。予行の監視の報告は `scope: non-production` で、段 3 の G5 では fail になる。

### 6. まだ済んでいないこと（2026-11-05 時点）
- 第5フェーズ（2026-11-06〜）の状態表と N1〜N7 の手順・合否の数値は `agent-docs/progress/2026-11-06-phase5/n0.md` と同じ dir の `runbook.md`。下の項目はそこで更新する。
- [ ] P0-F（実 LLM の縮小 pilot の再試験）: 未実施。人の許可待ち（account・model・token の上限・時間の上限）。記録は
  `agent-docs/progress/2026-11-05-phase3/` の P0-F の進捗（main への取り込み待ち）。許可が出たら外部 operator が sandbox の外で
  `scripts/dev/long-horizon-pilot-llm.sh` を回す。
- [ ] 24〜72 時間の非本番の実稼働と、本番 RSS の controller `off` との比較（P1-I の段 3・4）: 未実施。人の承認待ち。
- [ ] 上の 1〜5 の本番の操作: どれも未実施（人が行う）。
- [ ] `[project_controller.retention]` の本番での有効化（P4-D）: 未実施（人が config を書き、`archive_dir` の容量を決める）。
- [ ] 本番 1 案件の shadow（P4-G）: 未実施。2026-11-06 の判定は `no-go (pending)`（人の observe の承認が無い、G7 の pilot の
  再実行が未実施、本番の G2〜G5 は人の preflight で未確認）。
