---
unit: policy-close
status: done
completed: 2026-10-15
---
# policy-close

## 変更
- 統合試験 `crates/task-dispatch/tests/policy_close.rs`（7 件、公開 API のみ。分類は注入、実 API なし）: 既定 fixed・fixed は分類を読まない／分類は lane を下げない／明示 lane・天井・利用可否が勝つ／fallback と `real_api_verified` が別記録／一時障害は待機で昇格せず内容失敗は 1 段／unknown quota・cost を 0・可にしない／`mlsys-min-1` 固定と根拠表ルータ非依存。
- `task_dispatch` に `classify_failure`・`FailureOrigin`・`FailureAction`・`admit_by_availability`・`LaunchAdmission` を再 export（挙動変更なし）。
- ADR 付記「policy-close」（型名・関数名・試験名の突き合わせと `policy_version = "mlsys-min-1"`）、`docs/architecture-map.md` に policy の行。

## 証拠
- `cargo test -p task-dispatch --test policy_close`: 7 passed。
- `bash scripts/dev/test-parallel.sh` と `cargo clippy --workspace -- -D warnings`: test-parallel 5346 passed / 0 failed（exit 0）、clippy exit 0。

## 未解決事項
- **Jev 実 API 統合は未検証**（`real_api_verified = false`）。live smoke は有料・人の承認が要る。
- 分類段が WU ready 時に board を埋める tick 連携、`decide_route_failure` を worker_finish の再試行・再計画経路へつなぐ部分（attempts を events から導く）は未配線（classify-stage・wire-escalation の未解決事項のまま）。
- 本番 config の移行は不要（`[routing]` は任意で既定 fixed、消した欄なし）。

## 提案
- 投稿時に本番設定の lane 対応を写して `mlsys-min-1` と併記する。
- 次段で上記 2 つの配線を行ってから decision_assisted を shadow 運用に上げる。
