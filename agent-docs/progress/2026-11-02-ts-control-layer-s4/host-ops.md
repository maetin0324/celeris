---
title: S4 host-ops — Host API に cos_digest の async 読み出し op 2 つ・capability・要求ごとの分離
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 host-ops — Host API に cos_digest の async 読み出し op 2 つ

完了日: 2026-11-02（unit host-ops）。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S4-12・S4-13・S4-18・S4-19 の host-ops の行を実装した。
`PolicyScript` の trait（task-dispatch）は変えていない。

## 結論
- `crates/script-runtime` に async op `op_cos_digest_list_runs`・`op_cos_digest_read_run`（JS では `cosDigestListRuns`・`cosDigestReadRun`）を足した。Host API は 1.1.0。
- op は store を持たない。引数を DTO で検証し、要求ごとに Rust 側が渡す host sink（`ScriptRuntimeHandle::call_with_host_sink(call, Some(sink))`、容量 64 の bounded `mpsc`、応答は要求ごとの `oneshot`）へ送って await する。応答も DTO で検証してから script に渡す。
- 許可表 `DOMAIN_CAPABILITIES` に `("cos_digest", [cos_digest:list_runs, cos_digest:read_run])` を足した。`lane_rules`・`route_priority` は空のまま。
- 要求ごとの分離は S4-13 のとおり V8 context を分けず、次で実装した。
  1. 排他: capability を持つ要求は isolate の中で同時に 1 件まで。actor は後から来た capability 付きの要求を `held` に置き、前の要求が終わってから来た順に始める。capability の無い要求は並行して走る。
  2. domain への結び付け: grant に要求の `domain` を持たせ、op は `Capability::domain()` と一致する grant でだけ通る。
  3. 失効: `GrantGuard` が落ちる（返事・deadline・cancel・generation の作り直し）と token を消し、待っている op に終わりを知らせる。
- await 中の error（S4-18）: deadline は `timeout`、要求の終わり（cancel・返事の後）は `terminated`、終わった要求の token を後から使うと `denied`、generation の作り直しは呼び出しが `GenerationGone` になり op の応答口が閉じる。1 要求の op は 64 回まで（`MAX_OPS_PER_REQUEST`）で、65 回目は `busy`。
- `docs/api/script-host/host-v1.schema.json` に DTO（`CosDigestListRunsRequest`・`CosDigestListRunsReply`・`CosDigestRunRef`・`CosDigestReadRunRequest`・`CosDigestInput`・`CosTriageItemReport`・`CosTriageDecision`・`CosTriageOption`）と capability 2 つを足し、`control-ts/src/host.d.ts` を `gen-host-types.mjs` で再生成した。

## 付記との違い・実装で決めたこと
- **読み出し口の形**: objective は「trait object」と書くが、付記 S4-12 は「bounded mpsc の host sink、応答は oneshot」と決めている。付記に従い `HostSink = mpsc::Sender<HostOpCall>` にした。理由: op の future は V8 の thread の current-thread runtime で動くので、trait object の async fn をそこで await すると store の読み出しが V8 の thread で走る（付記の「store を V8 の thread に渡さない」に反する）。
- **DTO の置き場所**: 付記は「DTO の正本は task-core の型」と書く。`celeris-script` は task-core に依存しない（付記 S4-12）ので、同じ形の型を `crates/script-runtime/src/dto.rs` に持ち、schema はここから出す。task-core は dev-dependency にだけ足し、`tests/dto.rs::cos_digest_input_matches_task_core` が fixture `control-ts/fixtures/cos_digest/cases.jsonl` の全行を両方の型で往復させて直列化の byte 一致と欄の名前の一致を確かめる。
- **schema の nullable**: 既存の `#[schemars(required)]` は `Option` の `null` を schema から落としていた（`SelftestEchoRequest.note` が `string` になっていた）。container の transform `require_all_properties` に替え、`Option` の欄は `string | null` で required になった。`HostResult<T>` の schema 名は `{T}Result`（`SelftestEchoReplyResult` など）。
- **schema の hash**: schema が変わったので `crates/script-runtime/tests/fixtures/selftest-bundle/manifest.json` の `schema_sha256` を直した。control-ts の bundle は build 時に schema から hash を取るので、作り直せば合う。
- **deadline と終わりの知らせ**: actor は deadline で要求を終わらせるので、op の中で両方が同時に起きる。deadline を過ぎていれば `timeout` を優先して返す。
- **control-ts の試験 stub**: `control-ts/src/testing/host-stub.ts` に `cosDigestListRuns`・`cosDigestReadRun` と差し替え用の `setCosDigestListRuns`・`setCosDigestReadRun` を足した（`domains.test.ts` の型検査が host の export 全部を要求するため）。pass replay runner は ts-pass unit が書く。
- host sink は `ScriptCall` の欄にせず、`call_with_host_sink` の引数にした（attempt 2）。`ScriptCall` を変えないので `crates/celeris/src/daemon/script_policy.rs` などの既存の呼び出し側は無変更。`call` は sink 無しの `call_with_host_sink`。

