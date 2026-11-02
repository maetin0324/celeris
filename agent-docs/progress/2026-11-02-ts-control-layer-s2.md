---
title: S2 — 純粋な判定ロジック 2 領域（lane_rules・route_priority）の TS 同等実装と、Rust を正とする compare モード
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 — 純粋な判定ロジックの TS 同等実装と compare モード

完了日: 2026-11-02（task 01M6E6HEH7CPZX9KX4Y2SAZKTG。2 つ目の領域は子 task 01M6EJYCFFSJXYBVH87Q8FSNYB。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- 2 領域を TS に同等実装し、Rust を正とする compare を本物の呼び出し経路に付けた。
  - `lane_rules`（`model_policy::match_rule`）: fixture 19,683 件（9 軸の全組み合わせ）が Rust と TS で全件一致。
    結線は `dispatch_run.rs` の `decide_lane`（人の明示・System の task では TS を呼ばない）。
  - `route_priority`（`llm_route::route_priority_decide`）: fixture 2,026 件（手書き 26・固定 seed の生成 2,000）が全件一致。
    結線は `dispatch_run.rs` の `select_route_with` の直後（cos・review_spawn・lane_assist の試走では呼ばない）。
- compare は Rust の結果を変えない。TS の結果は event `script_compare_recorded` に書くだけで、dispatcher は TS を待たない（別の tokio task、deadline 50 ms）。
- mode は DB の表 `script_domain_modes`（migration 0103）が持ち、`rust`・`compare` だけを受け付ける。`celerisctl script mode <domain> rust` で戻せる。
  daemon は起動時に、bundle が読めない・hash が違う領域を `rust` に戻す。
- 本番の mode は全領域 `rust` のまま。本番で compare を有効にする手順は「本番への移行」に書いた（人が行う）。
- `direct_route` は Rust に残した（境界 ADR S2-1）。旧 `routing_shadow`・`[model_routing]`・tier binding は使っていない。

## 受け入れ条件ごとの証拠
各 unit の記録は `agent-docs/progress/2026-11-02-ts-control-layer-s2/<unit>.md`（route_priority は `…/route-prio.md` と `…/route-prio/<unit>.md`）。
下の結果は close unit が提出時の HEAD で実行したもの（基準は `$CELERIS_WU_BASE`）。

### 0. 対象領域の決定的 fixture が Rust と TS で全一致し、比較ログが残る
- `node control-ts/scripts/compare.mjs lane_rules` → `lane_rules: 19683/19683 matched (fixtures/lane_rules/all-features.jsonl)`、exit 0。
- `node control-ts/scripts/compare.mjs route_priority` → `route_priority: 2026/2026 matched (fixtures/route_priority/cases.jsonl)`、exit 0。
- 本物の deno_core host で全件（同じ isolate に正順と逆順の 2 回、isolate の作り直し 0 回）:
  `cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile && corepack pnpm@12.6.0 run bundle && cd .. && CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_compare_lane_rules --test script_compare_route_priority`
  → exit 0、14 passed・0 skipped。全件の試験は `script_compare_real_host_matches_rust_on_all_fixtures` と `script_compare_route_priority_real_host_matches_rust_on_all_fixtures`。
- fixture と Rust の byte 一致は `crates/task-core/tests/script_fixtures.rs`（example `dump-lane-fixtures`・`dump-route-priority-fixtures` と同じ関数）。
- 比較ログ: event `script_compare_recorded`（17 欄、outcome 5 種。境界 ADR S2-4）。記録の試験は `script_compare_match_records_and_keeps_rust_lane`・
  `script_compare_route_priority_match_records_one_per_decision`（task-dispatch）と、本物の host の `script_compare_dispatch_match_rollback_and_replay`・
  `script_compare_route_priority_dispatch_match_rollback_and_replay`（記録の入力を TS に通し直すと同じ byte 列になる replay を含む）。
- `cargo nextest run -p task-dispatch -p task-api -p task-core -E 'test(script_compare) | test(script_domain) | test(script_fixtures) | test(lane_fixture) | test(route_priority)'` → exit 0、65 passed。

