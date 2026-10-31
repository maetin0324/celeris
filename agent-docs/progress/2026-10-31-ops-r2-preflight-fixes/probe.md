---
title: K2 ops-r2 の開始前チェック修正 — WU probe（deny list file・--claude-sha256）
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
status: done
updated: 2026-10-31
---
# WU probe

完了日: 2026-10-31

## 内容
- probe-read・preflight_isolation は deny list を `--deny-readable-file <path>`（1 行 1 path）で読み、bwrap の引数に path を並べない。旧 `--deny-readable` は削除。
- preflight に `--claude` と `--claude-sha256` を足した（どちらか片方だけなら miss）。
- 試験: `crates/campaign/tests/preflight_deny_file.rs`、`runtime_probe.rs`（15,000 件の合成 deny file）。userns が要る試験は `CELERIS_USERNS_TESTS=1` opt-in で、WU では走らせていない。

## 前回の check 不合格の原因と対応
範囲 check が `scripts/campaign/measure-launcher.sh`（範囲は crates/campaign/・progress・adr のみ）の変更を範囲外とした。この変更を取り消した（この WU の diff に残っていない）。

## 検査（取り消し後の HEAD で実行）
- `cargo fmt --all -- --check`: exit 0
- `bash scripts/dev/test-affected.sh`: 末尾の出力は全 suite ok
- 範囲 check と同じ式: 範囲外 path なし

## 提案
- 後続の unit（docs・manifest）で `scripts/campaign/measure-launcher.sh` に `--claude`・`--claude-sha256` の pass-through（ISOLATION_KEYS への追加、`*_SHA256` は abs 化しない）を足す。この WU の範囲外のため入れていない。
