---
title: S3 runner — 検証専用の本物の host script-host-runner
tasks: [01M6EVP95S9VM3DBMZFRD8FS35]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S3 runner — 検証専用の本物の host script-host-runner

完了日: 2026-11-02（task 01M6EVP95S9VM3DBMZFRD8FS35、unit runner。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- [ADR 2026-11-02-ts-fast-verify](../../adr/2026-11-02-ts-fast-verify.md) (3) に沿って `crates/script-runtime/src/bin/script-host-runner.rs`
  （package `celeris-script`、`Cargo.toml` に `[[bin]]` を 1 件追加）を作った。依存 crate は足していない（`Cargo.lock` は不変）。
  既存の `manifest`（`load_bundle_dir`・`deploy_bundle`・`load_deployed`・`host_schema_sha256`）、`selftest`、`ScriptRuntimeHandle`、
  `domain_capabilities` だけを使う。daemon・celerisctl からは呼ばない。
- 子 command: `run`（`--suite golden|safety|all`、`--domain`、`--call-timeout-ms` 既定 5000）、`activate`、`domains`。
  出力は stdout に JSON 1 個（schema `celeris.script-host-runner/1`）。exit code は 0 / 1（採点の失敗）/ 2（使い方・入出力）/ 3（bundle の拒否）。
- 採点の表（Rust の `DOMAINS`）: `lane_rules`（`lane_rules/all-features.jsonl`、最少 19,683 行、入力 `{"features": row.features}`、
  欄 `rule_id`・`proposed`・`condition`）と `route_priority`（`route_priority/cases.jsonl`、最少 2,026 行、入力 `row.input`、欄 `basis`・`candidates`）。
- 判定: `match`・`mismatch`・`undeclared_field`・`invalid_output`・`exception`・`timeout`（`Timeout`・`Terminated`・`HeapLimit`）・`denied`・
  `order_dependent`（正順と逆順の 2 回で結果が違う）・安全違反 `undeclared_candidate`、行数不足 `too_few_rows`。
- 安全シナリオ: manifest の `domains` = 採点の表 ∪ `{host_selftest}`、`capabilities` ⊆ 各領域の許可表の和（外れたら exit 3）。
  壊した入力（`lane_rules` 7 件・`route_priority` 8 件）がすべて `exception` になること。純粋領域は空の scope で呼ぶ（op を使えば `denied`）。
- 試験 `crates/script-runtime/tests/script_host_runner.rs` は試験の中で小さな bundle と表の形の fixture を tempdir に書く
  （`control-ts/dist`・`control-ts/fixtures` を使わない）。一致・不一致・例外・宣言外の 4 場合と、bundle の拒否（領域の欠け → exit 3、
  入力を検査しない TS → 安全シナリオで exit 1）を固定した。
- `docs/architecture-map.md` の `celeris-script` の行に bin・試験・ADR (3)・不変条件を足した。

## ADR (3) からの細部の決め（ADR に書いていない点）
- `route_priority` の候補は `{source, model}` の順に組み直してから比べる。workspace の `serde_json` は deno_core のため
  `preserve_order` が有効で、TS が `{model, source}` の順で返すと byte が変わる。daemon の `classify_route_priority` は DTO に
  deserialize し直して比べるので順を見ない。runner も同じ扱いにした（欄の過不足・型は従来どおり `invalid_output`）。
- `Busy`・`GenerationGone`・`InputTooLarge` は ADR の分類に無いので、`host_error` の数を足した（失敗に数える）。
- 正順と逆順の比較は結果（出力の JSON か誤りの文）の一致で見る。数（`matched` など）は 1 回目の判定で数える。
- `activate` の probe は領域ごとの組み込みの入力 1 件を呼び、出力の形と値の検査だけをする（期待値とは比べない。golden の比較は `run` が受け持つ）。
- exit 2・3 以外の致命的な誤りは無い。exit 3 でも stdout に `{ok:false, exit, error}` の JSON を出す（exit 2 は stderr の usage だけ）。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
| 条件 | コマンド | 結果 |
|---|---|---|
| 本物の bundle（`$TMPDIR` に作り直した）で全件一致 | `node -e "…buildBundle({outDir:'$TMPDIR/rv/dist'})"` の後 `script-host-runner run --bundle-dir $TMPDIR/rv/dist --fixtures control-ts/fixtures`（debug build） | exit 0。`lane_rules` 19,683 行・`route_priority` 2,026 行とも 2 回で全一致、安全シナリオ 17 件すべて ok、13.2 秒（call p50 223 µs・295 µs） |
| activate | `script-host-runner activate --bundle-dir $TMPDIR/rv/dist --state-dir $TMPDIR/rv/state` | exit 0、selftest ok、2 領域とも `valid`、71 ms |
| 意図的な破壊を検出 | bundle の `"frontier/broad-judgment"` を書き換えて manifest の hash を合わせ、`run --suite golden --domain lane_rules` | exit 1、`mismatch` 972 件・`matched` 18,711 件、`complete: false` |
| 使い方・拒否 | `--fixtures` 無し / 無い bundle dir | exit 2 / exit 3 |
| 4 場合の試験 | `cargo nextest run -p celeris-script --test script_host_runner` | 5 tests run: 5 passed（8.5 秒） |
| 影響試験 | `bash scripts/dev/test-affected.sh`（対象 celeris・celeris-script・celerisctl） | exit 0、915 tests run: 915 passed |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0（警告は vendor の deno_core の既存 1 件だけで、`-D` の対象外の依存 crate） |
| fmt | `cargo fmt --all -- --check` | exit 0（1 回目に差分が出たので `cargo fmt --all` で直した） |
| 地図 | `python3 scripts/dev/check-architecture-map.py` | OK: 841 件のパス |

## commit
- `phase s3: runner: 検証専用の本物の host script-host-runner と 4 場合の試験`

## 未解決
- release build の runner での秒は測っていない（上の秒はすべて debug build）。cache の作り直しの秒と release の呼び出しの秒は verify-sh・bench unit が測る。
- runner の値の検査の上限（128・512・256 byte/件）は `task-dispatch::script_policy` の写し。一致を固定する試験は `crates/celeris/tests` に置く必要があり、
  この unit の範囲に無い（ADR「境界 ADR との関係」と同じ）。

## 提案
- `route_priority` の最少行数 2,026 は今の `dump-route-priority-fixtures` の出力の行数に合わせた。Rust 側で case を減らす変更をしたら、
  同じ変更で runner の `min_rows` を直す必要がある（runner は cache key に入るので作り直される）。