### 1. 本物の呼び出し経路で compare を検証し、Rust 参照へ切り戻せる
- daemon の経路: `script_compare_daemon_injects_records_and_restores_rust`（`celeris::run` を通し、起動で `loaded` の generation を作り、compare の dispatch が `match` を記録する。
  `[script]` を外すか配備先に無い bundle を指すと `system:script-startup` が `rust` へ戻す）。
- 切り戻し: `script_compare_dispatch_match_rollback_and_replay`・`script_compare_route_priority_dispatch_match_rollback_and_replay`（`rust` に戻した後は記録 0 件、actor の完了数が増えない）、
  `script_compare_mode_rust_never_calls_ts`・`script_compare_route_priority_mode_rust_never_calls_ts`。
- mode API: `crates/task-api/tests/script_domains.rs` の `script_domain_mode_rust_compare_rust_round_trip`・`script_domain_mode_through_cos_operations`、
  canary・ts の 422、古い version の 409（どの拒否でも表と `script_domain_events` は変わらない）。
- 配備: `cargo nextest run -p celeris-script -E 'test(script_deploy)'` → exit 0、3 passed。

### 2. 安全回帰が無い。見つけた現行 bug は別 task として記録されている
安全回帰が無いことは次の試験で固定した（すべて上の実行で合格）。
- 天井越えが出ない: `script_compare_lane_after_ceiling_is_rust`（担当の `max_lane = standard` で、TS が cheap・frontier のどちらを返しても lane は standard）。
- 人の明示の上書きが出ない: `script_compare_human_tier_never_calls_ts`（人の明示の task では TS の呼び出し 0 回、lane は人が決めた cheap）。
- Rust の結果が変わらない（常に違う答え・例外・不正出力・timeout）: `script_compare_mismatch_records_and_keeps_rust_lane`・`script_compare_ts_error_and_invalid_output_keep_rust_lane`・
  `script_compare_timeout_records_ts_error_and_keeps_rust_lane`・`script_compare_route_priority_mismatch_keeps_rust_route_bytes`・`script_compare_route_priority_errors_keep_rust_route_bytes`・
  `script_compare_route_priority_timeout_records_ts_error`（task-dispatch）、`script_compare_different_answer_keeps_rust_decision_bytes`・`script_compare_throwing_bundle_keeps_rust_lane`・
  `script_compare_route_priority_different_answer_keeps_rust_route`・`script_compare_route_priority_throwing_bundle_keeps_rust_route`（本物の host。`routing_decided`・route の event が基準と byte 一致）。
- 宣言外の候補を検出する: `script_compare_route_priority_undeclared_candidate_keeps_rust_route`（`safety_violations` が `["undeclared_candidate"]`、route は Rust のまま）。
- 対象外の経路で TS を呼ばない: `script_compare_route_priority_skips_cos`・`…_skips_review_spawn`・`…_skips_lane_assist_trials`。
- 機密が記録に残らない: `script_compare_route_input_keeps_only_allowlisted_ids`・`script_compare_lane_input_is_nine_levels_only`（task-core）と、本物の host の route_priority の replay 試験（endpoint・credential_ref・account・title が記録に無い）。
- 見つけた現行の挙動と bug 候補は「提案」に別 task 候補として書いた。どれも直していない。安全側に止めたものは「安全側に止めたもの」に書いた。

### 3. 変更の影響範囲の test が通る
各 unit が自分の HEAD で実行した（数は unit の記録）。統合の段（integrate-prep・integrate-build・integrate-wire・integrate-second）も決定的な check を通った。
- 例: compare-store `test-affected.sh` exit 0（cargo 6,903 passed・21 skipped、web 697 passed）、route-prio core exit 0（nextest 6,953 passed・21 skipped）、
  route-prio hook exit 0（2,981 passed・9 skipped）、stale-dist exit 0（870 passed・0 skipped）。各 unit の `cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` は exit 0。
- close unit の差分は文書だけ（この file・`docs/architecture-map.md`・境界 ADR）。close unit の検査の結果は run の報告に書く。

