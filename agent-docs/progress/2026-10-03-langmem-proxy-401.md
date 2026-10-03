---
title: 知識整理 run の llm-proxy 401 の修正（ADR-0139）
tasks: [01M4092VWVZHKVH8J28FMXWY67]
status: done
updated: 2026-10-03
---
# 知識整理 run の llm-proxy 401 の修正（ADR-0139）

> 旧 `docs/PROGRESS.md`（main 4c354a7f）の節を ADR-0128 D6 に従い sync-main-3（task 01M40D0QW6HX3XEZK3GBCQV5ZM）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

task id は ADR-0139 の front matter から採った。

## 知識整理 run の llm-proxy 401 の修正（ADR-0139）— 2026-10-03

- 原因: 06:58:42〜07:00:49 に verify.sh の staging celeris（新旧 2 つ）が本番の config を読み、`[llm_proxy] listen = 127.0.0.1:18100` に `SO_REUSEPORT` で相乗りしていた（staging の乱数トークン）。langmem の接続の一部が verify 側に振られて 401 になった。失敗・成功の run の `langmem_input.json` の鍵は同じ値で、`celeris-api-token` と一致した（sha256 の先頭で照合）。worker 側の鍵の経路には問題が無かった。
- 修正: verify は proxy を待ち受けない（D1）。proxy を指す langmem の鍵は `[api]` のトークン（D2）。鍵が渡らない・食い違う設定は `Config::load` で警告する（D3）。鍵は起動 env にも入れる（D4）。
- 証拠: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p task-worker --lib langmem && cargo test -p celeris config` → exit 0（langmem 14 passed、celeris config 105 passed ほか）。`cargo test -p celeris --lib` → 242 passed。
- 未解決: この release が `current` になるまで、verify.sh の N-1 は 18100 に bind しうる。その間の verify は知識整理 run と重ならない時間に回す。`[mcp]` の待ち受けも verify で同じ形で相乗りする可能性があり、未確認。
