---
title: S1 host-api — Host API celeris:host v1（native op・capability/scope 検査・DTO と JSON Schema・import 拒否）
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
status: done
updated: 2026-11-02
---
# S1 host-api — Host API `celeris:host` v1

完了日: 2026-11-02

## 結論
- `Math.random`・`Date.now`・引数なしの `Date`（`new Date()`・`Date()`・`Date` の subclass の引数なし構築）は、呼ぶと `TypeError` を throw させる。
  seed での置き換えはしない。現在時刻は入力 DTO の `Timestamp` で渡す（`new Date(input.now)` は使える）。`performance` は消す。
  理由: (1) 置き換えると全入力 DTO に seed と時刻の欄が要り、渡し忘れても黙って同じ値で動く。throw なら渡し忘れが試験ですぐ落ちる。
  (2) S2 の `compare` mode で Rust と TS の結果を比べるには、TS が隠れた入力（時計・乱数）を持たないことが要る。
  確かめた試験: `runtime_clock_random_policy`。
- bundle は main ES module として load する（旧来の classic script の評価は廃止）。import してよいのは bundle 自身と `celeris:host` だけで、
  静的・動的の import ともに loader の resolve で拒否する（`runtime_import_rejected`。`ext:` は deno_core の module map が先に拒否する）。
- 権限の境界は Rust の op。bootstrap の JS での隠蔽（`Deno`・`__bootstrap` の削除）は補助で、境界にしない。

## 変更
- `phase 1: host-api: celeris:host v1 の native op・capability/scope 検査・DTO と JSON Schema・import 拒否`
  - `crates/script-runtime/src/dto.rs`（新規）: DTO（`Capability`・`U64String`・`Timestamp`・`HostError{code,message}`・`HostErrorCode`・
    `HostResult<T>`・試験用 op の `SelftestEcho*`）と `host_api_schema_json()`。`HostError` → `ScriptError` の写像（`From`）。
  - `src/ops.rs`（新規）: `op_host_version`（同期・capability 不要）と試験用 async op `op_host_selftest_echo`。
    領域ごとの許可表 `domain_capabilities`（今は `host_selftest` → `selftest:echo_task` だけ）。
    actor が要求ごとに `Grant`（manifest の宣言 ∩ 許可表、scope の task id）を要求 token（要求 ID の ULID）で登録し、op が照合する。
    要求が終わると `GrantGuard` の drop で消えるので、終わった要求の token で呼んでも `denied`。async op は待った後にもう一度照合する。
    deno_core の組み込み op のうち resource の読み書き・`op_print`・`op_eval_context`・`op_import_sync` 等を middleware で無効にする（`DISABLED_BUILTIN_OPS`）。
  - `src/loader.rs`（新規）: `BundleLoader`。`src/host.js`（新規、静的）: `celeris:host` の本文（op の結線と `HostError`）。
  - `src/bootstrap.js`（新規、静的）: host の op を 1 回限りの関数で `celeris:host` へ渡し、時計・乱数を throw にし、`Deno`・`__bootstrap` を消す。
  - `src/actor.rs`: `runtime_options()`（loader・extension・`inspector: false`）、bootstrap → bundle の module load・評価（10 秒の watchdog 付き）、
    入口を `__celerisDispatch(domain, input, ctx)` で呼ぶ（`ctx = { token }`）、`HostError` の例外を `ScriptError` の種類へ写す。
  - `src/handle.rs`: `ScriptSource.capabilities`、`ScriptCall.scope_task_ids`、`inspector_enabled()`・`op_names()`。
  - `src/bin/script-schema.rs`（新規）と生成物 `docs/api/script-host/host-v1.schema.json`。
  - `tests/runtime.rs`: `runtime_denied_capability`・`runtime_denied_scope`・`runtime_import_rejected`・`runtime_no_ambient_io`・
    `runtime_clock_random_policy`・`runtime_no_inspector`・`runtime_float_exact` を追加。既存の試験は新しい欄を足しただけ。
  - `tests/dto.rs`（新規）: `dto_null_required`・`dto_bigint_as_string`・`dto_unknown_enum_rejected`・`dto_unknown_field_rejected`・
    `dto_schema_matches_committed`（`script-schema` の出力と生成物の byte 一致）。

