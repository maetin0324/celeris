---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
title: selector: 根拠表・route_defaults・段階・legacy 撤去の全体検証
status: done
updated: 2026-10-13
---
# selector: source・model 選択器（根拠表＋route_defaults・lane ごとの人の既定・shadow→enforce・legacy の撤去）

ADR [2026-10-11-llm-source-model-routing-evaluation.md](../../adr/2026-10-11-llm-source-model-routing-evaluation.md)（D2 選択規則・D3 段階と止める条件・D4 legacy）と
[2026-10-11-task-routing-features.md](../../adr/2026-10-11-task-routing-features.md)（判断 record・features hash/版）を実装した。
この file は全体検証（verify unit）の記録。各 unit の詳細は [selector/](selector/) の unit ごとの file。

## unit 一覧

| unit | 内容 | 状態 |
|---|---|---|
| merge-main | 先行 task（provider 廃止）入りの main を no-ff 取り込み | done |
| carry-fix | 取り込み後に落ちた migration 試験 11 件を直す（SCHEMA_VERSION 追従） | done |
| export-rewire | 旧 routing export・evaluate を消し、routing_export へ配線し直す | done |
| integrate-carry | 統合 | done |
| policy-core | `task-core::route_select`（根拠表型・D2 順位・fallback・ε 探索・D3 guardrail 純粋関数） | done |
| evidence-db | migration 0074（routing_evidence・routing_stage）と store・`routing_stage_changed` event | done |
| integrate-core | 統合 | done |
| lane-defaults | `route_defaults.<harness>.lanes.<lane>` と `[route_selection]` 節 | done |
| guardrail-tick | daemon の guardrail 判定（enforce を止める条件で shadow へ戻す） | done |
| selector-wire | dispatcher の RoutePolicy を route_select に差し替え、routing_decided に特徴量 hash と版 | done |
| stage-api | 段階・根拠表の読み取り API と人の enforce 承認（API・celerisctl・schema 再生成） | done |
| integrate-wire | 統合 | done |
| rm-legacy | 旧 shadow/enforce・estimator・optimizer・tier binding・`[model_routing]` の撤去（子 task、葉 hist・rm-daemon・rm-dispatch・rm-proxy・rm-store・rm-core・web-audit） | done（未解決 7 件は下の「未解決」） |
| rm-assign | 手動割り当て `model_assignments` の撤去 | done |
| rm-shadow / rm-estimator | rm-legacy に統合 | superseded |
| web-routing | web の routing 画面を段階・根拠表表示に作り直し、割り当て・catalog 画面を消す | done |
| integrate-remove | 統合 | done |
| verify（この file） | 全体検証と記録。fmt 1 件と architecture-map の 1 行を直した（下） | done |

## 受け入れ条件ごとの証拠

### 条件 0: test-parallel・clippy・fmt・web 検査・文書検査 3 本が exit 0

- `cargo fmt --all -- --check`: 初回 exit 1（`crates/task-ops/src/routing_audit_tests.rs` の整形のみ。試験の内容は変えず `cargo fmt --all` で整形）→ 再実行 exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0（`Finished`）。
- `TMPDIR=$(mktemp -d /local/x/sel/t.XXXX) bash scripts/dev/test-parallel.sh`: exit 0。`CELERIS_TEST_SUMMARY`: `nextest_binaries 164`, `passed 4985, failed 0, ignored 13`, `doctest_exit 0`, `tmp_leftovers 0`, `userns null`。ログ: `/local/x/sel/tp-verify.log`（ローカル）。
- web（`corepack pnpm@12.6.0 -C web …`、先に `install --offline --frozen-lockfile` exit 0）:
  - `typecheck`（`tsc -b`）exit 0。
  - `lint`（`biome check .`）exit 0。`Checked 476 files`。warning 4 件は `web/styles.css` の既存の `!important`（web-routing 報告と同じ）。
  - `test`: vitest `Test Files 92 passed (92)`、`Tests 650 passed (650)`。node `--test` `tests 93 / pass 93 / fail 0`。exit 0。
- 文書検査 3 本（agent-docs/README.md の land 系の検査）:
  - `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`（exit 0）。
  - `sh scripts/dev/check-adr-numbers.sh` → `ok (200 files)`（exit 0）。
  - `sh scripts/dev/progress-index.sh --check` → `ok`（exit 0）。
