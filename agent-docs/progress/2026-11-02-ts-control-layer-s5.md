---
title: S5 — generation 管理・段階切替・rollback（rust→compare→canary→ts）と停止基準の監視
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S5 — generation 管理・段階切替・rollback

完了日: 2026-11-03（S5 の段。task 01M6FPPJN7A8H427MXFVYFY7H4 は S5→S6 で、S6 は続く。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）。S6 の生データは `agent-docs/progress/2026-11-02-ts-control-layer-s6/` に置く。

## 結論
- 3 領域（`lane_rules`・`route_priority`・`cos_digest`）を、領域ごとに `rust`→`compare`→`canary`→`ts` の順に上げられるようにした。既定は `rust`。`canary`・`ts` へ上げられるのは人の API token の主体だけで、自動で上がる経路は無い。
- 新しい bundle は API で登録し、daemon が新しい isolate で検証してから `loaded` として記録する。mode の CAS が同じ transaction で generation を `active`・`draining` に移す。切替の前の要求は旧 generation で終わり、旧は in-flight 0 か 30 秒で `retired` になる。
- 起動時は mode の行が指す generation を新しい isolate で load し直して mode を保つ。使えない領域は `system:script-startup` で `rust` に戻す。
- script-guard が (f) の停止基準（数値は変えていない）で `canary`・`ts` を `rust` に戻す。安全違反は generation を `invalid` にし、検証の外の副作用は全領域を止める（`halted`。解くのは人だけ）。自動の戻しは受信箱の種類 `script_control` で人に知らせる。
- 本番の mode・config・deploy は変えていない。本番で上げる手順は下の「本番への移行」に書いた（人が実行する）。
- 実装と境界 ADR の突き合わせは付記 S5-13 に書いた。権限・安全・停止基準の規則に違いは無い。S5-9 の (1)(2) は実装に合わせて改めた。

## unit と記録
| unit | 記録 | 主な commit（件名） |
|---|---|---|
| adr-s5 | [adr-s5](2026-11-02-ts-control-layer-s5/adr-s5.md) | phase s5: adr-s5: 境界 ADR に S5 の generation・canary・drain・checkpoint・監視と S6 の測定の定義（付記 S5-1〜S5-12） |
| trace-read-fail | [trace-read-fail](2026-11-02-ts-control-layer-s5/trace-read-fail.md) | phase s5: trace-read-fail: cos_digest_trace に read_run が失敗する場面を足す |
| bundle-skip | [bundle-skip](2026-11-02-ts-control-layer-s5/bundle-skip.md) | phase s5: bundle-skip: 試験が control-ts の source から bundle を作り、dist を読まない |
| runtime-gen | [runtime-gen](2026-11-02-ts-control-layer-s5/runtime-gen.md) | phase s5: runtime-gen: generation の登録簿（新 isolate で検証・要求の固定・drain・30 秒の打ち切り） |
| store-gen | [store-gen](2026-11-02-ts-control-layer-s5/store-gen.md) | phase s5: store-gen: generation の状態・canary_bps・script_guard の計数を store に足す |
| core-fix | [core-fix](2026-11-02-ts-control-layer-s5/core-fix.md) | phase s5: core-fix: draining の generation を retired→再登録してから compare に戻す |
| api-canary | [api-canary](2026-11-02-ts-control-layer-s5/api-canary.md) | phase s5: api-canary: canary・人だけの昇格・generation の登録 API・guard の解除・CoS registry |
| script-inbox | [script-inbox](2026-11-02-ts-control-layer-s5/script-inbox.md) | phase s5: script-inbox: 受信箱の種類 script_control と event から項目を作る経路 |
| daemon-gen | [daemon-gen](2026-11-02-ts-control-layer-s5/daemon-gen.md) | phase s5: daemon-gen: generation の登録・切替後の drain・起動時の復元を daemon に結線し、active_bundle_sha256 を消す |
| canary-dispatch | [canary-dispatch](2026-11-02-ts-control-layer-s5/canary-dispatch.md) | phase s5: canary-dispatch: canary の選択・TS 側の検証後の適用・副作用後の照合・tick の予算を dispatcher に結線する |
| guard | [guard](2026-11-02-ts-control-layer-s5/guard.md) | phase s5: guard: 停止基準の見回り（検証の外の副作用で止める・進む条件を出すだけ）と (f) のしきい値の試験 |
| guard-fix | [guard-fix](2026-11-02-ts-control-layer-s5/guard-fix.md) | phase s5: guard-fix: guard の試験の記録に canary 2 欄を足し、canary の記録で止まる試験を足す |
| gen-e2e | [gen-e2e](2026-11-02-ts-control-layer-s5/gen-e2e.md) | phase s5: gen-e2e: script_generation_e2e（新旧併存→切替→途中 error→rollback→再起動）と試験の入口 |
| s5-close | この file | phase s5: s5-close: S5 の記録・付記 S5-13・SPEC・architecture-map・本番への移行 |

