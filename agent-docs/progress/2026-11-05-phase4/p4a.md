---
title: P4-A 残課題の棚卸し・契約・性能のベースライン
tasks: [01M6PAMC3F809GDHJA5H9T1TXZ]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P4-A 残課題の棚卸し・契約・性能のベースライン

完了日: 2026-11-05。第 4 フェーズ親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の子（人の依頼 seq 2343）。基準は `git merge-base HEAD main`。
ADR は `agent-docs/adr/2026-11-05-phase4-p4a-baseline-contract.md`（D1 所在と担当、D2 不変条件と試験の対応、D3 計測の口、D4 B/C の分担）。

## 結論
- 先行の確認: 第 3 フェーズの子 10 件と P0-F usage の修正 01M6P8HSXJ は main に入っている（main の先頭は `p0f-usage: pilot driver の usage に CoS chat run を含め key を記録に合わせる`）。第 3 フェーズ親の要約 `agent-docs/progress/2026-11-05-phase3.md` は親の branch（`celeris/01M6N7WD1WXXW0A5FV7RVZ67B7`）にだけあり、main には未取り込み（親 closeout の配送待ちと推定、未確認）。
- **dispatch の遅延の原因の段を特定した。** blocked 600 件で dispatch の遅延 p50 4,684〜4,824 ms のうち、`tick_cos_chat_launch` の段が 4,572〜4,718 ms を占める。中では `triage_events`（`cos_chat/triage.rs`）と `triage_fallback`（`cos_chat/fallback.rs:459`）が `task_ops::human_inbox::human_inbox`（`crates/task-ops/src/human_inbox.rs:264`）を 1 tick に最大 2 回作り直す。100 件で 1 回約 190 ms（約 1.9 ms/件）。`store.list` は 600 件で 34 ms で、遅延の 1% に満たない。P4-C はここから着手する。
- 計測の口を足した: `TickReport::step_us`（`dispatcher/tick_steps.rs`）と試験 `crates/celeris/tests/tick_scale_baseline.rs`。同じ command で C の before/after を取れる。
- 達成の誤り（G1）・Roadmap の checks（G2）・C06/C10（G3）は所在と事実を ADR D1 に固定し、P4-B に渡した。wake/判断の肥大（G5）・実 LLM の統括 CoS 比較（G6）・本番 RSS（G7）は所在と既存の数値を固定した。
- 実 LLM・本番・長時間の試験はしていない（「未実施」節）。

## 棚卸し（実 file・関数・query）
| # | 残課題 | 事実（この run で確認） | 担当 |
|---|---|---|---|
| G1 | 達成後の根拠の reopen で誤達成のまま | `project_guard.rs` の `pre_decide` 1 段目が `phase == Achieved && !human` で `achieved-quiet` を返し判断しない。decide〜commit の間の reopen だけは `project_root_event_mark` と `StaleReason::Evidence` で止まる（P0-B attempt 2）。commit の後の reopen を wake にする口は無い | P4-B |
| G2 | Roadmap の Outcome に checks が無い | `roadmap.rs` の `RoadmapOutcomeSpec` は id・title・acceptance・priority・depends_on・dropped だけ。`project_proposal.rs` の Outcome block は `checks: Vec<OutcomeCheck>` を持つ | P4-B |
| G3 | replay v2 の new C06・C10 は oracle pass で `controller_achieved false` | `p0c/replay-v2/facts/`（trial は `p0c/replay-v2/arm-map/` で case=C06・C10、arm=new の 2 件）。C06: o2:1 の root は `ready→running→cancelled`、`questions ["root_failed:o2:1"]`、承認の人の操作は 259,500 秒と 522,900 秒。C10: o1:1・o1:2 は failed、o1:3 は CoS の instructed root（`created_by cos`、86,400 秒の人の回答の後）で done、`questions ["attempts_exhausted:o1"]`。推定（未確認）: C06 は o2 の公開が却下され到達しないので停止は妥当、C10 は no-progress の blocked が instructed root の done で解けない | P4-B |
| G4 | blocked に比例する dispatch の遅延 | 下の「ベースライン」 | P4-C |
| G5 | wake・判断の行の肥大 | `0112_project_controller.sql` の `project_wakes`・`project_controller_decisions`、`0104` の `script_compare_log`。P1-I の仮想 72 時間（20 案件）で wake 1,860 行・判断 1,440 行、削除の仕組みは無い | P4-D（C の後） |
| G6 | 実 LLM の統括 CoS の旧新比較なし | `scripts/dev/general_cos_eval.py`（オフライン、実 LLM 観測 0）、`scripts/dev/test-general-cos-consult-e2e.sh` | P4-E（人の承認後） |
| G7 | 本番 RSS 182→589 MB/26 分の原因が未確認 | P1-I の非本番の傾きは本番の 8.1〜9.5%（全区間）、V8 heap 最大 2.9 MB | P4-F（人の承認後） |

## ベースライン（G4）
command（`CARGO_TARGET_DIR` は Celeris が渡した値。一時 DB、fake harness、LLM なし）:
`TICK_SCALE_RAW=<abs>/tick-scale.jsonl TICK_SCALE_BLOCKED=0,600,2000 TICK_SCALE_TICKS=3 TICK_SCALE_DISPATCHES=5 cargo nextest run -p celeris --test tick_scale_baseline`
（対照は `TICK_SCALE_NATURAL=1 TICK_SCALE_BLOCKED=100`）。raw は `p4a/*.jsonl`（`celeris.tick-scale-baseline/1`）。

