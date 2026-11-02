---
title: S0 baseline-cos — CoS のまとまった制御変更の baseline と計測の総括
tasks: [01M6DCPKPJZZ9F9J4QJ47FH8KK]
status: done
updated: 2026-11-02
---
# S0 baseline-cos — CoS のまとまった制御変更の baseline と計測の総括

完了日: 2026-11-02

## 変更
- `scripts/dev/edit-activate-baseline.sh` を直した: cos の編集を 33 行の制御（集計 → match → 4 分岐 → 拒否）に広げた。
  編集中の `test-affected.sh --list` を `kind: affected` の行に残す。段の途中の他の cargo/rustc の数と loadavg の最大を
  `env.during` に残す。並列 link の和集合 `link_union_secs` を足し、`compile_secs = secs - link_union_secs` にした。
  任意の段 `affected-test`・`affected-rerun`（`CELERIS_WU_BASE=HEAD` の test-affected.sh）を足した。
- `baseline/cos.jsonl`（warm 3 試行・prime・cold 1 試行）、`baseline/cos-link.jsonl`、`baseline/cos-affected.jsonl`。
- `baseline.md` に cos の表、2 scenario の段別比較、cache・負荷・ばらつき、TS 化で消せる部分の見積りを足した。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- warm 3 試行 → exit 0（18 段すべて exit 0、14 分 8 秒）。cold 1 試行 → exit 0（6 段 exit 0、6 分 23 秒）
- `--validate baseline/cos.jsonl` → exit 0（132 行、problems 0）。`cos-link.jsonl` 59 行・`cos-affected.jsonl` 55 行も problems 0
- 実行後の `git status --short` に crates/ の差分なし

## 未解決
- sccache が効く状態の cold は未計測（私設 server が socket path の長さで起動しなかった）。
- test-affected は `celeris-campaign::ops_r2_socket socket_flag_rejects_non_sockets` の失敗で fail-fast し、実行時間は下限。
  失敗の原因（sandbox の中だけか）は未確認。

## 提案
- sccache を測るときは、私設 server の socket を短い path（例: `$TMPDIR` を短くした sh で `sccache --start-server`）に置く。
- TS 化の効果は、Rust の段（cos で warm 約 137 秒）と test-affected の逆依存試験を、TS の型検査・試験の秒と比べて S1 の PoC で確かめる。
