---
tasks: [01M4TJFJFTA037YD6D3AGZ8SXJ]
unit: record
status: 実装完了・検査は一部未解決（下記「未解決」）
date: 2026-10-13
---
# rm-legacy: 旧 shadow/enforce・estimator・optimizer・tier binding・[model_routing] の撤去（ADR D4）

## 結果（要約）

旧 shadow/enforce、estimator・optimizer・policy・shadow の model router、tier→model 解決、`[model_routing]` の経路を 1 つの変更群で消した。新しい source/model 選択は dispatcher の `RoutePolicy`（`route_defaults.<harness>`・`lanes.<lane>`）の 1 点だけになった。過去の `routing_shadow_recorded` event は読み取り専用の型（`task_core::routing_shadow_history`）で読める。表 `routing_shadow_reservations` は migration 0075 で drop した。events は書き換えも削除もしていない。

受け入れ条件の検査はすべて通った（test-parallel 4985 passed / 0 failed、clippy exit 0、gen:types 差分なし、文書検査 4 本 ok）。一括 `cargo test` の TMPDIR 残存だけは再現する未解決として残す（下の「未解決」1）。

## 葉ごとの結果（先行葉のメモの要約）

| 葉 | 内容 | 状態 |
|---|---|---|
| hist | 履歴の型を `task_core::routing_shadow_history` へ移し、旧形式 JSON fixture（`crates/task-ops/tests/fixtures/routing_shadow_recorded.json`）で読めることを確かめた。試験 `legacy_shadow_history_fixture_deserializes_and_appears_in_audit` | done |
| rm-daemon | daemon の `routing_shadow*`・`routing_sidecar*` と、その配線（services・admin・bootstrap・run）を削除。新規の `routing_shadow_recorded` 書き込み経路を無くした | done |
| rm-dispatch | dispatcher の `routing_shadow`・`routing_enforce`・`routing_members` と公開 export・setter を削除。`dispatch_run` の enforce 呼び出しと `LaneResolution` の組み立てを外した。選択は既存 `RoutePolicy` 経路だけ | done |
| rm-proxy | llm-proxy の `shadow`・`shadow_budget`・`estimator_shadow`・`estimator_sidecar`・`selection_state`・`catalog` を削除。`selection.rs` は account 順位付けと relay の到達性だけを持つ。proxy は dispatcher が決めた `<source>/<model>` を実行する | done |
| rm-store | store の shadow 予約 API・型・試験を削除。migration `0075_drop_routing_shadow_reservations.sql` で `routing_shadow_reservations` と index を drop（0049 は書き換えない）。`SCHEMA_VERSION` 74→75 と固定値の試験を追随。task-ops `RequestLog::shadow_reservation` と `RoutingShadowAudit.reservation` を削除 | done |
| rm-core | celeris config の旧 `model_routing` module・runtime・catalog 統合を削除。`[model_routing]` を含む TOML は route_defaults を案内して拒否（試験 `model_routing_section_rejected_with_route_defaults_guidance`）。`GET /llm/routing/catalog` の route・型・試験を削除。task-core の estimator・optimizer・policy・shadow など旧 `model_router` 群、`LaneResolution`、tier→model 解決を削除。Tier と selector と読み取り専用の履歴型は残す。API schema と web 生成型を再生成 | done |
| web-audit | web の routing catalog 画面（`web/features/ops/routing-catalog*.ts(x)`）を表示ごと削除。`routing-audit-view.tsx` から予約・上限消費・estimator・optimizer・候補ごとの除外理由・score・費用内訳の表示を消した。e2e fixture の旧欄も消した | done |

### GET /llm/routing/catalog と web の catalog 画面

- API: `GET /llm/routing/catalog` の route・型・試験を rm-core で削除した。model catalog の API（`[model_catalog]` の発見・override）は残す。
- web: `routing-catalog.ts`・`routing-catalog-section.tsx`・`routing-catalog.test.tsx` を web-audit で削除した。`UNKNOWN`（欠測の表示）は `web/features/ops/routing-state.ts` へ移した。

### 削除の経路（残り）