## 試験（S4-19 の名前のまま。`crates/script-runtime/tests/cos_digest_ops.rs`）
| 試験 | 確かめること |
|---|---|
| `cos_digest_ops_denied_without_manifest_declaration` | 許可表にあっても manifest が宣言しなければ `denied`。sink に何も届かない |
| `cos_digest_ops_forward_to_host_sink_and_return_reply` | list→read の pass、request_id、op の error の伝達、引数の形の誤りは `invalid_output`（sink に送らない）、応答の形の誤りは `exception`、sink 無しは `exception` |
| `cos_digest_ops_grant_bound_to_domain` | `lane_rules`・`route_priority`・`host_selftest` の要求からは `denied` |
| `cos_digest_ops_terminated_after_request_end` | 返事の後に待っている op は `terminated` で応答口を閉じる。終わった token は `denied`。cancel（future の破棄）でも応答口を閉じる |
| `cos_digest_ops_timeout_at_deadline` | 応答の無い op で呼び出しは `Timeout`、op は応答口を閉じる。後の要求は通る |
| `cos_digest_ops_generation_gone_on_rebuild` | restart で `GenerationGone`、古い op の応答口は閉じ、新しい generation の要求は通る |
| `capability_requests_are_exclusive_per_isolate` | capability 付きの 2 件目は 1 件目が終わるまで始まらない。capability の無い要求は並行して通る |
| `cos_digest_ops_busy_after_64_calls` | 65 回目は `busy`、sink が受けたのは 64 回 |

他に `tests/dto.rs` の `cos_digest_input_matches_task_core`・`cos_digest_schema_nullable_and_required`、`src/ops.rs` の `domain_table_matches_capability_domain`。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p celeris-script`: 51 passed, 0 skipped（exit 0）。`--test cos_digest_ops --test dto` を 10 回続けて 15 passed ×10。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `cd control-ts && pnpm typecheck && pnpm test`: tsc exit 0、vitest 81 passed、node --test 34 passed。`node scripts/gen-host-types.mjs --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。Cargo.lock の変更で workspace 全体が対象になり、nextest 7105 passed・21 skipped、web vitest 697 passed、control-ts vitest 81 passed。

## 未解決事項
- 本番に deploy 済みの bundle は schema の hash が変わるので、新しい binary では load の照合で合わない。`cos_digest` は本番で `rust` のままなので挙動は変わらないが、bundle は作り直して deploy する必要がある（本番への移行の節）。
- 付記 S4-13 のとおり、capability の無い要求（純粋判定）が走っている `cos_digest` の要求の token を global から読めば、その要求の scope で op を呼べる。scope は広がらない（同じ pass の scope だけ）。要求ごとに isolate を作り直す方式は S5 以降で見直す。

- 計画の範囲 check（attempt 1 で不合格）は `control-ts/src/testing/host-stub.ts` を許していない。`host.d.ts` に op を足すと `domains.test.ts` の型検査（stub が `celeris:host` の export 全部を持つこと）が stub の追記を要求するので、stub を戻すと control-ts の tsc が TS2739 で落ちる。plan_issue として範囲に足すよう申告した。

## 本番への移行
- celeris の新しい binary を入れる前後で、`control-ts` の bundle を作り直して deploy する（`pnpm bundle` → `script-deploy`）。作り直さなければ起動時の復元で `system:script-startup` が各領域を `rust` へ戻す。本番の `cos_digest` は `rust` のままなので挙動は変わらない。

## 提案
- pass-wire unit は、op server の応答を `celeris_script::dto` の型ではなく task-core の型で組み、`serde_json::to_value` で sink へ返す。形の一致は `tests/dto.rs` が見張る。
