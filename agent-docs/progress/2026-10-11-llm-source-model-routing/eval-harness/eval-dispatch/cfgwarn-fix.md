---
title: cfgwarn-fix — celeris config 試験の tracing 警告捕捉を並走でも決定的にする
tasks: [01M4Q7C4X2QR3HYPTTQAG5GXQ9]
status: done
updated: 2026-10-12
---
# cfgwarn-fix — config 試験の警告捕捉を並走でも決定的にする

対象: `crates/celeris/src/config/tests.rs` の `legacy_provider_concurrency_is_optional_and_warns_including_zero_and_include`。
直したのは試験側だけ。本体の警告（`crates/celeris/src/config/providers.rs` の `tracing::warn!`）と assert の意味は変えていない。

## 原因

- `tracing` の callsite interest は process 全体で共有される。`warn!` の callsite は最初に通った thread で登録され、その時点の interest が cache される。
- dispatcher（subscriber）が 1 つしか登録されていない（`has_just_one`）と、tracing-core は interest を「登録した thread の既定 subscriber」だけで決める（`Rebuilder::JustOne` → `dispatcher::get_default`）。
- 並走する別の試験（同じ `warn!` を通る `validate()` を呼ぶ）が、この試験の `with_default` の scope の中で callsite を初めて登録すると、その thread は既定 subscriber を持たないので `Interest::never` が cache に残る。`with_default` は interest を作り直さないので、以後この試験の thread では警告が一切出ず、捕捉 log が空になる。
- 単独実行や別順序では scope の前に callsite が登録されるため通る。並走の初回登録が scope の中に入ったときだけ落ちる（実測 1 回 / 3 回）。
- `MAX_LEVEL` は dispatcher が 1 つも作られていない間 `OFF` なので、scope の前に subscriber なしで読んでも callsite は登録されない（前処理では効かない、と実験で確認）。

## 直し方

1. 常時 1 つ anchor dispatcher を `OnceLock` に登録する（sink に書く fmt subscriber、WARN のみ）。これで registry は複数 dispatcher の経路（`Rebuilder::Read`/`Write`）に乗り、interest は全 dispatcher から計算されて thread に依らなくなる。anchor は何の thread の既定にもならないので、他の試験のログには影響しない。
2. 並走の初回登録を決定的に再現するため、scope の中で別 thread に同じ `Config::load` を走らせてから本体を読む。anchor が無いと、この形で必ず落ちる（実験で確認）。

`tracing::callsite::rebuild_interest_cache()` を scope の中で呼ぶ案は効かない（実験で確認）。並走の初回登録が rebuild の後に来ると、同じ `JustOne` 経路で `never` が残るため。

## 検証

| 条件 | 結果 |
|---|---|
| 修正前、並走を模した試験（scope 中に別 thread が登録） | 決定的に失敗（捕捉 log が空、4623 行付近の assert） |
| `rebuild_interest_cache()` のみ（scope 中） | 失敗（並走の初回登録が rebuild の後） |
| subscriber なしの前読み（scope 前） | 効かない（`MAX_LEVEL` が OFF で callsite が未登録のまま） |
| anchor + scope 中の別 thread 登録（修正後） | 通過 |
| `cargo test -p celeris config` を 3 回続けて | 3 回とも exit 0（187 passed, 0 failed） |
| `cargo test -p celeris --lib config` を追加で 10 回 | 10 / 10 通過 |
| `cargo clippy -p celeris --all-targets -- -D warnings` | exit 0 |

修正前は `cargo test -p celeris config` の 3 回中 1 回落ち、直後の再現ループでも 1 回落ちた。修正後は計 13 回（上の 3 回と追加の 10 回）とも通過。

## assert について

assert は一つも変えていない。deprecated 警告 3 行（pool・included・legacy）、omitted 無し、pool と非 pool の文言、`(implicit/none)` はそのまま。`--test-threads=1` や `ignore` は使っていない。

## 同じ捕捉の形をもつ他の試験

- `crates/celeris` 内で `with_default` / `set_default` を使うのはこの試験だけ（`main.rs` の `with_writer` は本番の log 初期化）。直すものは他に無い。
- 別 crate の `crates/llm-proxy/tests/proxy_integration.rs:1901` も `set_default` で捕捉するが、current_thread runtime の別形。celeris の作業範囲外なので触っていない。同じ並走 flake の疑いがあれば別 WU で確かめる。

## 未解決事項

- anchor は tracing-core の内部（`has_just_one` の経路）に依る。tracing の版を上げたら試験 `legacy_provider_concurrency_…` で再確認する。
- 提案: 警告を戻り値で返す形（`Config::load` が警告を収集する）に寄せると、log 捕捉そのものが要らなくなる。本体の変更になるので今回は見送った。