cos-ts-scenario unit（`edit-activate-baseline.sh` の `cos-ts`・`cos-digest`）は同じ段で並行して走っており、記録は統合（integrate-s5-verify）の後に S6 の記録から参照する。

## 受け入れ条件ごとの証拠
下の結果は s5-close unit が提出時の HEAD で実行したもの（基準は `$CELERIS_WU_BASE`）。各 unit の実行結果は unit の記録にある。

### task 条件 0: 新旧併存→切替→途中 error→rollback→再起動の e2e が本物の bundle で通り、event の欠落 0・意図しない二重副作用 0
- `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_generation_e2e --test script_generation_daemon` → exit 0、6 passed・0 skipped（`script_generation_e2e_switch_error_rollback_restart` 1 件と daemon の登録・切替・drain・30 秒の打ち切り 5 件）。bundle は試験の support が control-ts の source から `$TMPDIR` に作った物（付記 S5-9 を改めた S5-13）。
- e2e の流れと検査（`assert_ledger`）は [gen-e2e](2026-11-02-ts-control-layer-s5/gen-e2e.md): `script_domain_events` が予定の 11 件と一致し version が 1 ずつ増える、`script_generation_events` の遷移が A 2 周・B 1 周で一致する、run ごとの digest message が 1 件で冪等 key の重複 0、`script_compare_log` も run ごとに 1 件まで。gen-e2e unit は同じ試験を 40 回続けて合格させた。

### task 条件 1: 変更 crate と逆依存の試験・clippy・fmt
- `bash scripts/dev/test-affected.sh` → exit 0。s5-close は文書だけを変えたので「試験対象の変更なし（docs のみ等）」。文書の検査の self-test は通った。
- S5 のコードの試験は統合の段で走る。各 unit の最後の test-affected: daemon-gen 5070 passed・20 skipped（web vitest 697 passed）、gen-e2e 910 passed・0 skipped、guard-fix 909 passed。script bundle の SKIP はどれも 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

### task 条件 3: 文書の検査
- `bash scripts/dev/check-doc-links.sh` → ok。`bash scripts/dev/check-adr-numbers.sh` → ok。`sh scripts/dev/progress-index.sh --check` → ok。`python3 scripts/dev/check-architecture-map.py` → OK。結果の数は下の「検査（s5-close）」。

