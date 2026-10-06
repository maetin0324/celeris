---
title: ブラウザ実行の launcher 経路（D4・D6・egress）と確認台本（D1〜D3・D5・D7）の修正
tasks: [01M47QXZR0QMCYZM9KAZC81BCD]
status: done
updated: 2026-10-06
completed: 2026-10-06
---

# ブラウザ実行の launcher 経路と確認台本の修正 — 統合後検査と記録

Fable の実機確認（[証跡](../2026-10-05-browser-web-live-view/real-check-evidence/)）で見つかった欠陥を 3 つの WorkUnit（run-path / egress / script）で直し、統合 commit `d8f3e5e6ca6a` で検査を通した。本葉は実装の修正をせず、検査の実行と記録のみ。

## 直したこと（各葉の記録参照）

- [run-path](2026-10-06-browser-launcher-fix/run-path.md): D4 の action socket path を短い固定長の共通 path に（107 byte 上限を bind 前に検査）、D6 の shim config に `policy_sha256` を書き policy に `launch` を含める。launcher の origin 照合（`action_allowed`）も正規 origin で scheme・host・port を照合する形に。
- [egress](2026-10-06-browser-launcher-fix/egress.md): egress allow が全要素へ `:443` を付け `http` の loopback を塞いでいたのを、daemon と同じ `origin_host_port`（origin の scheme・port に従う）へ。
- [script](2026-10-06-browser-launcher-fix/script.md): 台本と手順書の D1〜D3・D5・D7 を直し、未認証 Live View の拒否・lease 無しの入力拒否・settings 編集の確認を追加。

## 統合後の検査（HEAD `d8f3e5e6ca6a`、run sandbox）

| コマンド | 結果 |
| --- | --- |
| `bash scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0（ok） |
| `bash scripts/dev/check-doc-links.sh` | exit 0（ok） |
| `bash scripts/dev/check-adr-numbers.sh` | exit 0（ok, 146 files） |
| `cargo test -p task-worker --lib` | ok. 785 passed; 0 failed; 4 ignored |
| `cargo test -p task-worker -p task-core` | 全 test result ok、failed 0 |
| `cargo test --workspace --no-run` | exit 0（全 target ビルド成功） |
| `cargo test -p task-worker --lib -- action_socket launcher_shim origin_tests launcher_egress_uses_origin_scheme_and_port` | 7 passed（本 task の新規試験すべて） |
| `cargo fmt --all -- --check` | exit 0 |

worker sandbox では launcher・userns を使えないため、`cargo test --workspace`（実行）の代わりに task-worker・task-core の実行＋workspace の `--no-run` で代えた（実 browser・実 launcher 試験は sandbox 外）。

## 未解決事項

- Fable による実機再確認（台本 `scripts/dev/browser-web-live-check.sh` の全実行、egress 拒否の証跡 `egress-denied.json` 等）は未実施。sandbox 外で launcher が要る。
- host の `/usr/local/libexec/celeris/celeris-browser-launcher` を root が入れ替えないと、launcher 側の修正（origin 照合・egress allow）は本番に効かない。手順は [egress 記録](2026-10-06-browser-launcher-fix/egress.md#人が行う-host-launcher-入れ替え) と `docs/ops/browser-launcher-admission-evidence-run.md` にある。

## 提案

- host launcher の入れ替えは稼働 session が無い時間帯に、退避（`.pre-egress-fix`）を取ってから行うこと。
