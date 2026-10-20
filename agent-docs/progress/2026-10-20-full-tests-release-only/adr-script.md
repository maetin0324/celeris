---
title: 全体の試験は release gate だけ — ADR と test-affected.sh
tasks: [01M5BZZWMF3EZ0HQ45PAN935FD]
status: done
updated: 2026-10-20
completed: 2026-10-20
---
# 全体の試験は release gate だけ — ADR と test-affected.sh（WorkUnit adr / script）

完了日: 2026-10-20

## 作ったもの

- `agent-docs/adr/2026-10-20-full-tests-at-release-only.md` — 全体の試験（test-parallel.sh、web e2e、本物 browser/userns 試験）は
  `scripts/selfdeploy/release.sh` の gate だけ。WU・段の統合・task 受け入れ・review・phase 完了は、変更 crate と逆依存 crate の試験、
  clippy、fmt、web だけの変更は web の unit 試験（e2e なし）。
- `scripts/dev/test-affected.sh` — 変更 crate と `cargo metadata` の逆依存閉包に絞って nextest（と doc test）を走らせる。`--list` は対象 crate 名だけを出す。
- `scripts/dev/tests/test-affected-list.sh` — 小さな git fixture で `--list` を確かめる単体試験。

## 証拠

- `sh scripts/dev/tests/test-affected-list.sh` → `test-affected-list: ok`（exit 0）
- `bash scripts/dev/test-affected.sh --list` → exit 0
- `cargo fmt --all -- --check` は Rust の変更がないため実行不要。

## 未解決事項

- 旧い運用文言（phase 完了ごとの `test-parallel.sh` など）を CLAUDE.md・guides から消す作業は本 WorkUnit の範囲外。
- `test-affected.sh` の実際の nextest 実行（`--list` なし）は重い負荷になるため、ここでは走らせていない。

## 提案

- CLAUDE.md の「各 Phase 完了時」の検査を `test-affected.sh`・clippy・fmt に置き換え、全体の試験は release gate のみと明記する。
- planner の check 作成規則に `test-parallel.sh` を受け入れ条件へ置かない旨を足す。
