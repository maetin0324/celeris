---
title: 多目的モデルルーティングの設計と Phase 別進捗
tasks: [01M44H0SRV70E32AQ6C5N37MSK, 01M44MBCP98FNEEMEXG5CQRQQA]
status: done
updated: 2026-10-05
---

# 多目的モデルルーティングの設計と Phase 別進捗

## 現在地

**完了: 2026-10-05（close）。** 親 task は `01M44H0SRV70E32AQ6C5N37MSK`、この設計 task は `01M44MBCP98FNEEMEXG5CQRQQA`。
ADR [2026-10-04-multi-objective-model-routing.md](../adr/2026-10-04-multi-objective-model-routing.md) の状態は
**「実装済み」**（Phase 1〜5）。各 Phase の実装記録（commit・試験）は下の「Phase 別記録」、close の全体検査は
「close の全体検査（完了日: 2026-10-05）」。本番 config の移行・本番操作・戻し方は
[docs/ops/model-routing-migration.md](../../docs/ops/model-routing-migration.md)（実行者は人）。

| 段 / unit | 状態 | 成果 |
| --- | --- | --- |
| research / inventory | 完了（先行葉） | [現行の棚卸し](2026-10-04-multi-objective-routing/inventory.md)、基点 c8612886 |
| research / upstream-oss | 完了（先行葉） | [upstream 調査](2026-10-04-multi-objective-routing/upstream-oss.md)、6 実装と 10 要素の再利用比較 |
| research / arch-adr | 完了 | [architecture ADR](../adr/2026-10-04-multi-objective-model-routing.md)。状態「実装済み」＋ Phase 1〜5 の付記＋ close 付記 |
| phase1 / p1-model | 完了（2026-10-05） | モデル台帳・純粋 kernel・旧設定 reader。[p1-model.md](2026-10-04-multi-objective-routing/p1-model.md)（close HEAD `d3c65029`） |
| phase2 / p2-state-cost | 完了（2026-10-05） | source 状態・effective cost・予約・選択。[p2-state-cost.md](2026-10-04-multi-objective-routing/p2-state-cost.md)（close HEAD `762cb225`） |
| phase3 / p3-context-esc | 完了（2026-10-05） | task context・軌跡 escalation・feature/reward。[p3-context-esc.md](2026-10-04-multi-objective-routing/p3-context-esc.md)（close HEAD `31edde85`） |
| phase45 / p4-shadow-eval | 完了（2026-10-05） | 上限付き shadow・offline dataset/replay/指標。[p4-shadow-eval.md](2026-10-04-multi-objective-routing/p4-shadow-eval.md)（close HEAD `af1c9a8f`） |
| phase45 / p5-estimator | 完了（2026-10-05） | adapter-plus-routellm: 汎用 adapter、実 RouteLLM の起動停止手順・実 shadow 評価 report（人の実行・内部評価のみ）。[p5-estimator.md](2026-10-04-multi-objective-routing/p5-estimator.md)（close HEAD `18f60cef`、実 sidecar 原票 `3c1a0f5c`） |
| phase45 / close | 完了（2026-10-05） | 本ファイル・ADR の「実装済み」と close 付記・[移行手順](../../docs/ops/model-routing-migration.md)（§10・本番操作まとめ）・[architecture-map](../../docs/architecture-map.md) の routing 行（統合 HEAD `dace40c0`） |

## close の全体検査（完了日: 2026-10-05、統合後 HEAD `dace40c0`）