## unit と commit（件名）
| unit | 内容 | commit の件名 |
|---|---|---|
| [adr-compare](2026-11-02-ts-control-layer-s2/adr-compare.md) | 境界 ADR の付記 S2-1〜S2-8 | `phase 1: adr-compare: 境界 ADR に S2 の compare mode の付記、進捗を書く` |
| [policy-split](2026-11-02-ts-control-layer-s2/policy-split.md) | `match_rule`/`decide_with` の分割（挙動不変）と lane fixture | `phase s2: policy-split: model_policy を match_rule と decide_with に分ける（挙動不変）`、`phase s2: policy-split: lane 規則の比較 fixture（dump-lane-fixtures と script_fixtures 試験）`、`phase s2: policy-split: 進捗の記録` |
| [compare-store](2026-11-02-ts-control-layer-s2/compare-store.md) | migration 0103・mode の CAS・compare event・機密除去 | `phase s2: compare-store: script_domains の migration・mode の CAS 更新・compare event の store`、`phase s2: compare-store: 進捗の記録を replan 後の範囲に合わせる` |
| [deploy-cmd](2026-11-02-ts-control-layer-s2/deploy-cmd.md) | `script-deploy` | `phase s2: deploy-cmd: bundle を state_dir に配備する script-deploy command と試験` |
| [lane-rules-ts](2026-11-02-ts-control-layer-s2/lane-rules-ts.md) | lane_rules の TS と `compare.mjs` | `phase s2: lane-rules-ts: lane_rules 領域の TS 実装・単体試験・fixture 比較 script` |
| [mode-api](2026-11-02-ts-control-layer-s2/mode-api.md) | `GET/PUT /api/v1/script/domains…`・CoS 操作・`celerisctl script` | `phase s2: mode-api: 領域の mode を読む・切り替える API・CoS 操作・celerisctl script`、`phase s2: mode-api: celerisctl の cos_mapped 試験の表を source-size の上限に収める`、`phase s2: mode-api: 進捗の証拠`、`phase s2: mode-api: 範囲外 2 file が試験に必須であることの確認を記録`、`phase s2: mode-api: replan 後の再検査と進捗の完了` |
| [dispatch-hook](2026-11-02-ts-control-layer-s2/dispatch-hook.md) | `trait PolicyScript` と `decide_lane` の結線 | `phase s2: dispatch-hook: decide_lane の lane_rules に compare を付ける（trait PolicyScript）`、`phase s2: dispatch-hook: check の不合格の調査を進捗に記録` |
| [host-adapter](2026-11-02-ts-control-layer-s2/host-adapter.md) | `ScriptPolicy`（本物の host）・daemon の注入・起動時の戻し | `phase s2: host-adapter: PolicyScript を ScriptRuntimeHandle で実装し daemon から注入、本物の host で試す`、`phase s2: host-adapter: 地図の関連試験と証拠を記録` |
| [thread-census](2026-11-02-ts-control-layer-s2/thread-census.md) | `script_runtime_valid_bundle_loads` の flaky（試験だけ） | `phase s2: thread-census: thread の観察を tid 単位にして親の名前の引き継ぎを actor と数えない`、`phase s2: thread-census: 進捗に検査の結果を記録` |
| [egress-stray](2026-11-02-ts-control-layer-s2/egress-stray.md) | egress の「接続しない」試験の迷い込み対策（試験だけ） | `phase s2: egress-stray: 接続しない確認を迷い込みと区別できる helper に直す`、`phase s2: egress-stray: 原因と確認の記録` |
| [route-prio](2026-11-02-ts-control-layer-s2/route-prio.md)（子 task） | route_priority の分割・fixture・TS・結線・本物の host | `phase s2: route-prio core: …`（3 件）、`phase s2: route-prio ts: …`（2 件）、`phase s2: route-prio hook: …`（4 件）、`phase s2: route-prio real-host: …`、`phase s2: route-prio close: 進捗のまとめ`（全件の件名は route-prio.md） |
| [stale-dist](2026-11-02-ts-control-layer-s2/stale-dist.md) | 古い `control-ts/dist` を SKIP にする（試験だけ） | `phase s2: stale-dist: 古い control-ts/dist を SKIP する判定と stale_dist 試験`、`phase s2: stale-dist: 検査結果の記録` |
| close | この file・architecture-map・境界 ADR の S2-12 | `phase s2: close: S2 の進捗まとめ・architecture-map・境界 ADR の実装との突き合わせ` |

