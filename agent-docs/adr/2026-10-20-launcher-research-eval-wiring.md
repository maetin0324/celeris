---
tasks: [01M5BT8CM2EAKEG7CH9DXTS0RK]
---
# ADR: 測定 launcher を共有台帳・arm 別認証・variant 基準 hash・評価側観測に接続する

- Date: 2026-10-20
- Status: Accepted

## Context

`measure-launcher.sh` は CLI の現行契約に追従していなかった。共有承認 manifest は `--global-ledger` が無く watch で拒否され、`A_ACCOUNT` は B/C のみの構成でも必須、`--variant-b0-sha256` と `--initial-trees` は渡せず、`collect-observed`（手動）は標準の watch → grade → export に入っていなかった。

## Decision

- D1: launcher env に `GLOBAL_LEDGER`・`VARIANT_B0_SHA256`・`INITIAL_TREES` を足し、設定されたものだけを CLI に渡す。一致・不一致の判断は CLI（`check_shared_ledger`・`check_variant`）に置き、launcher は判定しない。
- D2: `A_ACCOUNT` を必須キーから外す。A が有効なら preflight が `--a-account` 欠落で拒否し、B/C のみは A の認証を求めない（旧「`A_ACCOUNT is required`」拒否は消えた）。
- D3: `celeris-campaign collect-observed-out` を足し、launcher の `collect` と `grade` の先頭が呼ぶ。実験 case の trial ごとに、台帳の job ID・case の cluster・grade-case の条件・成果物内の raw から `<out>/observed/…` を作る。記録は `trial`（campaign sha256・arm・problem・rep・trial_no）を持つ。単発の `collect-observed` は `--campaign-sha256` と `--trial-no` を要る。
- D4: `grade-out` は観測記録を使う前に、`trial` が当該 trial と一致、job 集合が台帳と一致、scheduler 応答の控えの sha256 が記録と一致、を確かめる。違えば `graded:false` で拒否し非 0。収集側は成果物が実行終了時の hash と違う・job 未終了・台帳に job が無いとき記録を作らない。
- D5: `grade` は収集失敗を警告にとどめて採点を続ける（その trial は観測なしの実験証拠で成功にならない）。単独の `collect` は非 0。

## Consequences

試験は `crates/campaign/tests/launcher.rs` の標準 launcher 入口（実 CLI・実 task-api daemon）。本番 host・LLM・外部ネットワークには触れない。本番の launcher.env の書き換えは不要（新キーは任意。B/C のみで A_ACCOUNT を外すのは任意）。
