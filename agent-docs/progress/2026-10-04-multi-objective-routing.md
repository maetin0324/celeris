---
title: 多目的モデルルーティングの設計と Phase 別進捗
tasks: [01M44H0SRV70E32AQ6C5N37MSK, 01M44MBCP98FNEEMEXG5CQRQQA]
status: running
updated: 2026-10-04
---

# 多目的モデルルーティングの設計と Phase 別進捗

## 現在地

research の arch-adr を作成。親 task は `01M44H0SRV70E32AQ6C5N37MSK`、この設計 task は `01M44MBCP98FNEEMEXG5CQRQQA`。Phase 1〜5 は未実装で、research 段の人レビューを経て進める。このファイルの running は全体の状態であり、今回の ADR 作成の未完了を意味しない。

| 段 / unit | 状態 | 成果 |
| --- | --- | --- |
| research / inventory | 完了（先行葉） | [現行の棚卸し](2026-10-04-multi-objective-routing/inventory.md)、基点 c8612886 |
| research / upstream-oss | 完了（先行葉） | [upstream 調査](2026-10-04-multi-objective-routing/upstream-oss.md)、6 実装と 10 要素の再利用比較 |
| research / arch-adr | 文書作成・検査完了 | [architecture ADR](../adr/2026-10-04-multi-objective-model-routing.md)。実装予定の型・設定・API・試験と現行を区別 |
| phase1 / p1-model | 未着手 | モデル台帳・純粋 kernel・旧設定 reader |
| phase2 / p2-state-cost | 未着手 | source 状態・effective cost・予約・選択 |
| phase3 / p3-context-esc | 未着手 | task context・軌跡 escalation・feature/reward |
| phase45 / p4-shadow-eval | 未着手 | 上限付き shadow・offline dataset/replay/指標 |
| phase45 / p5-estimator | 未着手・[needs-human] | adapter-plus-routellm: 汎用 adapter、実 RouteLLM の起動停止手順・実 shadow 評価 report。weights 利用条件の回答待ち、本番切替なし |
| phase45 / close | 未着手 | 全 Phase の回帰と移行手順の整合確認 |

## 設計で固定した境界

- ModelProfile は source 間で共有し、DeploymentProfile と SourceState に供給元の静的属性・動的観測を分ける。新しい account 帳簿は作らない。
- kernel は `task-core` の純粋型・関数。dispatcher は lane/run/provider、proxy は同じ lane 内の request/source/model を決める。QualityEstimator は助言のみで、外部 estimator は非同期の shadow 専用。
- cash、subscription の shadow price、self-host の resource pressure/機会費用、推定/実測/unknown を区別する。hard constraints と品質下限は score より優先する。
- 再利用方式は upstream 調査と同じく (c) 8 項目、(b) 評価指標の数式、(a) Phase 5 の estimator sidecar。pin・配布条件・notice・追従負担を ADR §8 に記録した。weights/dataset の未確認 license をコードの license で代用しない。
- 回答済みの `shadow-exec=opt-in-capped` と `estimator-scope=adapter-plus-routellm` を採用。実行 shadow は既定 off、対象・日次上限・再起動後の予約を検証する。Phase 5 は偽 sidecar 試験だけでは完了せず、実 RouteLLM の起動停止・上限付き shadow 評価を必須にする。

## Phase 5 の前提と受け入れ条件

**[needs-human] `routellm-weights-use`、needed_before: `p5-estimator`。** upstream-oss §5 で `routellm/bert_gpt4_augmented` の weights license 宣言を確認できていないため、人が根拠を確認してローカル shadow 評価を認めるか、確認まで Phase 5 を保留するかの回答を待つ。回答済みの `adapter-plus-routellm` の範囲を変更する決定ではない。回答までは weights を取得・使用しない。今回の ADR 作成と Phase 1〜4 は進められる。

ADR §7.3 / §10 の Phase 5 で、次を必須の成果と検査契約にした（実装・実評価は後続 Phase の仕事）。

- `docs/ops/model-routing-migration.md`: RouteLLM/wrapper/weights の pin、license/notice と承認根拠、Python・torch 等の lock、CPU/GPU 要件、起動・ready・推論・停止・PID/port/資源解放を確認する手順。
- `docs/reports/model-routing-routellm-shadow.md`: 許可済み dataset と上限、実 classifier の成功応答、消費・失敗・coverage・校正・overhead・primary 不変の評価 report。
- `routing_routellm_runbook_pins_dependencies_and_license`、`routing_routellm_real_sidecar_start_stop`、`routing_routellm_real_shadow_within_caps`: 通常 CI の偽 sidecar 試験に追加する必須検査。未提供・skip・全件失敗は Phase 5 未完了。品質向上は必須にせず、本番切替はしない。

## cheap-local-first との関係

確認時の main は `33774b6ae894c9c3727842cf9337266c64822c18`。task `01M44G5KKF8VJ0ARJH8J843T7D` の修正を含み、進捗・ADR-0132 付記 L1〜L8・差分を同 commit の原本で確認した。この worktree の着手 HEAD `aa44fe420e25` はその変更を含まない。今回 main のコードは merge していない。

Phase 1 前に親計画で既存修正を統合し、`cheap_local_first`、`select_provider_for`、probe、`LaneResolution.selection` と 6 件の dispatcher 回帰試験を再利用する。Qwen 優先の別実装は作らない。reviewer/CoS/sticky の対象範囲も維持する。main に入ったことと本番昇格済みであることは区別し、本番稼働版はこの task では確認・変更していない。

## 検証（この ADR 作成 task）

結果はこの task の最終 HEAD で確認する。今回の変更は ADR と本進捗だけで、Rust/GUI/web の実装試験は対象外。ADR §10 の新しい試験名は後続 Phase の受け入れ契約であり、合格済みの試験ではない。

| 検査 | 結果 |
| --- | --- |
| ADR 必須語句（ModelProfile / SourceState / RoutingPolicy / RoutingContext / QualityEstimator / Phase 5 / ADR-0069 / ADR-0132 / shadow / (a) / (c)） | exit 0 |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0 |
| `sh scripts/dev/check-doc-links.sh`（新規ファイルを index に登録して検査） | exit 0 |
| `sh scripts/dev/progress-index.sh --check` | exit 0 |
| `git diff --quiet $(git merge-base HEAD main) -- crates/ gui/ web/` | exit 0、コード差分なし |
| `git diff --cached --check` / `cargo fmt --all -- --check` | exit 0 |

後続では移行手順 `docs/ops/model-routing-migration.md` を Phase 1 で新設し、各 Phase で更新する。今回の task は ADR の範囲に留め、本番 config/DB/service/release を操作しない。