統合の commit（`integrate wu/…`）は celeris が作った。

## stale-dist の記録（古い dist を SKIP にした理由）
- integrate-second の test-affected で、本物の host の route_priority の試験 2 件が落ちた（`unknown domain: route_priority` など）。
- 原因: `control-ts/dist` は git の追跡外で、task 根の worktree に route_priority を足す前の source で作った bundle（manifest の domains が `host_selftest`・`lane_rules` だけ）が残っていた。
  試験の helper は dist が読めれば使うので、古い bundle を正として比べた。本番コードの不具合ではない。
- 直し方: 試験の helper（`crates/celeris/tests/support/script_compare.rs` の `usable_bundle`）が、manifest の `domains` の集合が `control-ts/bundle.config.json` の key 集合と違う bundle と、
  試験が要る領域を含まない bundle を「古い bundle」として SKIP する。`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら失敗にする。
  SKIP にしたのは、dist の有無と鮮度は source の正しさと別の事柄で、dist を作り直さない check（test-affected）で落とすと source と無関係に落ちるため。
  鮮度を必ず確かめたい場では REQUIRE=1 で失敗にできる。
- 残り: 統合の check は dist を作り直さないので、古い dist のある worktree ではこれらの試験が SKIP で通る（提案 1）。

## 安全側に止めたもの
- compare は記録だけで、dispatcher は TS を待たない（付記 S2-6 の「最大 50 ms 待つ」より安全側。S2-12）。
- 起動時に、generation の bundle と読み込めた bundle が違う領域は `rust` に戻す（`system:script-startup`）。
- 両領域とも capability を許可しない（scope 空、S2-7）。
- 検証に落ちた bundle の `rejected` の行は作らない（S2-9。行を作ると同じ hash を二度と compare にできなくなるため）。

## 未解決
1. 入出力の型（`lane_rules.ts`・`route_priority.ts`）は手書きで、Rust の DTO から生成していない。DTO を変えると fixture と本物の host の試験で食い違いが出るが、型検査では出ない。
2. `release.sh`・`test-affected.sh` は `control-ts/dist` を作らないので、dist が無いか古い環境では dist を使う 5 件（lane_rules 3・route_priority 2）が SKIP で通る。
3. `ceiling_exceeded` は今の `LaneCeiling::clamp` では起きない（dispatch-hook 未解決 1）。試験は「違反なし」だけを確かめている。
4. compare の対象は `route_select_one` の最初の列だけで、frontier 列を前置きした後の候補列・review_spawn・cos の判定は比べない（付記 S2-11）。
5. `rejected` の行を作らないので、S2-8 の集計に拒否の件数は入らない（通知 `script_bundle_rejected` だけ）。
6. thread-census が直した flaky について、dispatch-hook が起票した follow-up task は重複である。task の id と状態は未確認で、人か CoS が取り消す。
7. egress の迷い込み接続の出所は未確認（egress-stray）。
8. `cargo clippy -p celeris --tests -- -D warnings` は、S2 が触っていない `campaign_lock_e2e` の `duplicate_mod` で落ちる（必須の検査は `--tests` なしで通る）。

## 提案（別 task の候補）
現行の bug 候補と挙動の観察。どれも S2 では直していない。
1. release gate と統合の check で `control-ts` の bundle を作り、`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で本物の host の試験を走らせる（host-adapter・route-prio・stale-dist の提案）。
2. lane が `None` の run は `unwrap_or_default` で空文字 `""` の lane 行を引く。config が `""` の lane を許すと、lane の無い run が意図せずその行を使う。config の読み込みで空文字の lane を拒むか、`None` を「lane 行を引かない」にする（route-prio core）。
3. 空の宣言の列（class・lane・cos）は下の列へ落ちず `NoDeclaredDefault` で止まる。harness の `route_defaults` の空配列と未宣言も区別できない。誤設定で run が止まるので、config の読み込みで空の列を拒むかを決める（route-prio core）。
4. `route_select` が全 harness で断ったとき、`RouteTrace.harness` は最後に試した harness、`RouteTrace.candidates` は空のまま残る。bug かは未確認（route-prio hook）。
5. 入出力の型を Rust の DTO（`LaneRulesInput`・`LaneRulesOutput`・`RoutePriorityInput`・`RoutePriorityOutput`）から `docs/api/script-host/host-v1.schema.json` 経由で生成する（未解決 1）。
   あわせて「Rust が出す `RoutePriorityInput` は常に `lane`・`execution_class` の欄を持つ」ことを試験で固定する（route-prio ts）。