### task 条件 4: 昇格と rollback、停止基準による自動の戻し、起動時の復元、未承認の本番変更 0、認可 error の fail-open 0
| 確かめること | 実装 | 試験（s5-close が提出時の HEAD で実行） |
|---|---|---|
| `rust→compare→canary→ts` の昇格と戻し、`canary`・`ts` は人の token だけ、前の段を飛ばせない | `crates/task-api/src/script_domains.rs`、`crates/task-core/src/store/script_domains.rs` | `cargo nextest run -p task-api --test script_domains --test script_generations` → 16 passed（`script_domain_mode_canary_requires_human`・`…_canary_requires_guard_ready`・`…_ts_requires_same_generation_canary`・`…_rollback_by_anyone`・`…_refused_while_halted`、停止の解除は人だけ、generation の登録 503・422・409）。`cargo nextest run -p task-core -E 'test(/script_domain\|script_guard\|script_generation/)'` → 34 passed（`script_domain_promote_through_canary_to_ts_and_back`・`script_domain_stage_and_human_rules_change_nothing` ほか） |
| 停止基準（(f) の数値のまま）で自動で `rust` に戻す | `crates/task-core/src/{script_guard.rs,store/script_guard.rs}`、`crates/celeris/src/daemon/script_guard.rs` | `cargo nextest run -p celeris -E 'test(/script_guard\|script_inbox\|script_restore_reason/)'` → 15 passed（不一致 1 件・timeout 率と 10 分 3 件・失敗率と連続 3 件・安全違反・検証の外の副作用で全体停止・計数が再起動の後も残る・canary の記録を両側で数える・受信箱の項目が event ごとに 1 件）。e2e の途中 error で `guard_failure_streak` |
| 起動時の active generation の復元 | `crates/celeris/src/daemon/script_layer.rs` の `ScriptGenerations::restore` | e2e の再起動（同じ mode・generation・version を新しい isolate で復元、mode の記録も digest も増えない）、bundle を消した再起動で `startup_bundle_unusable`。`script_runtime_daemon_restore_*` は daemon-gen の記録（43 passed） |
| generation の固定・drain・30 秒・上限 2・in-flight 64 | `crates/script-runtime/src/generation.rs` | `cargo nextest run -p celeris-script --test generation` → 11 passed |
| canary の選択・副作用の前後・認可の拒否で fail-open しない・tick の予算 | `crates/task-dispatch/src/dispatcher/script_canary.rs` | `cargo nextest run -p task-dispatch -E 'test(/canary\|tick_budget\|rollback_write_failure/)'` → 10 passed（`canary_denied_never_fails_open`・`canary_effect_unverified_stops_and_rolls_back`・`rollback_write_failure_treated_as_rust` ほか） |
| 未承認の本番変更 0 | — | worker は本番の config・mode・release・DB を変えていない。e2e は operator の token・CoS の run credential・知らない token・`system:` の主体のどれでも `canary`・`ts` の表と記録が変わらないことを確かめる |

### task 条件 7 のうち S5 の分: 本番の変更は人の手順として「本番への移行」に書かれている
- 下の「本番への移行」。

### WU の条件
- 0（進捗に証拠と本番への移行）: この file。
- 1（ADR・SPEC・architecture-map が実装と一致）: 付記 S5-13 に突き合わせの表を書き、S5-9 の (1)(2) を改めた。`docs/SPEC.md` の §5 に段階の昇格・人だけの昇格・自動の戻し・Rust に残すものを 1 項で書いた。`docs/architecture-map.md` の「TS 制御層の結線」の行に S5 の module（`script_guard.rs`・`script_canary.rs`・task-core の `script_guard`・`script_inbox`・task-api の `script_generations.rs`・migration 0105・0106）と試験と不変条件を足し、S4 のままだった不変条件（`ts` は `cos_digest` だけ・`compare` から）を S5-4 に直した。`celeris-script` の行に `generation.rs` と `tests/generation.rs` を足した。
- 2（文書の検査・test-affected・clippy・fmt）: 下の「検査（s5-close）」。

