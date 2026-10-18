---
task: pilot-test-fix
status: done
completed: 2026-10-18
---
# pilot_wiring a_exit_zero_without_any_artifact の不安定の原因と修正

## 原因
単独実行（`cargo nextest run -p celeris-campaign --test pilot_wiring` ×6）は全通過で再現しない。負荷時（workspace 全体並列）だけ落ちた。
スタブの書き出しは既に別 process の `cat > && chmod`（ETXTBSY 回避済み）で、no_artifact 判定（runner.rs）も正しい。
実バグは `HarnessExecutor::poll`（crates/campaign/src/harness_exec.rs）の競合:
1. `exit.code` を読む（まだ無い）
2. その直後に stub が終了し、wrapper が `exit.code` を書いて zombie になる
3. `alive()` が false → `process_vanished` で `Outcome::Error` と final.json を確定

→ A の outcome が `error` になり、`assert_eq!(a["outcome"], "failed", "{a}")` が落ちる（passed でなく error になる経路）。負荷で 1→3 の間に終了が挟まるほど起きやすい。

## 修正
`!alive` 判定の時点で `exit.code`/`final.json` が現れていれば消えた扱いにせず `Wait` を返し、次の poll で正規の結果を読む。テストは緩めていない。
時計/負荷に頼らず再現する hook は無いので、専用の追加試験は置かない（既存 pilot_wiring・harness_exec 試験で通常経路を確認）。

## 証拠
pilot_wiring 連続実行、test-parallel.sh、clippy、fmt の結果は run 報告に記載。
