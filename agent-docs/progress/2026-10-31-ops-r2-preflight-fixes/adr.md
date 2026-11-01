---
title: K2 ops-r2 の開始前チェック修正 — WU adr（ADR 付記: 方針版・pool・deny list file・claude・canary・L1・sirius・LLM 経路）
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
status: done
updated: 2026-10-31
---
# K2 ops-r2 の開始前チェック修正 — WU adr

完了日: 2026-10-31

ADR: `agent-docs/adr/2026-10-31-ops-r2-preflight-fixes.md`（上位 ADR `2026-10-30-ops-r2-isolation-principal.md` の付記 L1・G1〜G6 と整合させた）。
この WU はコードを変えない。実装は後続の cfg・leak・probe・manifest・docs unit。

## 変更
- ADR を新規に書いた。決定は 8 件。
  - D1 方針版は `ops-r1-measure-config.sh` の必須引数 `--policy-version`、固定の 6 を廃止、skill の版と不一致なら止める
  - D2 `--claude-accounts` を廃止し `--b-account-dir <dir>`、`$STATE/pool/` に symlink 1 つの pool を作る（a・operator は入れない）
  - D3 deny list は file 渡し（probe-read `--deny-file`、preflight は file を bwrap に ro bind、漏洩検査 `--deny-readable-file`）、旧い引数は廃止
  - D4 測定用 claude は local の固定 copy（版を承認票と照合し sha256 を記録）、probe の mount 照合は変えない
  - D5 ops-r2 は canary なし。same_uid は runtime probe と L1 probe の合格（4 条件）で外し `same_uid_cleared_by: runtime_probe_and_l1` を記録。根拠と緩めない点を書いた
  - D6 L1 probe の `mounts_outside_allow` は hit にしない（G6）。B の probe の手順書の command に `--allow` を入れる
  - D7 sirius への接続は operator が bwrap の中から ssh で確かめ、通らなければ止めて人に返す（コードでは確かめない、未確認）
  - D8 測定 daemon の `[maintenance.stagnation]` の閾値を 0、既存の langmem・gc 等の無効を維持、試行と無関係な CoS run は正式試行にせず別に記録
- 進捗ファイル（このファイル）を書いた。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 文書だけの変更（ADR 1 件・進捗 1 件）。
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」、check-doc-links self-test ok）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-doc-links.sh` → exit 0、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0、
  `sh scripts/dev/progress-index.sh --check` → exit 0、`python3 -I scripts/dev/check-human-text.py` → 0 findings

## 未解決
- D7 の sirius 接続は未確認（operator の実機確認待ち）。
- D8 の「account の credential で外部 LLM を呼ぶ他の常駐経路」の洗い出しは cfg unit に任せた。

## 提案
- なし