## 値の写像表（実装と試験で確かめた形）
| Rust | JS/TS | 規則 | 試験 |
|---|---|---|---|
| ID（ULID） | `string` | 26 文字のまま | `runtime_denied_scope` |
| `U64String`（2^53−1 を超えうる整数） | `string` | 10 進・先頭 0 なし。number・`-1`・`1e3`・2^64 以上は拒否。schema は `pattern` | `dto_bigint_as_string` |
| `Option<T>` | `T \| null` | 欄は常に出す。省いたら拒否（`deserialize_with = "Option::deserialize"`、schema の `required` に入る） | `dto_null_required` |
| unit の enum | snake_case の文字列 | 未知の値・大文字は拒否 | `dto_unknown_enum_rejected` |
| データ付きの enum | `{ kind: "…", … }` | `tag = "kind"` | `dto_unknown_enum_rejected` |
| `Timestamp` | `string` | RFC 3339、UTC、末尾 `Z`。`+00:00`・`+09:00` は拒否 | `dto_bigint_as_string` |
| 全 struct | object | `deny_unknown_fields` | `dto_unknown_field_rejected`・`runtime_denied_scope` |
| `HostError{code,message}` | `HostError`（`Error` の subclass、`code`） | code は `ScriptError` の 7 種と 1 対 1。script が catch しなければ actor が `ScriptError` へ写す | `runtime_denied_capability`・`dto_unknown_enum_rejected` |
| `f64` | `number` | serde_v8 で渡し、serde_json の文字列 parse を通さない。0.9999999999999999 は bit 単位で往復 | `runtime_float_exact` |
| JS の整数（32 bit を超える） | `number` | serde_v8 は `serde_json::Value` に **f64** の number として入れる（`1793577600000.0`）。`Value` 経由で `u64`・`i64` の欄へ `from_value` すると型の誤りで落ちる。整数の欄は `U64String` にするか、`serde_v8::from_v8::<T>` で型付きに直接 deserialize する | `runtime_clock_random_policy` |

## float_roundtrip
- 写しの deno_core から `float_roundtrip` を外した状態（crate-skeleton で決めた形）で、JS と Rust の間の f64 は serde_v8 の
  `to_v8`/`from_v8` だけを通り、文字列の parse を通らない。`runtime_float_exact` で 0.9999999999999999（bit `0x3FEFFFFFFFFFFFFF`）が
  入力として渡って戻る値・JS の literal の値の両方で bit 一致した。op の引数も serde_v8 で `serde_json::Value` にしてから `from_value` で検証する（文字列にしない）。

## 証拠（提出時の HEAD で実行）
- `cargo test -p celeris-script`: exit 0。runtime 19 passed、dto 5 passed、unit 1 passed。
- `cargo run -q -p celeris-script --bin script-schema | cmp - docs/api/script-host/host-v1.schema.json`: exit 0（差分なし）。
- `bash scripts/dev/test-affected.sh`: exit 0。Cargo.lock の変更で全 crate が対象。nextest 6942 passed・21 skipped、web の型検査と unit 697 passed。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 範囲 check と Cargo.lock（解決済み）
- 計画の範囲 check が当初 `Cargo.lock` を範囲外としていた。Objective が求める schemars を `celeris-script` の依存に足すと `Cargo.lock` の依存一覧が必ず変わる
  （差分は 3 行で、新しい外部 crate は入らない）ので plan_issue で申告し、replan で check が直った。コードは作り直していない。

## 未解決事項
- この WU の `CARGO_TARGET_DIR` の `debug/build/v8-*/output` が別の WU の target dir（seed 元）の `gn_out/obj` を指しており、
  初回の build が `could not find native static library rusty_v8` で落ちた。自分の target dir の `debug/build/v8-*` と
  `.fingerprint/v8-*` を消して build script を再実行させると直った。target の seed（reflink）を使う WU では同じことが起きうる。
- `runtime_no_inspector` は actor が JsRuntime に渡した設定値（`inspector: false`）を handle から読む形で、V8 の inspector の有無を
  deno_core の公開 API から直接は確かめられない（`has_inspector` は crate 内だけ）。
- 許可表は Rust の定数で、今は試験用の `host_selftest` 領域だけ。実際の領域と capability（例 `read:task_snapshot`）は S2 以降で足す。
- 同じ isolate で並行する要求どうしは、bundle が他の要求の token を global に保存すれば相手の scope で op を呼べる（同じ bundle の中の話で、
  要求が終われば token は無効になる）。要求ごとに V8 context を分けるかは S2 で領域の実装を足すときに決める。

## 提案
- bundle-load unit で manifest の `capabilities` を `Capability` で deserialize すれば、未知の capability を load の段で拒否できる。
