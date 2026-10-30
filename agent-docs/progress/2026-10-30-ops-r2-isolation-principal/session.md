---
title: ops-r2 実験専用 operator session の session-map 事前準備と poll・記録の script（WU session）
tasks: [01M669QB069GND2ZKA1T97QPY9]
status: done
updated: 2026-10-30
---
# ops-r2 実験専用 operator session の session-map 事前準備と poll・記録の script（WU session）

完了日: 2026-10-30

ADR 2026-10-30-ops-r2-isolation-principal の D6・D7（後続 unit の表の session 行）を実装した。
`operator_usage` の schema（`celeris.operator-usage/1`・`Mapping`）は変えていない。Claude Code・LLM・daemon は起動していない。

## 変更
- `session: 実験専用 operator session の準備・poll・記録の script と試験` — 次を追加した。
  - `scripts/campaign/ops-r2-session-prep.sh`: 試行前に起動票 `ticket.json`、測定専用 `CLAUDE_CONFIG_DIR`
    （`cleanupPeriodDays: 3650`、`Agent`・`Task` の拒否）、初期 context、`session-map.base.json`（prep・observe）、
    `session-trials.json`（trial_key ↔ condition_id・request_ref）、bwrap の `launch.sh`、`SHA256SUMS` を作る。
    既存の証跡 dir と人の普段の `~/.claude` は拒否する。
  - `scripts/campaign/ops-r2-operator-context.md.tmpl`: 初期 context の雛形（試行の識別・使ってよい script・停止条件・記録の規則だけ）。
  - `scripts/campaign/ops-r2-session-map.py`: `base`・`final`（human-op list の external_operator の op 区間で observe・prep を
    intervene に分ける。byte 一致。終了の無い介入・他試行にかかる介入は exit 2）・`summary`（取れない消費を null と理由で残す）。
  - `scripts/campaign/ops-r2-poll.sh`: 測定 daemon の GET だけで状態を見て、1 回ごとに `status_check` 行（actor.kind=script）を書く。止まる条件で exit。
  - `scripts/campaign/ops-r2-human-span.sh`: 人の能動時間の区間を begin/end で 1 行（actor.kind=human、evaluator_live）に記録。
  - 試験 `crates/campaign/tests/ops_r2_session.rs`（4 件）と `scripts/campaign/test_ops_r2_session.py`（3 件）。
  - 文書 `docs/ops/ops-r2-operator-session.md`（新規）、`docs/ops/operator-usage.md` に ops-r2 の節を追加。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo test -p celeris-campaign --test ops_r2_session` → exit 0（4 passed: 開始前の準備物、親と subagent の再取り込みで合計不変
  （同じ snapshot と途中 snapshot の再取り込みで added=0・byte 一致）、区間外 = shared_unattributable、欠けた usage は null と
  `usage_missing:1`・transcript 無しは全 null と `transcript_missing`、終了の無い介入・他試行にかかる介入で map を作らない）
- `python3 -m unittest test_ops_r2_session`（scripts/campaign）→ exit 0（3 passed）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 2861 passed、9 skipped。progress_index_commit_hashes も通過）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 実 Claude Code での `launch.sh` の起動と transcript の実 path（`ticket.json` の `transcript_expected`）は未確認。
  path は cwd の英数字以外を `-` にした名前という前提（2.1 系の形）。初回の起動で実物と照合する。
- `launch.sh` の bwrap 引数は isolate unit の `ops-r2-sandbox.sh`（並行 WU）と別に書いた。統合後に共通化できるか probe・close 段で見る。
- `.credentials.json` を ro で bind するため、Claude Code の OAuth token の更新が書けない可能性がある（未確認）。
- poll の止まる条件は task の status だけ（受信箱の件は見ていない。blocked で代用）。

## 提案
- probe unit: CoS 直接遂行と task 経由の fixture で、この script の出力（session-map・summary）を照合に使う。
