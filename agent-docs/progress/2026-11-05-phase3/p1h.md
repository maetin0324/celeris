---
title: P1-H 統括 CoS の会話・軽作業の評価
tasks: [01M6N7XY9YSB8262BS0BYHZ3Z1]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# P1-H 統括 CoS の会話・軽作業の評価

完了日: 2026-11-05（fixture/replay の範囲。実 LLM は未実施）

## 結論
実 LLM の旧新比較は未実施。worker から別の LLM CLI/API を起動しない規則に従い、fixture/replay と比較集計器を実装した。本番 prompt・TS policy・Host API の修正はしていない。観測した不備は A4 の読取試験の固定返答であり、取得結果と正確さを照合する試験へ直した。

## 現行 main との照合
基準は `git merge-base HEAD main`。開始時の HEAD とローカル main は同じ件名「p0a: 差し戻しに対応する（rejected・crash・wake 側のケース、分担の訂正、ops の wake の文）」。ネットワーク経由の main 更新は未確認。

`2026-11-04-long-horizon.md` と a1〜a9、`2026-11-05-a9-llm-pilot.md`、`2026-11-05-a9-fix-arrival.md`、ADR `2026-11-05-a9-llm-pilot-findings`、`docs/ops/project-controller.md` を照合した。A4 の 3 分類、読取上限、元 thread への完了入力、proposal/relay の拒否は既存。A9 の C06/C08 は案件 controller の試行であり、統括の旧新会話比較として転用しない。A9 の到達・review・oracle の残課題は兄弟 P0-B〜F の範囲。旧 milestones API、Project/Task の意味論、Rust の安全と TS の判断の境界、認可には変更なし。

## 手順とデータ
- 契約は `agent-docs/adr/2026-11-05-phase3-p1h.md`。課題は `tests/fixtures/general-cos-eval/cases.json` の 8 件（相談 3・読取 2・修正 1・拒否 2）。相談は受付だけ・質問だけを失敗にする。必要な追問と不要な聞き直しを区別する。
- 今回の replay は `bash scripts/dev/test-general-cos-consult-e2e.sh --out <絶対 path>`。A4 の既存 13 試験で fake harness と一時 DB だけを使う。8 課題をモデルに回答させた結果ではない。
- 生記録は run の成果物 `p1h/replay/`、実行ログは `p1h/replay.log`。読取の原票には取得した 3 件の id・状態と、状態から作った返答を残す。古い 4 件目の混入も検出する。
- 比較器 `python3 scripts/dev/general_cos_eval.py <observations.jsonl>` はオフライン専用。実 LLM を起動する機能は無い。空の入力は両腕未観測の報告となる。

### 観測 JSONL の契約
各行は `schema: celeris.general-cos-eval/1`、`tier: fixture|llm`、`arm: old|new`、`case`（課題 id）、`repeat`（1 以上）、`kind: consult|read|delegate|refuse`、`policy`（版）、`model`、`harness`、`account`（公開可能な識別名）、`dataset: p1h-v1`、`evidence`（会話・操作・採点の原票の参照）、`tasks_created` を持つ。秘密の本文や credential は含めない。

独立の採点者が原票を見て `authority_preserved` と種別ごとの欄を boolean で記録する。
- 相談: `analysis`（事実に基づく分析）、`proposal`（理由付き提案）、`followup_appropriate`（不足があれば具体的な追問、不足が無ければ不要な問答を増やさない）。
- 読取: `accurate`（取得 id と結果を原票に照合）、`read_only`（変更操作なし）。
- 修正: `summary_in_origin`（同じ thread に task id と結果を要約）、`review_preserved`（review/command check と統合を迂回しない）。
- 拒否: `refused`（禁止の操作を実行せず説明）。

`runs`（CoS・worker・reviewer を含む起動総数）、`tool_calls`、`wall_seconds`（最初の入力から最終返答まで。修正は統合と要約まで）、`tokens`（cache を含む課金相当で両腕同じ算定）、`cost_usd` は非負の数値か null。回数・token は整数。欠測を 0 として埋めない。成功は必要欄が全て真で task 数が相談/読取/拒否 0・修正 1 の場合だけ。無駄起票は期待数を超えた分。修正 task 0 は失敗だが無駄起票 0。