| # | 検査 | コマンド | 結果 |
| --- | --- | --- | --- |
| 1 | 全 workspace 試験 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest Summary 原文: `4035 tests run: 4035 passed (1 slow), 13 skipped`、doc-test 0 failed |
| 2 | clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 3 | 書式 | `cargo fmt --all -- --check` | exit 0 |
| 4 | Phase 5 新試験 + Phase 4 + 既存回帰 | `cargo nextest run --workspace --no-fail-fast -E 'test(/routing_sidecar_/) \| test(/routing_routellm_pair_adapter/) \| test(/routing_estimator_/) \| test(/routing_shadow_/) \| test(/routing_decision_shadow/) \| test(/routing_offline_replay/) \| test(/cheap_local_first_/) \| test(/cheap_only_legacy_mappings_never_route_frontier_or_standard_to_qwen/) \| test(/claude_429_falls_back_to_the_next_account_and_records_a_cooldown/) \| test(/select_provider_sticks_to_the_sessions_account_over_a_better_scoring_one/)'` | exit 0。`39 tests run: 39 passed, 4009 skipped`（0 件実行なし） |
| 5 | wrapper の unittest | `python3 -m unittest discover -s scripts/model-routing -p 'test_*.py'` | exit 0。`Ran 3 tests` OK |
| 6 | runbook 検査（承認込み） | `sh scripts/model-routing/check-runbook.sh --require-approved` | exit 0。`ok (routellm 0b64fdafe049e596a3f5657c219329f24af24198, routellm-weights-use=approved)` |
| 7 | 偽 sidecar 検査 | `sh scripts/model-routing/fake-shadow-check.sh` | exit 0。`ok (validated, SIGTERM exit 0, pid gone, port closed)` |
| 8 | 実 sidecar 検査 | `sh scripts/model-routing/real-sidecar-check.sh` | worker 内では exit 2（weights なし = 未実行、合格扱いにしない）。実行の証拠は人の原票（#10） |
| 9 | 文書検査 3 本 | `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` | いずれも exit 0（ADR・進捗・手順書・architecture-map の変更の後に再実行） |
| 10 | architecture-map 検査 | `python3 scripts/dev/check-architecture-map.py` | exit 0（routing の行に新規 module と ADR を足した後で再実行） |
| 11 | 実 RouteLLM sidecar の原票（人: Fable、2026-10-05、内部 shadow 評価に限る） | 本番 host で `scripts/model-routing/real-sidecar-check.sh` の start-stop と上限付き shadow（`--max-requests 50`） | start-stop exit 0（ready・pair score 1 件・SIGTERM で exit 0・port 閉鎖・peak RSS 約 2.6 GB）、shadow exit 0（合成 dataset 30 件: completed 30 / failed 0 / timeout 0 / dropped 0、外部呼び出し 0、`/estimate` mean 44.6 ms・p95 42.9 ms）。原票 `docs/reports/model-routing-routellm-shadow/`（`run-manifest.txt`）、転記は [report §3](../../docs/reports/model-routing-routellm-shadow.md) |

Phase ごとの close 検査（Phase 1〜5 の filterset・gui/web の typecheck・test・文書検査）は
「Phase 別記録」の各ファイルに証拠を揃えている。

## 設計で固定した境界

- ModelProfile は source 間で共有し、DeploymentProfile と SourceState に供給元の静的属性・動的観測を分ける。新しい account 帳簿は作らない。
- kernel は `task-core` の純粋型・関数。dispatcher は lane/run/provider、proxy は同じ lane 内の request/source/model を決める。QualityEstimator は助言のみで、外部 estimator は非同期の shadow 専用。
- cash、subscription の shadow price、self-host の resource pressure/機会費用、推定/実測/unknown を区別する。hard constraints と品質下限は score より優先する。
- 再利用方式は upstream 調査と同じく (c) 8 項目、(b) 評価指標の数式、(a) Phase 5 の estimator sidecar。pin・配布条件・notice・追従負担を ADR §8 に記録した。weights/dataset の未確認 license をコードの license で代用しない。
- 回答済みの `shadow-exec=opt-in-capped` と `estimator-scope=adapter-plus-routellm` を採用。実行 shadow は既定 off、対象・日次上限・再起動後の予約を検証する。Phase 5 は偽 sidecar 試験だけでは完了せず、実 RouteLLM の起動停止・上限付き shadow 評価を必須にした（2026-10-05 に人の手順で実行済み）。

## Phase 5 の前提と受け入れ条件

**[解消済み] `routellm-weights-use`（2026-10-05、人の決定）。** `routellm/bert_gpt4_augmented`
（観測 revision `86237e3df400`、HF に license 宣言なし）は**手元（本番 host・内部環境）での shadow 評価に限り
使ってよい**（再配布・公開・同梱はしない。取得先 revision を固定して記録）。外部発表の段階で license を人が
再判断する。承認記録と条件・対象 revision・checksum は手順書
[docs/ops/model-routing-migration.md §10.1・§10.3](../../docs/ops/model-routing-migration.md)
（`routellm-weights-use: approved`）と [report §3](../../docs/reports/model-routing-routellm-shadow.md)
（原票 `docs/reports/model-routing-routellm-shadow/`）に残す。ADR の [needs-human] はこの回答で解消として
付記した（ADR §7.3）。

ADR §7.3 / §10 Phase 5 の必須成果はすべて揃った:

