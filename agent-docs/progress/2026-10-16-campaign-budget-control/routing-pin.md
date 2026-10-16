---
title: "campaign B/C の routing 固定・記録と通しの確認"
tasks: [01M526E98AFXB02ZG468J789FB]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# routing-pin

## 実装済み
- ADR `2026-10-16-campaign-routing-pin`（D1〜D5）。
- `Executor::routing` を `Result` に（投入前拒否）。`routing_override` は B/C の routing 欠落も Err。
- `TrialRecord.routing` に実効 routing を記録。checker が manifest の arm 設定と突き合わせ（不一致・欠落・A への付与を違反）。
- 試験 `crates/campaign/tests/routing_pin.rs`（4 件）: task ごとの routing_override（Transport mock）／欠落・C の根拠表無しは投入 0 件で Err／checker の改ざん検出 4 種／通し e2e（反復 2・巡回順・A 全失敗でも B/C 全実行・per_case 超過 cancelled が分母に残る・routing 記録・checker ok）。
- docs/architecture-map.md の campaign 節を更新。

## 証拠
- `cargo test -p celeris-campaign --test routing_pin`: 4 passed。`cargo test -p celeris-campaign`: 全 test binary ok。
- `bash scripts/dev/test-parallel.sh`: 5487 passed, 0 failed。`cargo clippy --workspace --all-targets -- -D warnings` 通過、`cargo fmt --all -- --check` 差分なし。

## 本番への移行
なし（campaign は本番 daemon・config・DB に触れない）。Celeris 経路で本測定を回す manifest は B/C に `routing` を書くこと（上記 D5）。

## 未解決・提案
- `fixtures/pilot.json` の B/C に routing が無い（mock 用）。実測用 manifest は別途作る。
