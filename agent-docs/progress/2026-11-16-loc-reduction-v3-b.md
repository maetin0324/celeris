---
title: LoC 削減 v3 B — worker への目的の伝達と reviewer の純減判定
tasks: [01M7J3NKV5R4XFVSDEWDQ4MF22]
status: done
updated: 2026-11-16
completed: 2026-11-16
---
# LoC 削減 v3 B — worker への目的の伝達と reviewer の純減判定

完了日: 2026-11-16

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 B。設計は [ADR 2026-11-16-reduction-goal-delivery](../adr/2026-11-16-reduction-goal-delivery.md)。

## 結論
- 配送経路の欠陥は 1 つだった。計画のある task の WorkUnit の run は、task の目的を先頭 1,500 文字しか受けていなかった（`work_unit_prompt_context`）。削減の依頼の目的では、削除の候補・基点の commit・目標・禁止事項・不変条件が 1,500 文字より後ろにあり、WU の worker に届かなかった。全文を渡すように直した。
- 汎用規約（`change_rules/generic.md`）は全 run kind に届いていたが、行数の純減に触れていなかった。規約 11 を足した（純減を求める task だけ。計画は候補・基点・目標・禁止事項・不変条件と測定 command を写す、実装は消した対象と before/after を書く、review は純増・不変・移動だけと測定の欠損を合格にしない）。
- 純減の決定的な判定は、受け入れ条件の size check（command check）が持つ。落ちると reviewer は起動されず、reviewer の合格と health の pass では覆らない（既存の `review_decision`）。repo の測定の書き方は `CLAUDE.md` に 1 行で書いた。
- 新しい Rust の型・gate・config 欄は足していない。本体（production）の純増は 0。

## 変更（commit の件名）
- `loc-reduction-v3 B: WU に task の目的の全文を渡し、削減の目標を規約 11 で planner・worker・reviewer に届ける` — `WorkUnitPromptContext.task_objective_excerpt` を `task_objective`（全文）に置き換え（protocol・dispatcher・prompt の見出し・worker-protocol の文書と schema）、generic.md の規約 11、CLAUDE.md の測定の 1 行、ADR、6 例の試験。
- `loc-reduction-v3 B: 進捗ファイルを足す` — この file。

## 6 例の試験（`crates/task-dispatch/src/health_review/reduction_flow_tests.rs`）
本物の dispatcher が planner → WU → 統合 → final review を一時 git repo で回す。size check は repo の本物の `scripts/dev/source-size-report.py` を base の commit から `git show` で取り出して数える。試験の adapter（LLM を呼ばない偽の worker）は、dispatcher が渡した `RunRequest` から本物の `claude_code::build_prompt` で各 run の prompt を組んで記録し、reviewer は全 criterion を合格・health の finding 0 件と `review.json` に書く（甘い reviewer）。health の検査器の stub は verdict pass。

| 例 | 試験 | 結果 |
|---|---|---|
| (1) 増分・health pass | `reduction_flow_growth_with_health_pass_fails_review` | size check exit 1、criterion 0 と reviewer の criterion は不合格、reviewer run 0、done にならない |
| (2) 真正の削減 | `reduction_flow_real_reduction_passes_and_delivers_the_goal` | size check exit 0、reviewer 合格、done |
| (3) 移動だけ | `reduction_flow_move_only_fails_review` | 10 行を別 file に動かすと純減 0 で size check exit 1、done にならない |
| (4) 測定の欠損 | `reduction_flow_missing_measurement_fails_closed` | 基点に測定 script が無く exit 128、削減が真正でも done にならない |
| (5) 通常の機能追加 | `reduction_flow_feature_task_is_not_held_to_net_reduction` | 本体が増えても done |
| (6) 目的・差分の配送 | (2) の試験の後半 | planner・WU・reviewer の prompt に候補・基点の SHA・目標・禁止事項・不変条件（1,500 文字より後ろ）と規約 11、reviewer に size check の結果（pass=true と command）・target_sha・reviewed_sha・消した行（`+0・-10`）が届く |

変異の確認: WU の目的を先頭 1,500 文字に戻すと (2) が `the work unit prompt lacks "削除の候補 C9: …"` で落ちた（提出前に元に戻した）。

## 行数の実測（受け入れ条件 3 の size check。開始時 main の script と toml で数えた）
| 指標 | 開始時 main 415a19a7004cb6a73b872e861fa8d1cf634dddbb → HEAD | unit base（git merge-base HEAD main）→ HEAD |
|---|---:|---:|
| production 全言語 | 純減 0（386,338 → 386,338） | 純減 0 |
| production Rust | 純減 0（340,223 → 340,223） | 純減 0 |
| own_total_with_tests | 純増 412（試験 external_test 461,753 → 462,165） | 純増 412 |
| docs | +36 | +36 |

- 固定と unit の差は 0。unit base は開始時 main に A の進捗ファイル（docs）を足しただけで、main に混入した無関係の変更は無い。
- own_total の +412 は 6 例の試験の file。親の実測に全額を計上する。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo nextest run -p task-dispatch -E 'test(reduction_flow)'` → 5 passed
- `bash scripts/dev/test-affected.sh` → exit 0（6170 passed、22 skipped。対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 受け入れ条件 3 の size check（閾値 `0 - - - -`）→ exit 0、`OK []`

## 未解決
- 規約 11 の review の決まり（移動だけの削減を見抜く）は reviewer の LLM の判定に頼る。決定的に止めるのは size check の閾値だけで、閾値を 0 にした削減の task では移動だけの変更（純減 0）が size check を通る。削減の task の閾値は目標の行数（1 以上）で書く必要がある。
- `FakeAdapter`（`fake` の id の subprocess の adapter）は prompt を組まないので、試験は偽の adapter から本物の `build_prompt` を呼ぶ形（既存の `health_review_flow_` と同じ）にした。

## 提案
- 親の計画（C 以降の子）は、受け入れ条件の size check の閾値を各子の目標の行数で書く（`0` は純増を止めるだけで、移動だけの変更を止めない）。
