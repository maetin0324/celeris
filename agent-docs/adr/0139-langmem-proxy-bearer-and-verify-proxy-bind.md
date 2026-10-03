---
tasks: [01M4092VWVZHKVH8J28FMXWY67]
---
# ADR-0139: 知識整理 run の llm-proxy bearer と verify の proxy 待ち受け（ADR-0053・ADR-0047 付記）

- 日付: 2026-10-03
- 状態: **実装済み**（2026-10-03）
- 関連: [ADR-0053](0053-llm-source-proxy.md) D1（proxy の bearer は `[api] token_file`）、[ADR-0047](0047-knowledge-base.md) D4（langmem アダプタ）、[ADR-0040](0040-self-improvement-deploy.md) D3（`--mode verify`）、[ADR-0132](0132-provider-llm-source-split-and-cheap-qwen.md) D4

## 文脈

2026-10-03 06:59〜07:00 UTC、本番（163f41c0c857）で知識整理の langmem run 4 回（task 01M408QDYH8RB2XF09T2EQJZ7T・01M4090A0BVNTP1XB67F2GEYQW）が
`langmem extraction failed: Error code: 401 - missing or invalid bearer token` で落ちた。06:53 の別 task の run は成功している。

調べたこと（秘密の値は伏せ、sha256 の先頭 12 桁だけ比べた）:

- 成功・失敗の全 run の `runs/<run_id>/langmem_input.json` の `llm.api_key` は同じ値（`1b1078f74a58…`）で、`~/.config/celeris/secrets/celeris-api-token` と一致。`base_url = http://127.0.0.1:18100/v1`、`model = celeris/cheap` も同じ。**worker 側で鍵が抜けた・違う値になったのではない**（provider の env・tier・sandbox の経路はどれも無関係）。
- 401 の本文は `llm-proxy` の `guard`（`crates/llm-proxy/src/server.rs`）が返すもの。
- 同じ時刻、`scripts/selfdeploy/verify.sh` が staging を動かしていた（`~/.local/celeris/staging/logs/celeris-new.log` と `celeris-old.log` に 06:58:42 / 06:58:58 `llm-proxy listening addr=127.0.0.1:18100`、07:00:49 `llm-proxy stopped`）。
- verify の celeris は**本番の config.toml**を読み、`--token-file` だけ staging の乱数トークンに差し替える。`[llm_proxy] listen` は差し替えないので、新旧 2 つの verify インスタンスが本番と同じ `127.0.0.1:18100` に `SO_REUSEPORT` で bind した。verify の役割は `accepts_admin()` が真なので、カーネルが verify 側に振った接続は staging トークンで照合され 401 になる（3 つの listener への分散なので「ときどき」落ちる）。

## 決定

### D1. `--mode verify` は llm-proxy を待ち受けない

verify は `ProxyState` を組み立て（`GET /llm/sources` 用）るが、`[llm_proxy] listen` には bind しない。verify の煙試験は偽のアダプタだけを使い、proxy への接続を要しない。
本番の待ち受けを verify が横取りする経路はこれで無くなる（`daemon::run::serves_llm_proxy`）。

**残る穴**: verify.sh は N-1（`current`）のバイナリも verify で起こす。この修正を含む release が `current` になるまでは、N-1 側が 18100 に bind しうる。

### D2. langmem の鍵は proxy を指すとき `[api]` のトークン

`[knowledge.langmem].base_url` が同じ celeris の llm-proxy（`[llm_proxy]` が有効で、loopback か `listen` の IP、かつ `listen` のポート）を指すなら、langmem に渡す鍵は `[api] token_file` の値にする（proxy が照合するのはそれだけなので、常に通る）。
`api_key_secret` はそれ以外の接続先のための設定として残す。解決は `Config::langmem_api_key` 1 箇所にまとめ、アダプタ（`build_adapters`）と dispatch 前の probe（`dispatch_config`）が同じものを使う。

### D3. 渡らない・食い違う設定は起動時に警告

`Config::load` は、`[knowledge.langmem]` が有効なとき `langmem_auth_warnings` を `warn!` で出す（値はログに出さない）:

- proxy を指しているのに `[api] token_file` が無い・読めない（鍵が渡らない）。
- proxy を指していて `api_key_secret` の値が `[api]` のトークンと違う（D2 で `[api]` の方を使う旨）。
- proxy 以外を指していて `api_key_secret` が解決できない。

### D4. 起動 env にも鍵を入れる

langmem アダプタは入力 JSON の `llm.api_key` に加え、子プロセスの env に `OPENAI_API_KEY`（`openai-compatible`）/ `ANTHROPIC_API_KEY`（`anthropic`）を**provider の env より後に**入れる（provider 行や `[adapters.langmem].env` の古い値が勝たない）。
ランナーは JSON に鍵が無ければこの env を使う。

## 検証

- `cargo test -p task-worker --lib langmem`: tier cheap / standard × provider env（古い `OPENAI_API_KEY`）あり / なし の 4 通りで、起動 env と入力 JSON に鍵が入ることを偽ランナーで固定（LLM は呼ばない）。
- `cargo test -p celeris config`: proxy を指す設定で `[api]` のトークンが選ばれること、`api_key_secret` 無し・食い違い・未解決の警告、verify では proxy を待ち受けないこと。

## 人が行うこと

本番への反映は通常の release / verify / handoff で行う。D1 の残る穴のため、この release を `current` にするまでの verify は知識整理 run と重ならない時間に回す。
