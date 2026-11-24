---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: pilot-design
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 8 pilot-design: 対照 pilot の手順の ADR と進捗の骨組み

## 結論

- [ADR 2026-11-24-strategy-pilot-ablation](../../adr/2026-11-24-strategy-pilot-ablation.md) に pilot の手順を決めた。コード・script・fixture は変えていない。
- task の進捗 [2026-11-24-strategy-v1-8](../2026-11-24-strategy-v1-8.md) を status: running で作った。
- LLM の run はしていない。研究ケースの本文は repo に写していない（`research_case.py verify` で sha256 と slice を確かめただけ）。

## 受け入れ条件ごとの証拠（基準は `$CELERIS_WU_BASE`）

| 条件 | 証拠 |
|---|---|
| 0. ADR が (1)〜(9) を決め、3 方式の既存の入口（file・関数）を挙げる | ADR の D1（3 方式の表と D1.1〜D1.5。`review_task`・`start_side_review`・`parent_evaluation::{launch, request, settle, facts}`・`check_evaluation`・`allowed_actions`・`parent_checkpoint_section`・`validate_document`・`critic_subjects`・`critic_section`・`strategy_task_creates`）、D2 時点、D3 上限と承認の json、D4 漏れ、D5 盲検、D6 指標、D7 合格線、D8 入力の修正、D9 機密、D10 prompt。挙げた関数と試験の名は `git grep` で実在を確かめた |
| 1. task の進捗（status: running）と WU の進捗がある | `agent-docs/progress/2026-11-24-strategy-v1-8.md`・この file |
| 2. ADR に方式 × 時点 × 反復の run 数と USD の上限がある | ADR D3.3（(a) 0・(b) 12・(c) 28・予備 8、計 48 run・24.0 USD 以下、上限 25） |
| 3. crates/・control-ts/・web/・scripts/ を変えていない | `git diff --quiet "$CELERIS_WU_BASE" -- crates/ control-ts/ web/ scripts/` → exit 0（「検査」） |
| 4. 評価器の入力を eval-input・eval-seed で直してから測ること、人の答えがある | ADR D8 と「人の答え（2026-11-24、記録済み）」、背景の節 |

## 決めた点の根拠（確かめたこと）

- 子 6 の driver（`scripts/dev/parent-eval-blind.sh`）は時点を E-mid・D-done に限り、arms を `parent-eval-blind` で検査する。arm-driver が a/b/c と 3 時点に置き換える（ADR 実装前 brief の 6）。
- 承認の門 `pilot_gate.py check-approval` の `limits` は `max_usd`・`max_wall_secs`・`max_quota_pct` だけを受ける。run の数（48）は driver が数える（ADR D3.2）。
- task の `Budget` の欄は `max_turns`・`max_wall_secs`・`max_retries`・`max_tokens` で、USD の上限は controller の `max_usd_per_day`（一時 daemon ごと）で掛ける（ADR D3.1）。
- 子 6 の評価の run の実測: D-done の 1 回目 0.1371973 USD、4 run の計 0.487 USD（子 6 の進捗）。版を書く run と Critic の実測は無いので 1 run 0.5 USD を上側の見積りにした。
- 子 6 の盲検の seed の `StrategyRef` は名目の参照（文書の本体は無い）なので、方式 (b) はこれを使い、方式 (c) は v1 の文書を作業 tree に置く（ADR D1.2・D1.3）。
- `celeris.strategy/1` は候補 2〜8 件（子 5 D1.3）。方式 (c) の v1 は `selected` の無い版（`open` 2〜3）で、実験段階の gate に掛かる。
- 時点の材料: `python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/research/research_case.py verify` → exit 0（7 slice、否定対照 1 件を検出）。
  `python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/replay/replay.py verify` → exit 0（365 file 一致、12 slice・漏れ 0、否定対照 N1〜N3 を検出）。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ control-ts/ web/ scripts/` → exit 0（コード・script の差分 0）
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0。変更 path は ADR・task の進捗・この file の 3 件だけ
- `sh scripts/dev/progress-index.sh --check` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。repo health は verdict=pass、new 0・worsened 0、source-size は doc_only で not_applicable）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未確認の事項

- ADR の「未確認」の 6 点（方式 (a) の局所の review の再実行、方式 (c) の一時 daemon での流れ、研究ケースの発火、版を書く run の費用、盲検の限界、研究ケースの不要な実行の数え方）。
- 評価器の入力の修正（eval-input・eval-seed）は並行の WU で、この WU の時点では入っていない。

## 提案

- 無し。
