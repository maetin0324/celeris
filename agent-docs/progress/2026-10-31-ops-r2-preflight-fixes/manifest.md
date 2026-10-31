---
title: K2 ops-r2 の開始前チェック修正 — WU manifest（ops-r2 の K2 manifest と preflight の受け入れ形）
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
status: done
updated: 2026-10-31
---
# K2 ops-r2 の開始前チェック修正 — WU manifest

完了日: 2026-10-31

ADR: `agent-docs/adr/2026-10-31-ops-r2-preflight-fixes.md` の付記 2026-10-31（manifest unit）M1〜M4。

## 変更
- `crates/campaign/fixtures/frozen-manifests/ops-r2-k2.json`・`ops-r2-k2-input-manifest.json` を追加。契約 revision `main-4-ops-r2`、承認票の固定値（A1・B1、1 試行 1,500,000 token・3600 秒、合計 3,000,000 token・7200 秒）。
  private・grade-case は `contract.grader_refs` に sha256 の参照だけ（評価器領域は読んでいない。`case.json` は変えていない）。
- `manifest.rs`・`ops_r1_conditions.rs`: revision `main-4-ops-r2`、`grader_refs`（ops-r2 だけ、problems を過不足なく覆う）、`is_ops_r2()`、preflight だけ ops-r2 を受ける。
- `cases.rs`: `CaseSet::load_public`（private を読まない）。
- `preflight.rs`: `run_checks_ops_r2`。missing 8 件の扱いは ADR 付記 M2 の表のとおり（実在する入力の検査に直したもの 1 件、外したもの 7 件を理由つきで記録）。
- 試験 `crates/campaign/tests/preflight_ops_r2.rs`（7 件）。

## 証拠（提出時の HEAD で実行）
- `cargo test -p celeris-campaign --test preflight_ops_r2` → 7 passed
- `bash scripts/dev/test-affected.sh` → exit 0（2909 tests passed、9 skipped。docs・ADR 番号・human-text 検査 ok）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 範囲 check（`CELERIS_WU_SCOPE_PATHS`）→ 変更 path は ADR・自分の進捗・campaign の crate・fixtures だけ

## 未解決・手順書への引き継ぎ（docs unit）
- ops-r2 の preflight に渡す引数は `--manifest crates/campaign/fixtures/frozen-manifests/ops-r2-k2.json`・`--repo-src`・`--inputs-src`・`--measure-config`・`--normal-config`・`--a-program`（local の固定 copy）と隔離の引数。`--api-url`・`--token-env`・`--task-template`・`--materials-src`・`--kb-src`・`--canaries`・`--cap-env` は要らない。
- 実機の preflight（実物の `--repo-src` 等）は未確認。operator が §3 でやり直す。
- manifest の `claude_code_sha256` は `/local/celeris/data/measure/ops-r2-k2-p1/bin/claude-2.1.303` の sha256（manifest の `claude_code_sha256`）。別の copy を使うなら manifest を作り直す（campaign_sha256 が変わる）。
