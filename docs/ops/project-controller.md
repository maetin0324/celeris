---
tasks: [01M6JW1EFPBCJX8J7D2ZACHS3D, 01M6JW1EFT0NX6HP89S55ECNXE]
---
# Project Controller の運用手順（人と operator）

案件を長期に自律運営する Project Controller（[ADR 2026-11-04-project-controller](../../agent-docs/adr/2026-11-04-project-controller.md)）を、
人が本番で有効にする・止める・戻すための手順。既定は全案件で無効で、この手順を人が実行するまで本番の動きは変わらない。
A5・A6・A7 は節を足す。

## 仕組みの要点
- 判定は TS（`control-ts/src/domains/project_controller.ts`）。状態・検証・上限・効果は Rust
  （`crates/task-dispatch/src/dispatcher/project_decider_ts.rs`）。LLM を呼ぶのは `replan` のときの既存の案件 CoS run だけ。
- 案件 mode（`off` → `dry_run` → `shadow` → `enabled`）と領域 mode（`rust` → `compare` → `canary` → `ts`）の 2 段。
  `dry_run`・`shadow` は領域 `compare` 以上、`enabled` は領域 `ts`（または `canary` で選ばれた案件）を要する。足りないときは
  判断の記録が `pc/script-unavailable` になり、起票も LLM も起こさない。
- 進捗の増えない `replan`・`launch_root` が 5 回続くと、Rust が `escalate` に置き換えて phase を `blocked` にする。
  `escalate` は notice（`bad_news`、source key `project_controller:escalate:<案件>:<question_key>`）を 1 件出し、
  同じ問いは 2 回目を出さない。人が mode を置き直すまで判断しない。
- 案件 CoS は `/cos/operations` で次を独断でできない（422）: 依頼文の変更（`project_request_human_only`）、
  draft でない task の objective・acceptance の変更（`task_goal_human_only`）、`enabled` の案件の root の起票
  （`project_controller_owns_launch`）、controller が `off` でない案件での本番 release の promote（`production_requires_human`）。人の発言を
  `instructed_by` に引いたときだけ通る。secret・人だけの操作は registry の EXCLUDED が断る。

## 人の手順
| 場面 | 誰が | 何を | 確認 |
|---|---|---|---|
| 機能を本番に入れる | 人 | release を promote し、config に `[project_controller] enabled = true` と `allowed_project_ids = ["<案件 id>"]` を書いて daemon を再起動する | `GET /api/v1/daemon` が controller を示す。journal に `[project_controller] needs the CoS chat pipeline` が無い |
| 判定の領域を上げる | 人 | `celerisctl script mode project_controller compare`（観察）→ `ts`（有効）。人の token で | `celerisctl script domains` の `project_controller` の mode |
| 効果なしで判定を見る（dry-run） | 人 | `PUT /api/v1/projects/{id}/controller` `{"mode":"dry_run","expected_revision":<rev>,"reason":"…"}`（daemon の `[project_controller] enabled = true` と領域 `compare` 以上が要る。`allowed_project_ids` は不要）。置いた直後の tick で 1 回判定する。もう一度見るときは `POST /api/v1/projects/{id}/controller/wake` `{"external_ref":"dry-run","idempotency_key":"<毎回違う値>"}` | `GET /api/v1/projects/{id}/controller/decisions` の判断（mode `dry_run`、効果 `suppressed`）の `proposal_json` に proposal・explanation・violations・分類。event（cluster job・browser の待ちの解除を含む）・cron では判定せず、起票・CoS run・timer は 0 |
| 案件を観察だけにする | 人 | `PUT /api/v1/projects/{id}/controller` `{"mode":"shadow","expected_revision":<rev>,"reason":"…"}` | `GET /api/v1/projects/{id}/controller/decisions` の判断が `suppressed`、CoS run 0 |
| 案件を有効にする | 人 | 同じ API で `mode: enabled` | root が label `project-controller` で起票される |
| 止める・戻す | 誰でも（人・operator・CoS） | 同じ API で `mode: off`。作成済みの root は残る（cancel は別の操作） | 以後 wake が増えない |
| `blocked` から再開する | 人 | notice の問いに答えてから、同じ API で mode を置き直す（`revision` が進む） | phase が `active` に戻る |
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
| 本番の DB を新しい release で開く | 人 | promote で migration 0113 が表を 2 つ足す（既存の行は変えない）。release を戻すときは SCHEMA_VERSION 113 の DB を 112 の binary が開けない（`SchemaTooNew`）ので、Roadmap を使う前の DB の写しから戻す | `celerisctl` の schema version が 113 |
