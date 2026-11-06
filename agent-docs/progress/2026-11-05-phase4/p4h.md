---
title: P4-H P2 統合・再評価・証拠ベースの closeout
tasks: [01M6PAMC47G5XRZZVJXYPG626G]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# P4-H P2 統合・再評価・証拠ベースの closeout

完了日: 2026-11-06。第 4 フェーズ親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の子（人の依頼 seq 2343）。基準は `git merge-base HEAD main`（この task の branch は main と同じ先頭から始まった）。
製品のコードは変えていない。設計判断は無いので ADR は足していない。

## 結論
- **B〜G と追加の修正 task はすべて main に入っている。** 統合の作業（merge・衝突解消）は要らなかった。下の「統合の確認」の 10 本の branch の先端は全部 main の祖先。
- **最新の main で scripted の比較を再採点した。** oracle 合格は new 10/10・old.passive 3/10・old.proactive 8/10・ctl.null 2/10 で、P0-C の表と同じ。replay の raw は committed の baseline と一致（`raw-diff.log` が空）。
  変わったのは new の `controller_achieved` だけで 8/10 → 9/10（P4-B の C10 の修正の効果。C06 は人が任意の o2 を却下した妥当な停止で 1 件残る）。
- **実 LLM の観測は new 腕の pilot だけ（2/2 を 2 回）。** 旧腕（old.passive）の実 LLM、統括 CoS の旧新の実 LLM 比較（P4-E）、案件 CoS の usage が入った実 LLM の pilot（P4-G の G7）は未実施。
- **人の承認が無いので次をしていない（未実施）:** 実 LLM の評価、24〜72 時間の継続稼働、本番の shadow、release promote・systemd・本番 config。full release gate（`scripts/selfdeploy/release.sh`）は `~/.local/celeris/releases` に書くので worker は回さず、手順と段の名前の照合だけをした。
  これらを「実運用で成功した」証拠として扱わない。

## 統合の確認（`git merge-base --is-ancestor refs/heads/celeris/<id> refs/heads/main`、全部 exit 0）
| 子 | task | 内容 | main の commit（件名の例） |
|---|---|---|---|
| P4-A | 01M6PAMC3F809GDHJA5H9T1TXZ | 棚卸し・契約・性能の baseline | `p4a: test-affected の host 依存の失敗を進捗に書く` |
| P4-B | 01M6PAMC3JDQ9KKVQZSJBVZ1W3 | 達成の後の根拠の失効・Roadmap の checks（migration 0115） | `p4b: 試行の上限で止まった Outcome を Rust の検証を通る root が満たしたら再開する（C10）と replay の raw の更新` |
| P4-C | 01M6PAMC3PDP1DR5MR0W5ZRV0E | blocked が多いときの dispatch の遅延 | P4-C の進捗の「変更」節 |
| P4-D | 01M6PAMC3S9QN740JWEQTXV60J | wake・判断・compare の記録の保持と GC（migration 0117） | `p4d: retention GC は 3 表とも最大 id の行を残す（compare の id の使い直しで script guard が違反を見落とすのを防ぐ）` |
| P4-E | 01M6PAMC3XBVDS7C2TP3WGD2E5 | 統括 CoS の旧新 paired 比較の準備 | `p4e: USD の上限で試行を止め、承認者を human: の主体に限る` |
| P4-E seed | 01M6PZ88JMRZ52BP46GVK9Y1EN | tier=llm の seed の段の修正 | `p4e: seed の段は tier に依らず stub の harness にし、fake 以外の adapter の run があれば失敗にする` |
| P4-F | 01M6PAMC40X023V30F4T9QH2GW | 実 daemon の非本番 soak | `p4f: 26 分・4 条件の実 daemon soak と before/after の raw、集計の窓と RssAnon の傾き、ADR と進捗を足す` |
| P4-G | 01M6PAMC44JQ6DB90E9HQ076DW | 1 案件の限定 shadow の道具と予行 | `p4g: 本番 1 案件の限定導入の go/no-go・承認の分離・監視・差分・戻しの道具と非本番の予行の raw、ADR・手順書・進捗` |
| cos-usage | 01M6PYNMP4EFRX9C5SGT6XB97E | 案件 CoS の usage null の原因（空の `--cos-route`） | `phase4 cos-usage: 実 LLM pilot の --cos-route が空なら起動を止め、再実行の raw と ADR・進捗を足す` |
| P0-F usage | 01M6P8HSXJ | pilot driver の usage の配線（第 3 フェーズ） | `p0f-usage: pilot driver の usage に CoS chat run を含め key を記録に合わせる` |

