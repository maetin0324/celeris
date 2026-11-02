---
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
---
# ADR 2026-11-02: deno_core の写しを repo に置き、serde_json の挙動を変える feature を外す

- 状態: 採用。人の決定 `serde-json-unify` の答え「patch」（2026-11-02、task 01M6DJ6QCMCWT80MTYJ7SNE0XK）に従う。
- 関係: [TS 制御層の境界 ADR](2026-11-02-ts-control-layer-boundary.md) (b)(e)、
  [破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)、
  進捗 [crate-skeleton](../progress/2026-11-02-ts-control-layer-s1/crate-skeleton.md)。

## 結論
- crates.io の `deno_core` 0.413.0 を `crates/script-runtime/vendor/deno_core/` に展開して置き、root の `Cargo.toml` の
  `[patch.crates-io]` でこの写しを使う。写しで変えるのは `Cargo.toml` の `[dependencies.serde_json]` の features だけで、
  `float_roundtrip` と `preserve_order` を外し、`raw_value` は残す。
- 写しの元の版と `.crate` の sha256、変えた行は `crates/script-runtime/vendor/deno_core.lock.toml` に記録する。
  写しは `crates/script-runtime/vendor/vendor-deno-core.sh` で機械的に作り直す（`--check` で repo の写しと一致するかを確かめる）。
- 写しは workspace の member にしない（root の `exclude` に `crates/script-runtime/vendor` を足す）。

## 背景
deno_core 0.413.0 は `serde_json` を optional でない依存として `raw_value`・`float_roundtrip`・`preserve_order` 付きで使う。
cargo は feature を graph 全体で合わせるので、deno_core を workspace に入れると全 crate の serde_json が変わる。
- `preserve_order`: `serde_json::Map` が BTreeMap（key の辞書順）から IndexMap（挿入順）になる。直列化の key 順が変わり、
  並べ直さずに sha256 を取る所（監査した 10 か所のうち 4 か所）で値が変わる。`ApiProblem` が 144 byte になり
  `cargo clippy --workspace -- -D warnings` が task-api の `result_large_err` 851 件で落ちる。
- `float_roundtrip`: 文字列から f64 への parse の丸めが変わる（より正確な、遅い経路になる）。数値の値が変わりうる。
- `raw_value`: `RawValue` の API が増えるだけで既存の挙動は変わらない（axum も有効にしている）。

親 task の制約「言語移行と挙動・方針の変更を同じ差分に混ぜない」により、S1 で既存の JSON の挙動を変えない。

## 決定
### D1 写しの置き場所と形
- 置き場所は `crates/script-runtime/vendor/deno_core/`。deno_core を使うのは `celeris-script` だけ（境界 ADR (b)）なので、
  その crate の下に置く。
- 中身は crates.io の `deno_core-0.413.0.crate`（tar.gz）を展開したものそのまま。`Cargo.toml` 以外の file は変えない。
  `Cargo.toml.orig`・`Cargo.lock`・`.cargo_vcs_info.json` も crate に入っている形のまま残す。
- root の `Cargo.toml`:
  - `[patch.crates-io] deno_core = { path = "crates/script-runtime/vendor/deno_core" }`
  - `exclude` に `crates/script-runtime/vendor`。path 依存が workspace の下にあると cargo は member に数えるため、外して
    `cargo clippy --workspace` と `cargo test --workspace` の対象にしない。
- `crates/script-runtime/Cargo.toml` は `deno_core = "=0.413.0"` のまま。版が写しと食い違えば cargo が patch を使わず警告するので、
  食い違いは `cargo tree` で見える。

### D2 記録と作り直しの script
- `deno_core.lock.toml` に crate 名・版・`.crate` の sha256（crates.io の index の `cksum` と同じ値）・外した feature を書く。
- `vendor-deno-core.sh` は次の順で動く。network に出るのは cargo の cache に `.crate` が無いときの download だけ。
  1. `~/.cargo/registry/cache/*/deno_core-<版>.crate` か crates.io の download から `.crate` を得て sha256 を照合する。
  2. 一時 dir に展開し、`Cargo.toml` の `[dependencies.serde_json]` の features から記録の feature を消す。
  3. 元の `Cargo.toml` との差分が記録の行だけであることを確かめる。
  4. `--check` なら repo の写しと `diff -r` で比べ、違えば exit 1。指定が無ければ写しを置き換える。
- 版を上げるときは `deno_core.lock.toml` の版と sha256、`crates/script-runtime/Cargo.toml` の `=` の版を替えて script を走らせる。
  新しい版で serde_json の features が変わっていれば、script は差分の確認で止まる。

### D3 `float_roundtrip` を外す可否
- 外す。外した写しで `celeris-script` の試験が通ることを crate-skeleton の進捗に記録する。
  deno_core と serde_v8 の crate 自身の試験の結果も同じ進捗に書く（走らせられなかった場合はその理由）。
- deno_core 自身が serde_json で f64 を parse する所（op の引数の JSON 等）は丸めが serde_json の既定に戻る。
  `celeris-script` の Host API は数値を V8 の値か serde_v8 で渡し、serde_json の文字列 parse に頼らない（S1 host-api で確かめる）。

## 却下した案
- `preserve_order` を受け入れる: task-api 等の型と hash 計算 4 か所を変え、残り約 70 file の監査が要る。挙動の変更が S1 に混ざる。
- deno_core の git の source を `[patch]` で指す: 版の固定が commit hash になり、crates.io の sha256 で確かめられない。
  build にも network が要る。
- `cargo vendor` で依存全体を写す: deno_core 以外の約 90 crate も repo に入り、作り直しの差分が大きい。

## 影響
- repo に約 2 MB の写しが増える。`cargo fmt --all` と clippy は写しを対象にしない（member でない）。
- deno_core の版を上げる手順に script の実行が加わる。S1 以降で版を上げるときはこの ADR の D2 に従う。