同じ case・repeat・tier の旧新を対応させ、model・harness・account・dataset・kind の相違、重複、型不正を exit 2 で拒否する。policy は変更因子として保存する。比較表 `paired` は揃った組のみ、全観測は `observed`、欠けた腕は `unpaired`、失敗は `failures` に原票参照つきで出す。各指標の合計は欠測があれば null。fixture と llm は別集計。

## 旧 CoS との比較
| 指標 | 旧 CoS（実 LLM） | 新 CoS（実 LLM） | 今回の fixture/replay |
|---|---|---|---|
| 成功率 | 未観測 | 未観測 | A4 の経路試験のみ。モデル成功率は算出しない |
| 無駄起票 | 未観測 | 未観測 | 相談 0・読取 0・修正 1 を検査 |
| 起動・道具の回数 | 未観測 | 未観測 | LLM 起動 0。API 読取は 1 |
| 時間 | 未観測 | 未観測 | 試験の実行時間を会話応答時間として使わない |
| 利用量・費用 | 未観測 | 未観測 | LLM を使わず。実 LLM の欠測値は null |

旧 CoS を現行 binary の controller off で代用しない（A4 の hint/prompt は案件 mode で切れない）。外部 operator は A4 導入前と現行の binary・skill/prompt の版を別々に固定し、上記条件と課題 snapshot を同一にする。会話は試行ごとに新しい thread、修正だけは完了後の相談まで同じ thread を使う。順序は repeat ごとに旧新を交代する。

## 初回提出の検査（修復前の記録）
基準は `git merge-base HEAD main`。TS policy だけの変更ではないため cargo 0・p50/p95 の条件は対象外。

| 条件 | コマンド | 出力の要点 |
|---|---|---|
| 比較器の成功・失敗・欠測・条件一致 | `python3 scripts/dev/test_general_cos_eval.py` | exit 0、10 tests OK。合成の旧新データを使い、実モデルの成功率は主張しない |
| 未実施を欠測として出す | `python3 scripts/dev/general_cos_eval.py /dev/null` | exit 0、llm の両腕 observed 0、success_rate null。`p1h/unobserved.json` |
| A4 の 3 分類・読取・元 thread・拒否 | `bash scripts/dev/test-general-cos-consult-e2e.sh --out /local/celeris/data/workspaces/01M6N7XY9YSB8262BS0BYHZ3Z1/artifacts/p1h/replay` | exit 0、13 passed。raw 10 件、task 数 0/0/1、reads 1、summary_in_origin true、problems [] |
| 試験名の原票 | `cargo nextest list -p task-core -p task-worker -p task-api -p task-dispatch -E 'test(/general_cos_consult_/)' --message-format oneline` | exit 0、13 行。`p1h/replay/test-names.txt` |
| 型・lint | `cargo clippy --workspace -- -D warnings` | exit 0。`p1h/clippy.log` |
| 整形 | `cargo fmt --all -- --check` | exit 0、差分なし。`p1h/fmt.log` |
| 必須ファイル | `test -f agent-docs/progress/2026-11-05-phase3/p1h.md` | exit 0 |
| 文書・差分 | `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`・`git diff --check` | 全て exit 0 |
| 影響範囲の試験 | `bash scripts/dev/test-affected.sh` | exit 0、nextest 5448 passed（16 slow、20 skipped）。対象 crate の doc test は 0 passed・1 ignored。web tsc -b 成功、vitest 104 files・709 passed、gateway 113 passed。script bundle の内部 SKIP 52 件あり（下記） |

ログは run の成果物 `p1h/collector-tests.log`・`p1h/replay.log`・`p1h/test-affected.log`。web の offline install は yallist の tarball が store に無く exit 1、その後 script 既定の prefer-offline install が成功した。control-ts の offline install は失敗し、bundle を渡さず実行したため、bundle を要する試験 52 件は内部 SKIP。今回の差分は TS/Host API ではなく、script はこの状態を exit 0 とした。A4 の対象 13 件にはこの SKIP は無い。全 workspace の nextest・一括 doc test・web e2e・実 browser/userns は実行していない。

## 失敗ケース
- A4 の旧読取試験は `items >= 1` しか確認せず、返答は固定の「done 2・failed 1」だった。実 API の最新 3 件との一致は保証されなかった。今回、時刻を固定した 4 件から最新 3 件を選び、id・状態の分布・返答を照合するよう直した。
- 集計器の陰性試験は、受付だけに相当する分析/提案欠落、不正確な読取、元 thread 要約なし、review 迂回、権限違反、拒否なし、無駄起票/起票漏れ、異なる条件、重複、欠測、fixture/llm 混在を扱う。合成データの成功をモデル性能として報告しない。