## 研究用の比較表（実測と未実測を分ける）
| 比較 | 段 | 腕 | 結果 | 状態 | raw |
|---|---|---|---|---|---|
| long-horizon bench v2（10 課題、仮想時刻、LLM なし） | scripted replay | new | oracle 10/10、controller_achieved 9/10、avoidable 0、無駄起票 0、double effect 0、不一致 0 | 実測（この run、最新 main） | [p4h/rescore](p4h/rescore/) |
| 同上 | scripted replay | old.passive | 3/10、autonomous_reach 0.30、avoidable 10 | 実測（同上） | 同上 |
| 同上 | scripted replay | old.proactive | 8/10、autonomous_reach 0.90、avoidable 1、無駄起票 1 | 実測（同上） | 同上 |
| 同上 | scripted replay | ctl.null（対照） | 2/10、reach 0.18 | 実測（同上） | 同上 |
| 縮小 pilot（C06・C08） | 実 LLM | new | 2/2 pass、不一致 0、in+out 13,953 token（CoS を除く）、wall 273 秒 | 実測（P0-F、operator が実行） | `agent-docs/progress/2026-11-05-phase3/llm-v2/` |
| 同上の再実行 | 実 LLM | new | 2/2 pass、不一致 0。案件 CoS の 14 run は usage null（`--cos-route` が空） | 実測、ただし CoS の消費は未計測 | `agent-docs/progress/2026-11-05-phase4/cos-usage/llm-rerun/` |
| 同上 | 実 LLM | old.passive | — | 未実施（人が許可していない） | — |
| 同上（`--cos-route` 付き、CoS の usage 込み） | 実 LLM | new | — | 未実施（P4-G の G7、人の承認待ち） | — |
| 統括 CoS の旧新 paired（32 試行） | stub（LLM 0） | old / new | 決定的な採点 20 試行で旧 0.8・新 1.0（差は委譲の `summary_in_origin` だけ）。相談 12 試行は人の採点待ち | 実測（stub）。モデルの性能の差は示さない | `agent-docs/progress/2026-11-05-phase4/p4e/stub/` |
| 同上 | 実 LLM | old / new | 有効な観測 0。operator の 1 試行（read-three の旧 r1）は seed の段の不具合で失敗し、修正済み | 未実施（有効な比較なし） | `agent-docs/progress/2026-11-05-phase4/p4e/llm-pair1/` |

「scripted new 10/10」は台本の CoS・worker・人で動かした決定的な replay の値で、実 LLM の性能ではない。実 LLM で旧腕より優れることは示していない。

## before/after（各子の実測。値は各進捗から写した）
| 項目 | before | after | 出典 |
|---|---|---|---|
| new の controller_achieved（scripted） | 8/10 | 9/10（この run の再採点で確認） | P4-B、`p4h/rescore/summary-vs-p0c.diff` |
| dispatch の遅延 p50、blocked 600 件 off / shadow | 3,617 / 5,395 ms | 272 / 299 ms | P4-C |
| 同 2,000 件 off / shadow | 21,567 / 17,965 ms | 763 / 727 ms | P4-C |
| 同 10,000 件 off / shadow | 未測（30 分超の見込み） | 3,252 / 4,018 ms（目標 500 ms は未達） | P4-C |
| 30 日・100 万行の GC（wake / 判断 / compare） | 582,960 / 582,960 / 49,968 行 | 199,872 / 235,960 / 12,079 行（効果のある判断は全部残る） | P4-D |
| 実 daemon 26 分の RssAnon の増加 | 本番 +338,880 kB 以上 | 非本番 4 条件 +16,760〜19,440 kB（本番の増加は再現せず、原因は未確認） | P4-F |
| 限定導入の予行（非本番） | — | 期待 20 件 ok、この run の再実行で shadow の監視 19 標本・違反 0、restart 0.22 秒 | P4-G、`p4h/pc-rollout/summary.json` |