- `crates/celeris/src/config/model_routing.rs`・`model_catalog.rs` の旧 `[model_routing]` 経路、`crates/task-worker` の `with_tier_models` は、削除済みの範囲に含まれる（`git grep` で `with_tier_models`・`LaneResolution`・`estimator::`・`optimizer::` に hit 0）。
- `rm-router`・`rm-config`・`integrate-core` の計画欄は superseded。内容は rm-core が 1 つの変更で行った。

## 残す型と理由

- `RoutePolicy`・`route_defaults`・`task_core/src/route_select/`（selector 本体）。
- `Tier`（lane）。方針ラベルとして残す。
- `model_router::feedback`（`RoutingOutcome` ほか）・`context`・`context_registry`・`profiles`（`SourceState`・`Reachability` 等）。`task-api llm_sources` と guardrail が使う。
- `routing_shadow_recorded` の event 型（`Event::RoutingShadowRecorded`）と task-api の event kind・旧 event の読み取り路。新規の書き込みだけを無くした。
- `[model_catalog]`（カタログ取得設定）と `task_core::model_catalog`。旧 `[model_routing]` とは別機能。

## 検査（record 葉、2026-10-13、worktree `3e794a9a` 基点）

- `TMPDIR=$(mktemp -d /local/x/sel/t.XXXX) bash scripts/dev/test-parallel.sh` → exit 0。`CELERIS_TEST_SUMMARY`: `passed 4985, failed 0, ignored 13, nextest_exit 0, doctest_exit 0, tmp_leftovers 0, userns null`
- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished` を確認）
- `corepack pnpm@12.6.0 -C web gen:types` → exit 0。`web/api/generated` を写してから再生成し `diff -r` で差分なし。`git status --porcelain web/api/generated` も空
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）
- `python3 scripts/dev/check-architecture-map.py` → exit 0（`OK: 345 件のパスを確認した`）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0（引数に tsv が要る。引数なしは usage で exit 1）
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (200 files)`）
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0（web-audit の結果を再確認）
- 試験の実在: `legacy_shadow_history_fixture_deserializes_and_appears_in_audit`（task-ops）、`model_routing_section_rejected_with_route_defaults_guidance`（celeris config）、`migration_0075_drops_shadow_reservations_and_keeps_shadow_events`（task-core）は、それぞれの所在を `git grep -n 'fn <name>'` で確認した

## 未解決

1. **一括 `cargo test` で TMPDIR に `.tmpXXXXXX` が残る（再現 2/2）。** `sh scripts/dev/check-test-tmp-leftovers.sh`（引数なし、短い TMPDIR）が exit 1。残るのは `.tmp*` 1 件。`-p <crate>` を 11 package（`crates/*` と `tests/e2e`）ごとに流すと残らず、`cargo test --workspace --doc` でも残らない。nextest の gate（test-parallel）は `tmp_leftovers: 0`。原因（一括実行で後から作られる dir を作る試験）は未特定。直さずに記録した。
2. **llm-proxy の `reservation.rs`（`ReservationTable`・`CapacityLimits`）が死んでいる。** 非試験の呼び出しは無い（`ReservationTable::new` は `reservation_tests.rs` だけ）。rm-proxy のメモは「`model_routing.rs` だけが使う」と書いていた。`Clock`・`SystemClock` は `server.rs` が使うので残す。後続で `ReservationTable`・`CapacityLimits`・`reservation_tests.rs` を消す。
3. **`[llm_proxy] prefer_free` が読まれたまま無視される。** `crates/llm-proxy/src/config.rs:20-21` の欄は読み手が消えた（llm-proxy 内の試験だけが使う）。「消した config 欄は読み込みエラー」の規則に反する。後続で欄を消し、書かれていたら読み込みエラーにする。
4. **拒否文が実際の書き方と違う。** `crates/celeris/src/config/mod.rs:400` の文は「`[routing]` route_defaults」と書くが、config に `[routing]` 表は無い。正しくは top-level の `[route_defaults.<harness>]`（と `[route_defaults.<harness>.lanes.<lane>]`）。文を直す後続が要る。試験は `route_defaults` の語だけを見ているので通ってしまう。
5. **request 単位の routing trace が API から消えた。** web-audit の報告。`RequestRoutingAudit` は `decision_id`・`request_id` だけになり、候補の除外理由・score・費用 4 成分は出ない。`routing_request_decided` event 自体は残る。survey は trace を selector-wire の wire 型へ移す案だった。API から候補 trace を消すのが意図どおりかは人が確認する。
6. **e2e（Playwright）は走らせていない。** web-audit は typecheck と vitest（93 files / 661 tests）だけ。
7. **USERNS=1 の全体検査は走らせていない**（test-parallel の `userns: null`）。この record 葉の受け入れ条件には含まれないが、close-out 葉が必要とするなら別に置く。