- 追加の文書検査（同じ変更を見るため）:
  - `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok（exit 0）。
  - `python3 scripts/dev/check-architecture-map.py` → 初回 exit 1: `docs/architecture-map.md` の model catalog 行に削除済みの `web/features/ops/models-screen.tsx` が残っていた（web-routing で削除）。その 1 path を行から外した → `OK: 344 件のパスを確認した`（exit 0）。

### 条件 1: この file が status done で、受け入れ条件ごとの証拠と本番への移行節を持つ

- 本 file の front matter `status: done`。条件 0・2・最終レビューの条件の証拠は本節と「本番への移行」節。

### 条件 2: legacy 名の git grep の残りが理由つきで表にある

コマンド: `git grep -n "model_assignments\|HeuristicEstimator\|routing_sidecar\|routing_enforce\|LaneResolution\|with_tier_models" -- crates web/features`
→ 出力なし（exit 1 = 0 件）。`crates/` と `web/features/` には残っていない。

crates・web 以外（`git grep -n … -- . ':!agent-docs'` と `[model_routing]`）の残り:

| 場所 | 残り | 理由 |
|---|---|---|
| `docs/api/v1/api.md:3322` | 旧 `model_assignments` の説明文 | 削除の注記（API 文書の変更説明）。残す |
| `scripts/selfdeploy/tests/fixtures/release-build-deps-inventory.tsv:138` | `model_assignments-<hash>` の依存表の固定行 | 試験 fixture の固定データ。機能と無関係。残す |
| `crates/celeris/src/config/mod.rs:400`、`config/tests.rs:3308` | `[model_routing]` の読み込み拒否 | 意図的（旧 section は読み込みエラー） |
| `docs/ops/model-routing-migration.md` | 旧設定の削除手順 | 移行手順 |
| `crates/task-core/migrations/0049`・`0053`・`0054`・`0072`・`0075` | 旧表の作成と drop | migration 履歴。0049 等は書き換えない |
| `crates/task-api/tests/routing.rs`、`crates/task-ops/tests/fixtures/routing_shadow_recorded.json` 他 | 旧 event の読み取り試験と fixture | 追記専用 event の読み取り（`routing_shadow_history`）。残す |
| `agent-docs/adr/*`・`agent-docs/progress/*` | 過去の判断・進捗の記述 | 履歴。書き換えない（ADR の方針） |
| `docs/reports/model-routing-routellm-shadow.md` | 旧 estimator sidecar（Phase 5）の評価記録 | **未処理**。削除された仕組みの報告。下の「提案」1 |

lane の `Tier` 型は ADR D4 により方針ラベルとして残す（上の表の対象外）。

### 最終レビューの条件（Task の受け入れ条件）

- 0. workspace 試験が通る: 条件 0 の test-parallel（4985 passed, 0 failed）。
- 1. clippy が通る: 条件 0 の clippy（exit 0）。
- 2. 進捗 file に削除した仕組みの一覧と本番 config の書き換え手順がある: 下の「削除した仕組み」と「本番への移行」。
- 3. 既定が route_defaults と lane ごとの人の既定の 1 経路で、旧い tier binding・手動割り当ての経路が残っていない: `[model_routing]` は読み込みで拒否（`config/mod.rs:400`）。`crates/` に `model_assignments`・`LaneResolution`・`with_tier_models`・`routing_enforce` の hit は無い。選択は dispatcher の `RoutePolicy` → `task_core::route_select::select` の 1 経路（`selector-wire`）。

## 削除した仕組み（同じ変更で消した）

- 手動 model 割り当て: API・CoS operation・store・role/tier slot の型（rm-assign。migration 0072 で `model_role_assignments`・`model_role_scopes` を drop）。
- 旧 shadow/enforce: daemon の `routing_shadow*`・`routing_sidecar*` と配線、dispatcher の `routing_shadow`・`routing_enforce`・`routing_members`、llm-proxy の `shadow`・`shadow_budget`・`estimator_*`・`selection_state`・`catalog`（rm-daemon・rm-dispatch・rm-proxy）。shadow の予約表は migration 0075 で drop。
- estimator・optimizer・policy・shadow の model router、HeuristicEstimator と sidecar、tier→model 解決、`with_tier_models`、`LaneResolution`（rm-core・rm-legacy）。
- `[model_routing]` 全欄（読み込みエラー。案内文付き）、`GET /api/v1/llm/routing/catalog` と web の routing catalog 画面、旧 routing 監査画面の予約・estimator・optimizer 欄（rm-core・web-audit）。
- `/models` の割り当て画面（web-routing。`/routing` に置き換え）。
- `routing replay`・`routing evaluate`（export-rewire。`routing export` は `celeris.route-training/1` JSONL）。
- 残したもの: `RoutePolicy`・`route_defaults`・`route_select/`、lane の `Tier`（方針ラベル）、`model_router::feedback`・`context`・`profiles`（`SourceState` 等。llm_sources と guardrail が使う）、`routing_shadow_recorded` の event 型と読み取り（`task_core::routing_shadow_history`。追記専用の履歴）、`[model_catalog]`（別機能）。

## 段階・根拠の仕組み（実装の要点）

- 段階: genre×harness ごとに `shadow` | `enforce`。`routing_stage` に行が無い組は `shadow`。変更は `routing_stage_changed` event を同じ transaction で追記。
- enforce へ進めるのは人だけ（`actor = human`）。根拠表の最新版に「根拠が立つ」行（`n >= 24` かつ Wilson 下限 ≥ 0.5）が無ければ拒否（409 `routing_evidence_insufficient`、CLI も拒否）。
- 戻すのは daemon だけ（`actor = daemon`）。guardrail（15 分ごと、直近の `routing_outcome_recorded` の窓 50 件）が ADR D3 の停止条件に当たると shadow へ戻し、`routing_stage_changed` と受信箱通知を出す。quota の床（既定 10%）を下回るときは通知だけで段階は変えない。自動の昇格は無い。
- 選択: enforce は ADR D2（根拠の同等集合 → quota 消費の最小 → 決定的順位、ε=0.1 の run_id 固定探索）。shadow と根拠の無い組は harness×lane の `route_defaults` 優先列。admission は選ばれた route の順位付き fallback を順に試し、全滅なら fail closed。
- 記録: `routing_decided` に `celeris.route-decision/1`（stage・rule・evidence version・propensity・除外・options hash・features ref）。`route_decisions` 表に保存し、decision id は run id。

## 未解決（verify の時点で残るもの。コードは変えていない）

1. `cargo test` 直接実行で TMPDIR に `.tmp*` が残る件（rm-legacy 未解決 1）は、この verify では再確認していない（`scripts/dev/check-test-tmp-leftovers.sh` は workspace 全体の `cargo test` を走らせ時間が長いため、この run では回していない）。test-parallel の `tmp_leftovers 0` は確認済み。
2. `crates/llm-proxy/src/reservation.rs`（`ReservationTable`）が非試験の呼び出しを持たない（rm-legacy 未解決 2）。
3. `[llm_proxy] prefer_free`（`crates/llm-proxy/src/config.rs:20-21`）は読まれたまま無視される。規則「消した config 欄は読み込みエラー」に反する（rm-legacy 未解決 3）。
4. 拒否文（`crates/celeris/src/config/mod.rs:400`）は `[routing] route_defaults` と書くが、正しくは top-level の `[route_defaults.<harness>]`（rm-legacy 未解決 4）。
5. request 単位の routing trace が API から消えた（`RequestRoutingAudit` は decision_id・request_id のみ）。候補の除外理由・score・費用は出ない（rm-legacy 未解決 5。人の判断）。
6. e2e（Playwright）は走らせていない。web の検査は typecheck・lint・vitest・node test まで（rm-legacy 未解決 6）。
7. `CELERIS_USERNS_TESTS=1` の全体検査は走らせていない（`userns: null`）。この task の条件には含まれないが、最終レビューが要るなら close-out に別に置く。
8. 根拠表は `n`・`pass_lower` だけで D1 を判定する。高価 model の「12 問×1」と paired 比較の確定はまだ判定していない（stage-api 未解決）。
9. `GET /routing/stages` は guardrail の窓（直近 run 数・違反数）を出さない（stage-api 未解決。web-routing 側も表示しない）。
10. 承認の event は `task_id` 必須（承認を記録する task を指定する）。task に属さない system event の口は無い（stage-api 未解決）。

## 提案

1. `docs/reports/model-routing-routellm-shadow.md`（旧 estimator sidecar の評価記録）を削除するか、履歴として残すかを人が決める。方針どおりなら削除（git に残る）。
2. 未解決 2〜4 は小さな後続 task にする（`ReservationTable` の削除、`prefer_free` の読み込みエラー化、拒否文の修正）。
3. 未解決 5（request 単位 trace）は人が判断する。戻すなら selector-wire の wire 型に乗せる。
4. 未解決 9・10 は stage-api の提案どおり（根拠表に問題数・paired 結果を持たせる、system event の帰属を作る）。

## 本番への移行（人が実行する手順。エージェントは本番に触れていない）

本番の config（`~/.config/celeris`）と本番 DB は、この変更の release を起動する前に人が操作する。手順の詳細は [docs/ops/model-routing-migration.md](../../../docs/ops/model-routing-migration.md) と rm-legacy file の「本番への移行」節にある。要点:

### 1. 本番 config の `[model_routing]` を `route_defaults` に書き換える

- 新しい release は `[model_routing]` を含む config を読み込みエラーにする。旧 release のまま config だけを変えると旧 daemon が動かなくなるので、順序は「config を書き換えてから新 release を起動」。
- 手順:
  1. 退避: `cp -a ~/.config/celeris ~/.config/celeris.bak-$(date +%Y%m%d)`
  2. 旧表の場所を確認: `grep -rn '^\[model_routing' ~/.config/celeris`
  3. 旧 `[model_routing]` の tier→model の対応を読み、harness ごと（必要なら lane ごと）の候補を人が決める。自動変換はしない。
  4. 次の形で書く（source・model・harness は本番 config の実名。候補は `[llm_sources.<id>]` と `[harnesses.<id>]` で宣言済みのものだけ。宣言が無いと読み込みで失敗する）:
     ```toml
     [route_defaults.<harness>]
     candidates = [{ source = "<source_id>", model = "<model_id>" }]

     [route_defaults.<harness>.lanes.<lane>]
     candidates = [{ source = "<source_id>", model = "<model_id>" }]
     ```
  5. `[model_routing]` の表を削除する。`[llm_proxy] prefer_free` があれば削除（未解決 3。残っても無視されるだけ）。
- 確認:
  - `grep -n '\[model_routing' ~/.config/celeris/*.toml` が 0 件。
  - 新 release の起動後、`journalctl --user -u celeris@<sha12> -n 50` で config の読み込みエラーが無いこと。`[model_routing] is no longer supported` が出たら手順 2〜5 の残り。
  - `GET /api/v1/harnesses` と `GET /api/v1/llm/models` で候補の宣言と発見状態を確認。`GET /api/v1/llm/routing/catalog` は削除済みなので 404 が正しい。

### 2. routing_stage の初期値は全部 shadow

- 移行直後は `routing_stage` の行が無いので、全 genre×harness が `shadow`（route_defaults の harness×lane 優先列で選ぶ）。enforce は 1 つも有効になっていない。
- 確認: `celerisctl routing stage list --db <db> --json` で全組が `shadow`。

### 3. enforce の承認（人の操作。根拠が立つ組だけ）

- 根拠の確認: `celerisctl routing evidence --db <db> [--genre <g> --harness <h>]`（根拠が立つ行は `qualifies`）。
- 承認: `celerisctl routing stage set --db <db> --task <承認を記録する task id> --genre <g> --harness <h> --stage enforce --reason "<理由>"`。API は `POST /api/v1/routing/stages`（actor は human）。根拠が立たなければ拒否される。
- 戻す: 人も `--stage shadow` で戻せる。daemon の guardrail は自動で shadow へ戻す（通知が受信箱に出る）。

### 4. migration（起動時に自動。人が確認する）

- 版: `SCHEMA_VERSION` 75（`crates/task-core/src/store/migrations.rs`）。
- この task で追加された migration: `0072_drop_model_role_assignments.sql`（手動割り当て表の drop）、`0074_routing_evidence_stage.sql`（`routing_evidence`・`routing_stage` の追加）、`0075_drop_routing_shadow_reservations.sql`（shadow 予約表の drop）。`0071`・`0073` は先行 task の取り込みで main に入っていた。0049 等は書き換えない。
- 確認（バックアップを取ってから読み取り専用で）:
  - `sqlite3 -readonly <db> "PRAGMA user_version;"` → 75
  - `sqlite3 -readonly <db> "SELECT name FROM sqlite_master WHERE name LIKE 'routing_shadow_reservations%';"` → 0 行

### 5. events は残す（消さない・書き換えない）

- 移行の前に `sqlite3 -readonly <db> "SELECT COUNT(*) FROM events WHERE json LIKE '%\"type\":\"routing_shadow_recorded\"%';"` を取り、起動後に同じ値であることを確かめる。旧 event は `routing_shadow_history` の型で audit に読める。