## この run で実行した検査（提出時の HEAD で実行）
| command | exit | 要点 |
|---|---|---|
| `bash scripts/dev/test-long-horizon-bench.sh --out <artifacts>/bench`（1 回目） | 1 | v8 の build script の出力が古い path を指し `could not find native static library rusty_v8`。製品の不具合ではない |
| `cargo clean -p v8` の後、同じ command（`--out <artifacts>/bench2`） | 0 | replay 40 passed、grade 24 passed、contract 9 passed、raw の差なし（`raw-diff.log` 空）。CLI の採点と grades.jsonl 一致 |
| `bash scripts/dev/test-ts-verify.sh` | 0 | 20 passed。TS だけの編集は cargo 起動 0 回のまま |
| `cargo nextest run -p task-api --test lifecycle -E 'test(milestone_lifecycle_endpoints_are_gone)'` | 0 | 1 passed。旧 milestones API は 410 のまま |
| `bash scripts/dev/pc-rollout.sh --out <artifacts>/pc-rollout` | 0 | 非本番の一時 daemon・fake harness。期待は全部 ok、canary の preflight は短い shadow の報告で no-go（G3・G5） |
| `python3 -m unittest`（pc-rollout 16・daemon-soak 15・general-cos-compare 25） | 0 | 全部 OK |
| `python3 scripts/dev/source-size-report.py` | 0 | 手書きの本番の行 2110（閾値 2000）、warning 3・例外 1（既存の値。この run は変えていない） |
| release gate の段の名前の照合（`docs/ops/project-controller.md` の checklist 1 の 11 段を `release.sh` の `run_step`・`web_step` で grep） | 0 | 11 段とも release.sh にある |
| `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` | 下の「提出時の検査」 | |

logs の末尾は `p4h/logs/`。secret の grep（`sk-ant`・`Bearer`・`accessToken`・`refreshToken`）は `p4h/` で 0 件。

## full release gate の手順（人が実行）
1. `git merge-base --is-ancestor <sha> refs/heads/main` が 0。
2. `scripts/selfdeploy/release.sh <sha>`。段は cargo-workspace-clean（共有 target が別の tree なら）・cargo-fmt-check・control-ts-install・control-ts-bundle・script-bundle-verify・cargo-test（`scripts/dev/test-parallel.sh`、全体の試験と userns の試験）・script-bundle-tests・cargo-clippy・source-size-report・cargo-build・web-config・web-pnpm-install・web-pnpm-typecheck・web-pnpm-test・web-pnpm-release。確認は `<release>/gate.json` に `failed_step` が無いこと。
3. `scripts/selfdeploy/verify.sh <sha12>`、`celerisctl release preview <sha12>` で migration（0115・0117、SCHEMA_VERSION 117）と config 欄を読む。
4. promote は migration を含むので stop-start。`backup ok:` を確かめる。戻しは `rollback.sh --restore-db`（`docs/ops/project-controller.md` の checklist 2）。

この run は 1〜4 のどれも実行していない。

## 失敗ケース・edge
- bench の 1 回目は v8 の build の不整合で exit 1（上の表）。`cargo clean -p v8` で直る既知の現象。
- P0-C の committed の `grade-v2/summary.json` は 8/10 のまま。P4-B は facts を更新したが grade-v2 を作り直していない。bench script は raw だけを比べるので落ちない。第 3 フェーズの記録は書き換えず、今の値を `p4h/rescore/` に置いた（差は `summary-vs-p0c.diff` と `grades-vs-p0c.diff`、C10 new の 1 行）。
- 10,000 件の dispatch の遅延 p50 3.3〜4.0 秒は目標 500 ms を満たさない（P4-C の残り、`store.list(None)`）。
- 本番の RSS の増加の原因は未確認（P4-F）。

## 未解決
1. 実 LLM: old.passive の pilot、`--cos-route` 付きの pilot（CoS の usage 込み、G7）、統括 CoS の旧新 32 試行は未実施。
2. token の上限を input+output と cache 込みのどちらで数えるかは人の未決（P0-F・P4-E）。
3. 本番の限定 shadow は no-go (pending)（P4-G）。24〜72 時間の非本番 soak と本番の process_memory の読み取りも未実施（P4-F）。
4. P4-D の trigger は最大 id の行を守らない（GC の関数だけが守る）。P4-B の人・CoS の root（outcome_ids なし）は checks だけの改版で根拠のまま。
5. 10,000 件の dispatch の遅延（上）。

## 次の人の判断
1. G7: `--cos-route` 付きの縮小 pilot（C06・C08 の new、各 1 試行）を許可するか。許可するなら account・model と上限（token の数え方を含む）。
2. P4-E: 統括 CoS の旧新の実 LLM 比較の可否・account・上限（32 試行・100 万 token・2 時間・20 USD が候補）・相談の採点者。
3. release: 人が `release.sh` を最新 main で回し、gate.json を確かめてから promote するか。
4. P4-G: G7 の後、本番 1 案件の observe（dry_run・shadow）承認を出すか。
5. P4-F: 24/72 時間の非本番 soak を許可するか。

## 提案
- P0-C の `grade-v2` は facts を変える task が同じ commit で作り直すようにし、bench script が summary も committed と比べるとよい（今は raw だけを比べるので、P4-B の後に grade-v2 が古いまま残った）。

## 提出時の検査
（下に追記）