6. `campaign_lock_e2e` の `--tests` clippy の失敗を直す（未解決 8）。
7. `ops_r1_trace` の schema 版の固定は migration を足すたびに直す必要がある。読む表・欄の存在を確かめる検査に替えるかを campaign 側で決める（compare-store）。
8. `ScriptDomainStore` の 3 method を `TaskStore` に移す（dispatch-hook 提案 2）。
9. S5 で state の遷移を足すとき、`rejected` の行の作成と `loaded` への戻しの store 関数を作る（host-adapter 提案 2）。

## 本番への移行
S2 は本番 config を書き換えていない。本番の mode は全領域 `rust` のままで、`[script]` が無ければ TS を呼ばない。
compare を本番で有効にするのは人の手順で、既存の認可手順（release → verify → 人の承認で promote）に従う。
**IEEE Access・K2・K4 の凍結環境と測定用 release には入れない。** 測定の daemon では `[script]` を書かず、mode を変えない（境界 ADR (g)）。

1. migration 0103 を含む release を作る
   - `scripts/selfdeploy/release.sh <S2 を含む main の commit>`。migration 0103（`script_generations`・`script_domain_modes`・`script_domain_events`）は promote 後の起動で当たる。
   - S1 の「本番への移行」の V8 archive の確認（`RUSTY_V8_ARCHIVE`）が済んでいること。
   - 既存の手順で verify し、人が承認して promote する。
2. control-ts の bundle を作り、state_dir に配備する（release と同じ commit の checkout で）
   - `cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile && corepack pnpm@12.6.0 run bundle`
   - `node scripts/compare.mjs lane_rules` と `node scripts/compare.mjs route_priority` が全件 matched であることを確かめる。
   - `cd .. && cargo run --release -p celeris-script --bin script-deploy -- --dist control-ts/dist --state-dir <[script] state_dir>`
     → stdout の 1 行が `bundle_sha256`。`<state_dir>/script-bundles/<bundle_sha256>/` ができる。
3. config の `[script]` で active bundle を指す
   ```toml
   [script]
   state_dir = "<state_dir>"
   active_bundle_sha256 = "<2. の bundle_sha256>"
   ```
   `[script]` は起動時に読むので、daemon を既存の手順で再起動する。起動の log と通知に `script_bundle_rejected` が無いことを確かめる。
4. mode を compare にする
   - `celerisctl script domains` で `lane_rules` の `version` を読む（初めてなら 0）。
   - `celerisctl script mode lane_rules compare --expected-version <version> --generation <bundle_sha256> --mode-reason "S2 compare を本番で有効にする"`
   - 2 つ目の領域も比べるなら同じ手順で `route_priority` を compare にする（`lane_rules` の結果を見てから）。
5. 確かめ方
   - `celerisctl script domains` で mode が `compare`、`bundle_sha256` が 2. の値。
   - 本番 DB を読み取り専用で開き、記録の件数と outcome を数える:
     `sqlite3 -readonly <本番 DB> "SELECT json_extract(json,'$.domain'), json_extract(json,'$.outcome'), count(*) FROM events WHERE json_extract(json,'$.type')='script_compare_recorded' GROUP BY 1,2"`
   - 期待: dispatch のたびに件数が増え、outcome は `match` だけ。`mismatch`・`invalid_output` が 1 件でもあれば、または `safety_violations` が空でない記録
     （`… AND json_extract(json,'$.safety_violations') != '[]'`）があれば、6. で戻して記録を別 task にする。`unavailable` は bundle と generation の食い違いを示す。
6. 戻し方
   - `celerisctl script mode lane_rules rust --expected-version <現在の version>`（`route_priority` も同じ）。戻した後は TS を呼ばない。
   - `[script]` を消して再起動しても、daemon が起動時に `rust` へ戻す（`system:script-startup`）。
   - mode の変更の履歴は表 `script_domain_events`（追記専用）に残る。
