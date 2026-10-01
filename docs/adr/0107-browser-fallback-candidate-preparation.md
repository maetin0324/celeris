# ADR-0107: browser fallback 候補の run ごとの準備を主 adapter と揃える

---
tasks: [01M3QGRC6AQ81PWM1XP4C7BH45]
---

- 日付: 2026-09-30
- 状態: Accepted
- 関連: [ADR-0106](0106-browser-phase4-conformance-dispatch.md)、[ADR-0075](0075-tiered-build-cache.md)、ADR-0072 D14

## 文脈

ADR-0106 D3 で dispatch は browser fallback 候補を worker に渡すようになった。しかし `run_worker` が主 adapter にだけ run ごとの包み（ADR-0075 の `with_env_removed` による継承 env の除去、`CARGO_TARGET_DIR` と sccache 系の env、コンテナ、planner の permission mode）を施し、候補は `self.adapters` の生の `Arc` のまま `run_with_candidates` に届いていた。fallback した run は主 run と同じ条件で動いていなかった。

## 決定

1. fallback 候補は、主 adapter と**同じ 1 つの関数**（`prepare_run_adapter`）で run ごとの準備を受ける。対象は ADR-0075 の env 除去と env 設定（`CARGO_TARGET_DIR`・`[scratch.cargo]`・sccache）、コンテナ、planner の permission mode。tier ごとのモデル選択は、主 adapter と同じ `req.task.worker_hint.tier`（実行 tier）を使い、各候補の `TieredAdapter` が run 時に解決する。候補ごとに別の tier を選び直さない。`self.adapters` の生の entry を `run_with_candidates` / `run_with_executable_candidates_record` へ渡さない。
2. 候補は、有効かつ healthy な provider のうち、adapter id が runner の記録（ADR-0106 D1）で適合とされた browser backend に限る。候補の絞り込みは dispatch 側（`spawn_worker`）で行い、準備（本 ADR D1）は `run_worker` で行う。
3. `CredentialUse`（認証 wait・auth_section を含む run）と `IdentityRestore` を要求する run は fallback しない。主 adapter が失敗したら、その run は明示的に失敗・拒否する。worker では `CredentialUse` を含む policy の run で候補を足さない（ADR-0106 D3）。`IdentityRestore` などの機密能力は、P4-A/B の実適合が記録されるまで起動前に拒否される（ADR-0103）ので、fallback の対象にならない。

## 結果

- fallback 先の run は主 run と同じ除去済み env・`CARGO_TARGET_DIR`・permission mode・実行 tier で起動する。
- 主 adapter だけが準備に失敗する（例: `with_env` 非対応）場合と同じ扱い（既存どおり、その包みを適用しないまま走る）を候補にも適用する。
- auth_section / H3 の経路には手を入れない。