## 検査（s5-close、提出時の HEAD）
- `bash scripts/dev/test-affected.sh` → exit 0（docs のみ）
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-doc-links.sh` → ok、`bash scripts/dev/check-adr-numbers.sh` → ok、`sh scripts/dev/progress-index.sh --check` → ok、`python3 scripts/dev/check-architecture-map.py` → OK（880 件）
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は境界 ADR・`docs/SPEC.md`・`docs/architecture-map.md`・この file の 4 つ。コードの差分なし

## 未解決
1. `celerisctl script mode` は `rust`・`compare`・`ts` だけを受ける。`canary`・`canary_bps`・generation の登録と退役・停止の解除の subcommand は無い。help の文面は S4 の「`ts` は `cos_digest` だけ・同じ generation の `compare` から」のままで、今の規則（`ts` は全領域・同じ generation の `canary` から）と違う（`crates/celerisctl/src/commands/script.rs` の 25 行目と 36 行目の注釈）。本番では canary と登録を人の token で API を直接呼んで行う（下の手順）。s5-close はコードを変えない。
2. 登録で bundle の file が無い hash も `invalid` になり、同じ hash では登録し直せない（daemon-gen の未解決 1）。`script-deploy` の前に登録すると、その hash は使えなくなる。手順では配備を必ず先にする。
3. drain の 30 秒は見回りが drain を始めた時刻から数え、CAS の時刻より最大 1 秒遅れる（S5-13 の (5)）。
4. daemon が 10 分ごとに出す `script_tick_wait` の log 行は無い（`Dispatcher::script_tick_wait_summary` はある）。S6 の overhead の測定は、この関数を呼ぶ試験か log 行を足す変更が要る。
5. `route_priority` の canary の選び直し（`priority_override`）を通す dispatcher の試験は無い。tick の予算は値の差し替えで確かめ、dispatcher の中で 50 ms の判定を 5 回待たせる試験は無い（canary-dispatch の未解決）。
6. script-guard の見回りは `script_compare_log` を毎回全件読む（guard の未解決 1）。表が大きくなると見回りが重くなる。
7. 検証の外の副作用の検出は `cos_digest` の記録の自己矛盾で行う。記録を書かずに起きた副作用は検出できない（S5-13 の (12)）。
8. 受信箱 `script_control` の項目は CoS の triage にも流れ、daemon がその項目から triage run を起こす（gen-e2e で確認）。CoS は `canary`・`ts` へ上げられず、項目に受信箱で答えられないので、権限は広がらない。人への知らせが CoS 経由で 1 回、受信箱で 1 回になる。この扱いでよいかは人の確認が要る。
9. runtime-gen の入口の確認は bundle の文面 `unknown domain: <領域>` に頼る（runtime-gen の未解決 1）。
10. `script-host-runner` の `pass_traces.jsonl` の行数の下限は 20 のままで、read_run の失敗の 2 行が消えても runner の採点では落ちない（trace-read-fail の未解決）。
11. release gate の全試験で bundle を使う試験が SKIP しうる（REQUIRE を立てていない。bundle-skip の未解決 1）。
12. 付記 S5-5 のとおり、checkpoint の表は無い。長期 workflow の引き継ぎは、作り物の版の集合による照合の試験だけで確かめている。
13. S6 の `cos.jsonl` の中身の `scenario` は `cos-digest` で、S0 の `cos`（`job_wait_extend`）と名前が同じ（付記 S5-10）。S6 の報告で明記する。

## 提案
1. `celerisctl script` に `canary`（`--canary-bps`）・`generations list|register|retire`・`guard clear` を足し、help の文面を S5-4 に直す（未解決 1。別 task）。
2. 登録で bundle の file が無いときは `invalid` にせず 422 だけ返す（未解決 2。store と daemon の変更なので別 task）。
3. `tick_loop.rs` に `script_tick_wait` の log 行を足す（未解決 4）。S6 の overhead の leaf の前に入れると、測定が log から読める。
4. `script_compare_log` を id で絞って読む store の関数を足し、見回りを差分の読みにする（未解決 6）。
5. runner の `pass_traces.jsonl` の下限を 22 に上げる（未解決 10）。
6. release gate でも bundle を作って `CELERIS_SCRIPT_BUNDLE_DIR` と REQUIRE を渡す（未解決 11。release gate の変更は別 task）。
7. cos-operator skill の `metadata.version`（policy_version、今 5）は上げない。api-canary が足したのは操作表の 2 行（`script_generation.register`・`script_generation.retire`）と mode_set の行の注記で、CoS の判断の規則は変わらない（2026-10-28 の review thread の変更と同じ扱い）。上げると config の既定・評価契約の追従が要る。

## 本番への移行
worker は本番を変えていない。下は人が実行する手順である。本番の mode は全領域 `rust` のままで、release を入れても TS は呼ばれない。上げるかどうか・いつ上げるかは人が決める。
**IEEE Access・K2・K4 の凍結環境と測定 release には入れない。** 測定の daemon の config に `[script]` を書かず、mode を変えない（境界 ADR (g)）。

記号: `<api>` は本番の API の origin（`[api] listen`）、`<tok>` は人の主体（`human:<name>`）の token の file、`<db>` は本番 DB、`<state_dir>` は `[script] state_dir`、`<kb_root>` は `[knowledge] root`。

### 1. release の前に config を直す
- 本番の config（`~/.config/celeris/` の下）に `[script] active_bundle_sha256` があれば、その行を消す。新しい binary は欄があると読み込みエラーで起動しない。`[script]` に残すのは `state_dir`（と、あれば `input_limit_bytes`・`heap_limit_bytes`）だけ。`[script]` が無ければ何もしない。
- 確かめ方: `grep -n active_bundle_sha256 ~/.config/celeris/*.toml` が何も出さない。

### 2. release・verify・promote
- 既存の手順で `scripts/selfdeploy/release.sh <S5 を含む main の commit>` → verify → 人の承認で promote。migration 0105（generation の状態・`script_generation_events`・`script_guard_state`・`canary_bps`）と 0106（`script_inbox_items`・`script_inbox_cursors`）は起動で当たる。
- 0105 は既存の `rejected` を `invalid`、mode の行が指す `loaded` を `active`、指されない `loaded` を `retired` に移す（actor `system:migration`）。本番は全領域 `rust` なので、generation の行があれば `retired` になる。
- 確かめ方: daemon が起動し、`sqlite3 -readonly <db> "SELECT domain, mode, generation_id FROM script_domain_modes"` が 3 行とも `rust`。

### 3. cos-operator skill の操作表を本番 KB に取り込む
- api-canary が `config/skills/cos-operator/operations.md` に `script_generation.register`・`script_generation.retire` の 2 行を足し、mode_set の行を S5-4（CoS は `canary`・`ts` にできない）に合わせた。`SOURCE.md` にも 1 行足した。`metadata.version` は 5 のままで、`[cos] policy_version` の書き換えは要らない。
- release と同じ commit の checkout で `celerisctl skills import config/skills --name cos-operator --root <kb_root>`（冪等）。
- 確かめ方: `grep -c script_generation <kb_root>/skills/cos-operator/operations.md` が 2 以上。web の新規スレッドで一往復して返事が返る。

### 4. bundle を作って配備し、generation を登録する（登録は人の token で API を呼ぶ）
- release と同じ commit の checkout で:
  - `cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile && corepack pnpm@12.6.0 run bundle && cd ..`
  - `cargo run --release -p celeris-script --bin script-deploy -- --dist control-ts/dist --state-dir <state_dir>` → stdout の 1 行が `bundle_sha256`（以下 `<sha>`）。**配備を登録より先にする**（file の無い hash を登録すると `invalid` になり、その hash は使えなくなる。未解決 2）。
- 登録: `curl -sS -X POST <api>/api/v1/script/generations -H "Authorization: Bearer $(cat <tok>)" -H 'Content-Type: application/json' -d '{"bundle_sha256":"<sha>"}'`
  - 200 で `state` が `loaded`。422 `script_generation_invalid` は検証の失敗（本文に理由）、409 `script_generation_limit` は生きた generation が 2 つある（不要な `loaded` を `POST <api>/api/v1/script/generations/<id>/retire` で退役させる）、503 `script_runtime_unavailable` は `[script]` が無いか standby の daemon。
  - celerisctl には登録の subcommand が無い（未解決 1）。登録は挙動を変えないので operator の token でもよいが、手順は人の token にそろえる。

### 5. compare にして記録を確かめる
- `celerisctl script domains` で領域の `version` を読み、`celerisctl script mode <domain> compare --expected-version <version> --generation <sha> --mode-reason "<理由>"`（compare は人・operator・CoS のどれでもよい）。
- 確かめ方（本番 DB は読み取り専用で開く）:
  - `lane_rules`・`route_priority`: `sqlite3 -readonly <db> "SELECT json_extract(json,'$.domain'), json_extract(json,'$.outcome'), count(*) FROM events WHERE json_extract(json,'$.type')='script_compare_recorded' GROUP BY 1,2"`
  - `cos_digest`: `sqlite3 -readonly <db> "SELECT json_extract(json,'$.mode'), json_extract(json,'$.outcome'), json_extract(json,'$.applied_by'), count(*) FROM script_compare_log WHERE domain='cos_digest' GROUP BY 1,2,3"`
  - 期待: `outcome` は `match`（と分母に入らない `unavailable`）だけ、`applied_by = rust`。`mismatch` が 1 件でもあると script-guard が `clean_streak` を 0 にし、canary の前提が満たせない。

### 6. canary にする（人の承認の後に、人が自分の token で API を呼ぶ）
- 前提は script-guard の計数が (f) の「進む条件」を満たすこと（純粋判定 1000 件、CoS の経路 200 件。足りなければ 422 `script_canary_not_ready` で、本文の `extra.readiness` に計数が出る）。
- `curl -sS -X PUT <api>/api/v1/script/domains/<domain>/mode -H "Authorization: Bearer $(cat <tok>)" -H 'Content-Type: application/json' -d '{"mode":"canary","expected_version":<version>,"generation":"<sha>","canary_bps":<1〜9999>,"reason":"<承認の記録>"}'`
  - operator・CoS・`system:*` の主体は 403 `script_mode_requires_human`。元は同じ generation の `compare` だけ（他は 422 `script_mode_requires_previous_stage`）。`canary_bps` の変更も同じ本文で行う（人の token だけ）。
- 確かめ方: `celerisctl script domains` で mode が `canary`。記録の `canary_selected = true` の行で `applied_by = ts`・`fallback` なしの行が増える（`cos_digest` は `script_compare_log` の `json_extract(json,'$.canary_selected')`）。

### 7. ts にする（人の承認の後に、人が自分の token で API を呼ぶ）
- 前提は canary の間の TS 側の単位の計数が同じ閾値を満たすこと。
- `curl` の本文を `{"mode":"ts","expected_version":<version>,"generation":"<sha>","reason":"<承認の記録>"}` にして 6. と同じく呼ぶ。元は同じ generation の `canary` だけ。
- celerisctl でも `CELERIS_API_PRINCIPAL=human:<name> celerisctl script mode <domain> ts --expected-version <version> --generation <sha> --mode-reason "<承認の記録>"` で送れる（環境変数が無いと operator の token を使い 403 になる）。help の文面は古い（未解決 1）。

### 8. 監視の見方
- **受信箱 `script_control`**: 自動の戻し（`system:script-guard`）・起動時の戻し（`system:script-startup`）・generation の無効化・全体の停止のたびに項目が 1 件出る。本文は領域・generation・変更（例 `ts → rust`）・`reason` の先頭の code（`guard_mismatch`・`guard_timeout_rate`・`guard_timeout_burst`・`guard_failure_rate`・`guard_failure_streak`・`guard_safety_violation`・`guard_unvalidated_effect`・`script_effect_unverified`・`startup_bundle_unusable` ほか）と全文。受信箱では答えられない（answer は 409 `native_action_required`）。web の案内に従って daemon の画面と API で対応し、人がその領域の mode を変える（戻す・上げ直す）と項目は閉じる。この項目は CoS の triage にも流れる（未解決 8）。
- **mode と generation**: `celerisctl script domains`、`curl -sS <api>/api/v1/script/generations -H "Authorization: Bearer $(cat <tok>)"`（`state` が `loaded`・`active`・`draining`・`retired`・`invalid`）。全体の停止の有無は `GET <api>/api/v1/script/domains` の `guard_halted`。
- **履歴**: `sqlite3 -readonly <db> "SELECT id, ts, json FROM script_domain_events ORDER BY id DESC LIMIT 20"` と `script_generation_events`（どちらも追記専用）。計数は `script_guard_state`。
- **全体の停止**: `script_guard_state` の `domain = '*'` の行の `halted`（1 が停止）。止まっている間は `rust` 以外への変更が 409 `script_guard_halted`。原因（受信箱の項目の reason）を確かめてから、人が `curl -sS -X POST <api>/api/v1/script/guard/clear -H "Authorization: Bearer $(cat <tok>)" -H 'Content-Type: application/json' -d '{"reason":"<確認の記録>"}'` で解く。

### 9. rollback
- 戻しは誰でもできる: `celerisctl script mode <domain> rust --expected-version <version>`（または `compare`）。戻した後は TS を呼ばない（`compare` は影だけ）。
- 前の bundle（known-good）に戻すときは、その `<sha>` を 4. の登録で登録し直し（`retired` は `loaded` に戻る。`invalid` は戻らない）、5.→6.→7. を通し直す。自動で前の TS の generation へ戻す経路は無い。
- release ごと戻すときは既存の rollback の手順。migration 0105・0106 は列と表を足す形で、旧 binary は新しい表を読まない（旧 binary で起動したときの挙動は未確認）。
- Celeris は `<state_dir>/script-bundles/` の file を消さない。消すのは人の判断だけ。

### 10. 確認
- 全領域を `rust` に戻した状態で、`script_domain_modes` の 3 行が `rust`、`script_guard_state` の `*` の `halted` が立っていない、受信箱の `script_control` の未処理が 0 件。