| raw | 作り方 | blocked | 最初の tick（ms） | 空 tick p50（ms） | `store.list`（ms） | dispatch 遅延 p50 / 最大（ms） | dispatch 中の `tick_cos_chat_launch` p50（ms） |
|---|---|---|---|---|---|---|---|
| `tick-scale.jsonl` | 複写 | 0 | 9 | 4 | 2.4 | 90 / 122 | 29 |
| `tick-scale-natural.jsonl` | 本物の dispatch | 100 | 404 | 5 | 5.8 | 673 / 728 | 593 |
| `tick-scale-clone-check.jsonl` | 複写 | 100 | 732 | 227 | 5.8 | 642 / 658 | 585 |
| `tick-scale.jsonl` | 複写 | 600 | 3,868 | 1,245 | 34.5 | 4,824 / 5,116 | 4,718 |
| `tick-scale-head.jsonl`（提出時の HEAD） | 複写 | 600 | 3,832 | 1,209 | 33.8 | 4,684 / 4,766 | 4,572 |
| `tick-scale.jsonl` | 複写 | 2,000 | 16,517 | 8,860 | 117.6 | 15,943 / 19,217 | 15,711 |
| `tick-scale-10k.txt` | 複写 | 10,000 | - | - | - | 600 秒以内に終わらず（nextest の terminate） | - |

- P1-I の 26 分の計測（655 件で p50 3,953 ms、`store.list` 47 ms）と同じ桁を数十秒で再現した。dispatch 1 件あたり tick は 2 回。
- 複写の妥当性: dispatch の遅延と dispatch 中の段は本物の対照と一致した。**空 tick と最初の tick は一致しない**（本物 5 ms・404 ms、複写 227 ms・732 ms）。原因は未確認なので、空 tick の数値は本物の対照だけを使う（未解決 1）。
- 段の内訳（`triage_ingest` の中、一時的な計測。commit していない）: 100 件・本物で 1 tick 平均 `triage_events` 139 ms・`triage_fallback` 72 ms（最大 191 ms）・初回の `triage_reconcile` 最大 196 ms。どれも `human_inbox` の組み立てで、他の段は 1 ms 未満。
- 条件の違い: この試験の config は `[cos]` なし（CoS 無効）で、P1-I と同じ。本番は CoS 有効で、`triage_fallback` の候補の条件（`unavailable_after_secs`）が違う。本番の件数での遅延は未確認（未解決 2）。
- P4-C の目標: 同じ command で 600 件の dispatch 遅延 p50 ≤ 500 ms（before 4,684〜4,824 ms）。2,000・10,000 件の曲線も同じ command で出す。

## 変更（commit の件名）
- `p4a: tick の段ごとの時間と blocked の数のベースライン試験、残課題の契約の ADR を足す`
- `p4a: tick の段の計測を tick_steps.rs に移し、旧い粗い lap を段の列に置き換える`（source-size gate の `dispatcher.rs` 2,000 行の上限に合わせ、main より 10 行減らした。`slow tick phases` の警告は `total_ms` と `steps_us` を出す）
- `p4a: 進捗とベースラインの raw を書く`

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0。対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch、3,422 passed・10 skipped（`tick_scale_baseline` の既定 0,20 件を含む）。
- 1 回目の `test-affected` は exit 1 だった: source-size gate `crates/task-dispatch/src/dispatcher.rs production_size value=2492 threshold=2000 allowed=2412`。2 番目の commit で直した。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check`・`sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh` → 下の「文書の検査」。
- `test -f agent-docs/progress/2026-11-05-phase4/p4a.md` → exit 0。

## 失敗ケース
- 10,000 件の計測は nextest の 600 秒の打ち切り（exit 100）。`TICK_SCALE_TICKS=1 TICK_SCALE_DISPATCHES=2`。P4-C は test binary を `--exact tick_scale_baseline` で直接起こすか、件数ごとに分けて測る。
- 複写の DB の空 tick の食い違い（上）。

## 境界の確認
- 実 LLM・有料 API・本番の DB/config/daemon・systemctl・promote・本番 shadow・24〜72 時間の稼働はしていない。測定は一時 DB と fake harness（`sh`）だけ。CPU を焼く負荷はかけていない（負荷は DB の行の数だけ）。
- 旧 milestones API の 410（`milestone_lifecycle_endpoints_are_gone`）・human_required・許可の保護・TS と Rust の分担・K2/K4/IEEE Access の凍結評価には触れていない。TS の policy・bundle は変えていない。

## 未実施（人の承認が要る）
- P4-E の実 LLM の統括 CoS の旧新比較、P4-F の 24〜72 時間の非本番 soak と本番 RSS の比較、P4-G の本番 shadow。どれも範囲・費用・account・時間・停止条件を人に示して承認を得てから行う。

## 未解決
1. 複写の DB で空 tick が本物より遅い原因（受信箱・退避の表の行の状態の違いと推定、未確認）。
2. CoS 有効（本番）の config での遅延の内訳は未計測。
3. G3 の C06・C10 の判定（妥当な停止か不具合か）は P4-B が replay を再現して決める。上の推定は facts だけからのもの。
4. 第 3 フェーズ親の要約 `2026-11-05-phase3.md` が main に無い。

## 次の gate
- P4-B と P4-C は並列に着手できる（ADR D4 の file の持ち主、migration の予約 B=0115・C=0116・D=0117）。P4-D は P4-C の後。
- P4-C は `human_inbox` を 1 tick 1 回に共有するか差分で作る案から、受信箱の内容が前後で同じことを試験で固定して着手する（提案）。

## 提案
- `human_inbox` の組み立てを tick の中で 1 回にし、`triage_events` と `triage_fallback` で共有する。さらに触れた task だけの差分に寄せる（P4-C）。
- `tick_scale_baseline` に `[cos]` 有効の条件を足し、本番に近い内訳を取る（P4-C）。