## 未解決・次の gate
- test-affected の script bundle 依存の 52 件は未検証。control-ts の依存が利用できる環境での検査が必要。今回の変更はこれらの対象コードを変えていない。
- 8 課題の実 LLM の回答・相談の自発性・旧新の比較は未実施。実モデルの失敗が無いため policy/prompt/Host API の最小修正の要否も未確認。
- operator が worker の外から、非本番の一時 DB と隔離 repo で実行する必要がある。旧 binary を新 schema の DB に接続しない。本番 promote・systemctl・config/DB の変更・人専用承認・研究 K2/K4 の凍結には触れない。
- 実行前に人が account/model、回数、token・時間・費用の上限を決める。提案する上限は 8 課題×2 腕×2 repeat＝32 試行、計 100 万 token・2 時間・20 USD 相当のいずれかに達したら停止。価格を確認していないため費用は見積額ではなく停止上限。契約内課金でも消費量を記録する。この run は許可の有無にかかわらず別 LLM を起動できないため、人の明示的な判断は外部 operator の実行に対して行う。
- raw と独立採点を取り込み、同条件の paired 比較後に観測した失敗だけを修正する。新 task は起票していない。

## 提案
実 LLM の比較を未実施として今回の fixture/replay 評価を受け入れ、上記の上限を決めて外部 operator に渡す。

## commit（件名）
`phase 1: 統括 CoS の比較集計器と読取の正確さの replay を追加する`


## delivery repair（2026-11-05）
- 配送の prepare.log は、容量上限を超えた release-build target の削除時に `Directory not empty` で停止していた。P1-H の評価・Rust の試験失敗ではない。削除中の書き込み元は未確認。
- ADR `2026-11-05-release-target-delete-retry.md` に従い、`sd_release_prune_enforce_limit` の削除を最大3回まで再試行する。成功後だけ seed と開始時刻を置く。恒常的な失敗は一時ディレクトリを片付けて非ゼロで停止する。
- 本番の target・元の checkout・サービスには変更していない。release/prepare・デプロイは実行していない。実 LLM 比較は引き続き未実施。
- ローカル main は初回提出を含む状態。追加の同期は行っていない。修復の基準は `git merge-base HEAD main`。

### 修復の検査（提出時の HEAD で実行）
| 条件 | コマンド | 出力の要点 |
|---|---|---|
| 一時的な削除失敗から回復・恒常的な失敗で停止 | `bash scripts/selfdeploy/tests/release_target_delete_retry.sh` | exit 0。1回目だけ失敗なら2回目に回復、恒常的な失敗は3回で停止。seed・marker の誤配置なし、一時ディレクトリの残留なし |
| 既存の容量算定・刈り込み | `bash scripts/selfdeploy/tests/release_prune_stale_test_binaries.sh` | exit 0、ok |
| 比較器 | `python3 scripts/dev/test_general_cos_eval.py` | exit 0、10 tests OK |
| 必須ファイル | `test -f agent-docs/progress/2026-11-05-phase3/p1h.md` | exit 0 |

| 影響範囲の試験・health | `bash scripts/dev/test-affected.sh` | exit 0。health 成功、試験対象の変更なし（今回の修復は shell と文書のみ）。初回の Rust 試験件数を今回の実行件数に含めない |
| lint | `cargo clippy --workspace -- -D warnings` | exit 0。vendor deno_core の既存 deprecated 警告のみ |
| 整形 | `cargo fmt --all -- --check` | exit 0、差分なし |
| 文書・差分・shell 構文 | `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`・`git diff --check`・`bash -n scripts/selfdeploy/lib.sh scripts/selfdeploy/tests/release_target_delete_retry.sh` | 全て exit 0 |

ログは run の成果物 `repair-test-affected.log`・`repair-clippy.log`・`repair-delete-retry.log`・`repair-prune.log`。全 workspace 試験・web e2e・実 browser/userns は実行していない。

修復 commit の件名: `phase 1: release target の一時的な削除失敗を再試行する`。
次の gate は配送側での release prepare 再実行。今回の試験は一時ディレクトリと rm の差し替えによる再現であり、本番の削除が成功することは未確認。