## 本番への移行（人が実行する手順）

本番 config（`~/.config/celeris`）と本番 DB には、エージェントは触れていない。以下は人が実行する。

### 1. 本番 config から `[model_routing]` を消し、`route_defaults` へ移す

- 前提: 新しい release（この変更を含む）を起動する前に行う。旧 release のまま config だけを変えると、旧 `[model_routing]` を読む旧 daemon が動かなくなる。
- 手順:
  1. 退避する: `cp -a ~/.config/celeris ~/.config/celeris.bak-$(date +%Y%m%d)`
  2. 旧 `[model_routing]` の表を探す: `grep -rn '^\[model_routing' ~/.config/celeris`
  3. 旧 `[model_routing]` が決めていた tier→model の対応を読み、harness ごと・lane ごとに新しい候補を決める。候補は `llm_sources`（source）と、その source の models に含まれる model の組。ここは人が決める（自動変換はしない）。
  4. 次の形で書き足す（例。source/model/harness は本番 config の実名に合わせる）:
     ```toml
     [route_defaults.<harness>]
     candidates = [{ source = "<source_id>", model = "<model_id>" }]

     [route_defaults.<harness>.lanes.<lane>]
     candidates = [{ source = "<source_id>", model = "<model_id>" }]
     ```
     候補は `[llm_sources.<id>]` と `[harnesses.<id>]` で宣言済みのものだけ。宣言が無いと `route_defaults references undeclared ...` で読み込みに失敗する。
  5. `[model_routing]` の表（と、そこだけが参照していた欄）を削除する。
  6. `[llm_proxy] prefer_free` があれば削除する（未解決 3。後続が入るまでは残っても無視されるだけ）。
- 確認:
  - `grep -n '\[model_routing' ~/.config/celeris/*.toml` が 0 件。
  - 新 release を起動して、daemon が config 読み込みで落ちないことを `journalctl --user -u celeris@<sha12> -n 50` で見る。`[model_routing] is no longer supported` が出たら、手順 2〜5 が残っている。
  - API の確認: `GET /api/v1/llm/routing/catalog` は削除済みなので 404 になるのが正しい。model catalog 系の endpoint は残るので影響は無い。

### 2. migration で表を drop する（自動。人が確認する）

- 起動時の migration（版 74→75）で `routing_shadow_reservations` と `idx_routing_shadow_reservations_day` が drop される（`0075_drop_routing_shadow_reservations.sql`）。0049 は書き換えない。
- 確認（DB のバックアップを取ってから、読み取り専用で）:
  - `sqlite3 -readonly <db> "SELECT name FROM sqlite_master WHERE name LIKE 'routing_shadow_reservations%' OR name='idx_routing_shadow_reservations_day';"` → 0 行
  - `sqlite3 -readonly <db> "PRAGMA user_version;"` → 75

### 3. events は残る（消さない・書き換えない）

- 過去の `routing_shadow_recorded` は events 表の行として残る。`events` は追記専用。
- 確認: 移行の前後で同じ件数になること。
  - `sqlite3 -readonly <db> "SELECT COUNT(*) FROM events WHERE json LIKE '%\"type\":\"routing_shadow_recorded\"%';"` を移行前に取り、起動後に同じ値であることを確かめる。
- 旧 event の読み取りは `routing_shadow_history` の型で行える（task-ops の audit 応答に出る）。

## 提案（後続）

- 未解決 1〜4 の直し（TMPDIR 残存の原因特定、`ReservationTable` の削除、`prefer_free` の読み込みエラー化、拒否文の修正）。
- 未解決 5 は人の判断。候補 trace を API に戻すかを決める。
- 本番移行の手順 1 は、旧 `[model_routing]` の tier→model 対応を人が決めてから行う。