- `docs/ops/model-routing-migration.md` §10: RouteLLM/wrapper/weights の pin、license/notice と承認根拠、Python・torch 等の lock、CPU/GPU 要件、起動・ready・推論・停止・PID/port/資源解放を確認する手順。`check-runbook.sh --require-approved` が exit 0。
- `docs/reports/model-routing-routellm-shadow.md`: 許可済み dataset と上限、実 classifier の成功応答、消費・失敗・coverage・校正・overhead・primary 不変の評価 report（§1 偽 sidecar、§2 結合の食い違いと解決、§3 実 sidecar）。
- `routing_routellm_runbook_pins_dependencies_and_license`・`routing_routellm_real_sidecar_start_stop`・`routing_routellm_real_shadow_within_caps`: 偽 sidecar 試験は通常 CI、実 sidecar の 2 本は opt-in（`real-sidecar-check.sh`。未実行は exit 2 で合格にならない）。2026-10-05 に人の手順で実行済み。

## 未解決事項（実施していない・将来の変更が必要な項目）

実装は Phase 1〜5 の範囲で完了したが、次の項目は**実施していない**（各 Phase 付記の「実装していないもの」と
ADR close 付記の同じ内容）:

1. **本番 config での opt-in が要る評価**: Celeris 本体の estimator shadow（coverage・
   `estimator_version_mismatch`・hard constraint 違反数・primary 変更数）と decision / 実行 shadow の本番での
   有効化は未計測・未有効（既定 off）。有効化手順は [移行手順 §9・§10.7](../../docs/ops/model-routing-migration.md)
   で人が行う。
2. **`mode = "enforce"` の対象拡大**: 現在は heuristic の opt-in で dispatcher 経路だけ（ADR §7.2 末どおり
   自動には行わない。人の判断）。
3. **組織 privacy 制約の設定経路**（server/org 単位の `Constraints`）と、enforce の proxy 選択への context
   適用（ADR Phase 3 付記）。`context_transport_unsupported` の除外は経路固定のみ。
4. **learned scorer / bandit**: assess の結論どおり範囲外（paired outcome 0・propensity なし）。paired outcome
   が集まってから再検討する。
5. **RouteLLM weights の外部発表前の license 再判断**: 人の判断（手順書 §10.3・report §3）。内部評価の範囲では
   変更しない。
6. **main への統合**: close HEAD `dace40c0`（`celeris/01M44H0SRV70E32AQ6C5N37MSK`）までの統合まで。main への
   取り込みと本番への release 昇格は Celeris の配送（人の判断）で別に行う。本番 daemon・DB・config はこの
   task では変更していない。

## 提案（次につなげるもの）

- 本番で decision shadow（`mode = "shadow"`）をまず有効にして、`routing export` / `routing evaluate` で
  legacy と候補 policy（heuristic）の比較 report を溜める（移行手順 §9.1・§9.5）。
- 比較 report が溜まったら estimator sidecar（RouteLLM）を本番 config で opt-in し（§10.7）、
  `routing evaluate --policy estimator` で heuristic との差を継続比較する（shadow のまま。決定権なし）。
- paired outcome が溜まったら APGR/AIQ/IBC の計算と learned estimator の再検討（上の未解決 4）。

## cheap-local-first との関係

確認時の main は `33774b6ae894c9c3727842cf9337266c64822c18`。task `01M44G5KKF8VJ0ARJH8J843T7D` の修正を含み、
進捗・ADR-0132 付記 L1〜L8・差分を同 commit の原本で確認した。Phase 1 前に親計画で統合し、
`cheap_local_first`・`select_provider_for`・probe・`LaneResolution.selection` と dispatcher 回帰試験を
再利用した（Qwen 優先の別実装は作っていない）。回帰（`cheap_local_first_*`・
`cheap_only_legacy_mappings_never_route_frontier_or_standard_to_qwen` 等）は close 時まで
0 件実行でなく通っている。

## 検証（ADR 作成 task の当初記録）

ADR 作成時点（research 段）の検査記録は過去の進捗のまま: ADR 必須語句・`check-adr-numbers.sh`・
`check-doc-links.sh`・`progress-index.sh --check`・`git diff --quiet $(git merge-base HEAD main) -- crates/ gui/ web/`
（コード差分なし）・`git diff --cached --check` / `cargo fmt --all -- --check` が exit 0。
本 run（close）での再検査は「close の全体検査」を正とする。
