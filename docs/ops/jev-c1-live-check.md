# Jev 副比較（B1/C1）: C1 実 daemon での段階別実適用検査（人が実行する手順）

---
tasks: [01M59W8DFMVT3M8WRM4XP8ZEWE]
---

- 対象: Jev 副比較（B1=Celeris+固定方策、C1=Celeris+Jev。現行コードの arm 名は B=fixed / C=decision_assisted）の C1 を、**測定専用の実 daemon** で通常経路に通して確かめる検査。
- 実行者: 人。本番 host の操作・daemon の起動/再起動・鍵の登録・実 LLM の実行は、この文書の手順どおり人が行う。worker・agent は実行しない。
- **主比較（A0/B0）の依存先ではない。** B/C のみの実行は task 1 の実験セット選択で行い、この検査が未完了でも主比較・主比較用 pilot・論文の問題設定/設計/評価方法の執筆は待たない。
- 禁止（この手順で守ること）: 本番 SQLite / KB の直接編集、API キー値の表示・記録（有無と fingerprint の有無だけを見る）、`confidence` を成功確率・propensity として扱うこと、未測定の効果・有意性を書くこと。
- 参照: 棚卸し `agent-docs/progress/2026-10-19-jev-sidecomp/inventory.md`、段階別集計 `.../stage-agg.md`、variant 固定 `.../variant-pin.md`、合成試験 `.../synth-e2e.md`、測定環境の起動 [measure-run-final.md](measure-run-final.md)。

## 0. 区分表（現時点。効果・有意性は未測定で、ここには書かない）

| 区分 | 項目 | 根拠 |
|---|---|---|
| 実装済み | Jev 分類段（dispatcher の外）、lane 層（引き上げのみ。明示・許可/天井・利用可否が勝つ）、task 単位 routing 固定と祖先継承、Jev 消費記録、decision log への run 投影、campaign export、`jev_smoke` 判定器、段階別集計 `jev-stages`（`crates/campaign/src/jev_stages.rs`） | inventory.md (1)、stage-agg.md |
| 無 LLM 検証済み | 分類→lane 提案→制約適用後 lane→実行 model が通常の tick で届き、段階別件数が合うこと（合成 case、mock 分類）。B1/C1 variant の hash 固定・差は方策のみ・根拠表不使用・子/孫/retry 継承（in-memory store） | `jev_synth_e2e.rs`（1 passed）、`variant_pin_b1c1.rs`（7 試験）、`jev_stages.rs`（5 passed）。合成の結果は効果ではない |
| 実 pilot 未確認 | 実 Jev API の分類**成功**（観測は client_error / not_attempted のみ）、Jev の版が記録に残ること、実 daemon の子/孫 run で routing pin と `lane_assist` が維持されること、**実 export の検証器合格**（`jev-live-evidence.py` は用意済みだが、実 daemon の DB・実 export での実行は 1 度も行っていない＝未検証）、実費用（cash）の記録 | live-smoke.md 再実行 6〜7、inventory 不足 1・2・7・8 |
| 任意・延期 | ceiling/availability 抑止の合成 case、候補集合 2 モデル以上の検査の本番 preflight 化、B1/C1 を A なしの別 manifest として宣言する形、lane 変更と exec model 変更の連結集計、Jev token 数・CoS usage の記録 | synth-e2e.md 未実施、variant-pin.md 不備、inventory 不足 4・5・6・10・11 |
| 承認待ち | 実 LLM を伴う本検査の実行（残予算の確認と人の実行許可）、予算の増額、大規模再測定、論文投稿・外部送信 | 起票は自動開始の承認ではない |

## 1. 事前確認（読み取りのみ）

1. **残予算**: 最新の承認枠と既消費を確認する。過去の参考枠は cache 込み全体 300 万 / per_case 30 万 / per_arm 130 万 / 12 試行以内（`measure-run-final.md` 冒頭、人の決定 2026-10-16）だが、**現在有効な枠は人が最新の決定で確認する**。既消費は campaign の台帳（`final-accounting`）と `celeris-campaign usage` の出力から読む。
2. **残量が足りない場合**: 副実験（この検査を含む B1/C1）から縮小する（試行数を減らす → 1 case にする → 実行しない）。主比較の枠を削って副実験に回さない。縮小しても不足なら実行せず、人への通知待ちにする（予算の増額は人の新しい決定が要る。本検査は増額の承認ではない）。
3. 測定専用 daemon（本番と別の port・db・workspace、`push=false`）の用意は [measure-run-final.md](measure-run-final.md) §0〜§2 のとおり（H1〜H5）。本番 DB・本番 `~/.config/celeris` には書かない。

## 2. C1 の実 daemon で Jev enabled・認証・モデル版を確認する（管理 API / CLI のみ）

測定 daemon の API（`--api-url` と token は launcher の env ファイル由来。token の値は表示しない）に対して、読み取りだけを行う。

| 確認 | 手順 | 合格の見方 |
|---|---|---|
| Jev enabled | `GET /api/v1/config`（または `celerisctl` の config 表示）で `[jev] enabled = true` と routing mode が decision_assisted（C1）であること。daemon の config を直接編集して確かめない | enabled=true。B1 の daemon/task は fixed で、分類段が呼ばれない |
| 認証 | `GET /api/v1/secrets` で `jev_api_key` の `updated_at` と `fingerprint` が null でないこと。**値は読まない・書かない・チャットに貼らない**。未登録なら GUI『アカウント』で人が登録する（Jev を要求する work unit は credential 待ち→通知→登録→自動再開になる） | 登録済み。401/403 の fallback（`credential`）が出ていない |
| モデル版 | 分類が 1 件以上走った後、decision log の Jev 呼び出し行と `routing_decided` の `lane_assist.classifier_status` を `celerisctl campaign export <campaign_id> --out <dir>` で読み出し、Jev の model/snapshot が欠測でないか見る | **現状 `model_snapshot` は記録に書かれない（inventory 不足 1）。欠測なら「版は未確認」と報告し、推測で埋めない** |
| 実 API 成功 | `classifier_status.status` が `jev_ok` の件数 | `not_attempted`（tick/dispatch 競合）と `fell_back_to_fixed{reason}` は成功に数えない。`real_api_verified` 定数は証拠にならない |

## 3. 実行（残予算内のみ）

- B/C のみの実行は、task 1 の実験セット選択（A0/B0 とは別の測定 ID）で行う。本タスクは主比較の実行・開始条件にならない。
- 起動・照合・実行は [measure-run-final.md](measure-run-final.md) §5 の launcher（`prepare` → `start` → `preflight` → `watch`）をそのまま使う。`preflight` は有料実行 0 件で、ここで予算内か（`within_budget`）を確かめてから `watch` に進む。手で `--api-url` / token を書かない。
- C1 を単一モデル・明示 lane に pin しない（template に `tier` / `assignee` / `routing_override` を書かない。実行器が拒否する）。根拠表は使わない（`evidence_mode=none`）。B1/C1 は候補集合・階層対応・ハーネス・予算・初期情報・grader を同一にし、方策だけを変える。
- 実行中に予算の残りが枠に近づいたら、新規の試行を始めず停止し、人へ通知して待つ。

## 4. 実 export の検証（`jev-live-evidence.py`）と jev-stages の段階別集計

### 4.1 別測定 ID・条件固定・累計予算（実行前に守ること）

- B1/C1 は A なしの別 manifest（schema `celeris.campaign-manifest-side-b1c1/1`、別測定 ID = `campaign_id`。ADR 2026-10-19-jev-b1c1-side-manifest）で宣言する。A0/B0 の manifest・hash は変わらない。
- 共通条件（候補 Sonnet+Opus、階層対応、harness、予算、初期情報、grader）は hash で固定される。B/C の差は mode（方策）だけで、それ以外の差・根拠表の使用（`evidence_mode=none` 以外）・候補が単一モデル・明示 lane/tier/model の pin は `validate` / `check` が拒否する。
- 条件を変えて hash が変わっても、同じ `campaign_id` で同じ親 dir に並ぶ out の既消費（確定+予約）は承認総枠へ合算される。総枠に達していれば実行前に拒否される。別の親 dir に out を分けると合算されないので分けない。`final-accounting` 側への合算は未実装で、実行前の予算 gate のみ。

### 4.2 実 export の検証（人が実行。worker は実行しない）

**検証したか**: この文書を書いた時点で、実 daemon の DB・実 export に対する実行は **していない（未検証）**。検証器の試験（`python3 -m unittest discover -s scripts/dev/tests/jev_live_evidence`、87 件）は手組みの test double（`tests/jev_live_evidence/builder.py` が作る SQLite と export）で通るだけで、実証ではない。実データが得られるまで、この検査は「未検証」と報告し、試験の合格を「実検証済み」と書かない。

#### 信頼境界

- **信頼の根は 1 つだけ**: 運用者が evidence とは独立に設定する環境変数 `CELERIS_JEV_SOURCE_DB`（測定 daemon の SQLite の**絶対 path**）。検証器は SQLite URI `mode=ro` + `PRAGMA query_only=ON` の単一読取 transaction で開き、元 DB は変えない。evidence 内の `source` / path / command は読まず、実行もしない。
- **権限**: 運用者はこの DB を**読み取りだけ**できる権限で用意する（測定 daemon を止めるか、停止後の copy を ro で置く）。本番 DB・`CELERIS_DB` / `CELERIS_RUN_DB` と同じ file は信頼元にならない（拒否）。evidence dir の中の DB も拒否。未設定でも既定 DB へは暗黙に接続しない。
- **照合される側**: evidence の `records.jsonl` / `summary.json` / `provenance.json` / `routing_events.jsonl` は全て「照合される側」。検証器は読取元の原記録から export を決定的に投影し（`scripts/dev/jev_source_reconcile.py`。`crates/task-core` の export と同じ変換）、**ID（campaign_id → task → run → decision / usage / routing_decided の run_id）で全件・全欄**照合する。重複・欠落・余剰（別 campaign 混入）・改変・未知欄・順序違いは不合格。
- **証明にならないもの**: `provenance.json` の `source` / `files_sha256` / `export_command` / `generated_at`、ULID 形式の task_id、自分で計算できる hash、`fixture` / `synthetic` の有無の宣言、`real_api_verified` 定数。manifest hash は読取元の `task_campaigns` の値だけを使う。
- **run への結び付け**: routing_decided は event の `run_id`、Jev の消費は `usage.run_id`、Jev の応答版はその run の `decision_log_runs.classifier_snapshot`。task だけで結ぶ補完、他 run の版・費用での補完はしない（run に結べなければ未検証）。

#### 手順

1. 実 daemon の実行後、`celerisctl campaign export <campaign_id> --out <dir>` で `records.jsonl` と `summary.json` を取り出す（events・DB は読むだけ）。
2. 同じ `<dir>` に `provenance.json`（許される欄は `source` `campaign_id` `manifest_hash` `generated_at` `export_command` `files_sha256` `tasks`。`tasks` は `{task_id: {parent}}` の task 木）と `routing_events.jsonl`（1 行 `{task_id, run_id, event}` のみ。`routing_decided` の写し）を添える。現行の `celerisctl campaign export` はこの 2 file を出さない。**手で作った値は信頼しない**: どちらも読取元と全件照合され、読取元と 1 欄でも違えば不合格になる。読取元の抽出は `python3 scripts/dev/jev_source_reader.py --campaign <campaign_id> --out <file>`（同じ `CELERIS_JEV_SOURCE_DB`、読み取りのみ）で確認できる。この 2 file を原記録から作る専用の生成器は未実装（未解決）。
3. `<dir>` を `/local/celeris/data/workspaces/01M59KY5FPEWVNW0DT07HZRA3Q/artifacts/jev-live-evidence` に置き、運用者が信頼元を設定して、次を**この行のとおり**実行する。
   ```sh
   export CELERIS_JEV_SOURCE_DB=/abs/path/to/measure-daemon/celeris.db   # 運用者が設定。ro で開ける測定 daemon の DB
   python3 scripts/dev/jev-live-evidence.py --evidence-dir /local/celeris/data/workspaces/01M59KY5FPEWVNW0DT07HZRA3Q/artifacts/jev-live-evidence --require-live
   ```
   標準ライブラリのみ・外部通信なし。段階別 report（JSON）は stdout、判定は stderr。`--quiet` で report を省く。
4. **終了コード（0 以外は全て live 不合格。合格として扱わない）**:

   | exit | 意味 |
   |---|---|
   | 0 | 原記録と全件一致し、live 合格 |
   | 1 | 検証不合格（`FAIL [code] …` が stderr。不一致・混入・pin/候補逸脱・欠測・費用不一致・被覆不足・入力が読めない） |
   | 2 | live 証拠なし（evidence dir が無い・空・必須 4 file の欠落）、または信頼元なし（`CELERIS_JEV_SOURCE_DB` 未設定・相対 path・到達不能・開けない・daemon の DB と同一・evidence の中） |
   | 3 | 読取元の schema 不明（`schema_migrations` が既知の版でない）・必要な表/欄が無い・campaign 不在。**欄不足は成功にならない** |

   `--require-live` なしは「原記録と全件一致（--require-live なし）」で、実 task ID・Jev 認証成功・被覆まで要求しない。live 合格として報告しない。
5. **結果保存**: stdout の report（`--quiet` を付けない）を JSON として、stderr の判定と exit code を text として、同じ dir（`<dir>/../jev-live-evidence-result/` など evidence の外）に保存し、進捗ファイルへ要点（exit code、各段の件数）を書く。読取元 DB の path・sha256 と、実行日時・検証器の git commit を一緒に残す。API キー・token の値は保存しない。

### 4.3 live 合格の条件

`--require-live` で次をすべて満たすこと（すべて原記録との ID 照合の上）。
- 読取元が運用者設定で、到達でき、既知 schema で必要欄が揃っている。evidence の 4 file が原記録の投影と全件一致している。
- 混入なし: 全行が同じ campaign_id・読取元の manifest_hash。arm は B/C のみ（A 不可）で、B は fixed・C は decision_assisted。task_id は実 task の ULID。
- **各 trial ごとの条件**（root・子・孫・retry の全 run）: `proposed_candidates` が凍結 manifest の Sonnet/Opus 共通候補（`source_id` / `model_id` / `harness_id` の 2 件。重複・未知 ID・単一モデル・別 harness を拒否。arm 全体の和集合では判定しない）、`proposed_first` が候補内、実効 pin（`source` / `lower_policy` / `evidence_mode`=none / `evidence_table_id` / `online_update`=false）が root と一致し B/C で mode 以外同一、`exec_model` が欠測でなく候補内（fallback 理由があれば別）。C1 の `lane_assist`・明示 lane などによる抑止。
- C1: Jev の認証成功（`jev_ok`、同じ run の原記録で）、応答版（`classifier_snapshot`）、各段件数、全役割（cos/planner/worker/reviewer/jev）と retry の費用が summary と records で一致。
- 被覆: root・子・孫・retry が揃っていること。
- 欠測（費用・版・exec model）は欠測のまま数え、成功に数えない。

### 4.4 live 合格にならないもの・fixture と実証の区別

- **fixture は実証ではない**: `tests/jev_live_evidence/builder.py` の手組み SQLite/export は test double で、検証器の回帰試験専用。これを evidence として渡しても、`CELERIS_JEV_SOURCE_DB` が測定 daemon の実 DB を指さない限り信頼元にならない（未設定なら exit 2）。fixture と同じ内容を自前で作った DB に向けて合格させても、実 daemon の証拠とは認めない。
- 自己申告だけ（source 名・ULID 形式の task_id・再計算した hash・fixture 宣言の有無）の入力、export 後に改変した file、別 campaign/別 hash/arm A/B1 と C1 の混入、pin 不一致、単一モデル・明示 lane・根拠表の使用、費用の不一致、被覆不足、fake 成功（`not_attempted` や fallback を成功扱い）、run に結べない版・費用。
- review で再現された 2 件の偽陽性は回帰試験で拒否される: (1) 手組み fixture の自己申告だけでの live 合格 → `test_live_source_contract.py`（`NoTrustedSource` / `SelfDeclaredInput`: 信頼元なしは exit 2、再 hash した自己申告は非 0）、(2) C1 の子 r2 の候補を Sonnet 1 件・exec_model を Sonnet にした入力 → `test_per_run_conditions.py::test_reviewer_repro_c1_r2_single_sonnet_exec_sonnet`（`test_each_depth_retry_and_b_side_single_sonnet` が各深さ・retry・B 側へ展開）。いずれも test double 上の試験で、実 export での検証ではない。

### 4.5 段階別集計 `jev-stages`

検証器の stdout report は `jev_stages` と同じ段定義（分類試行・jev_ok・lane 提案変更・制約適用後 lane の変更・model 変更・抑止理由別・最終成果/再試行/全費用・`*_missing`）で、原記録から組まれる。**各段の件数、欠測（`*_missing`）、抑止理由別の件数、全役割（Jev を含む）と retry の費用を人が確認する**: 欠測が 0 でなければ「欠測あり」と報告し、成功に数えない。`jev-stages` 自体を使う場合の入力は `stage-agg.md` の形で、記録に無い欄は null/省略のまま渡す。
```sh
export CARGO_TARGET_DIR=...            # 渡された値のまま
cargo run -p celeris-campaign -- jev-stages --records <records.json>
```
`confidence` は成功確率・propensity として読まない。件数差は記述するだけで、有意性・効果の主張はしない。

## 5. 実 daemon での variant・継承の確認（読み取り）

- 実行した task の `campaign.manifest_hash` が B1/C1 で同じ共通部分の hash を指すこと、方策差（mode）以外が同一であること（`celeris-campaign check` / `validate`）。
- 子・孫・retry の `routing_resolved` / `lane_assist` が root の pin を維持していること（inventory 不足 7。mock 試験のみで、実ログは未確認）。維持されていなければ「未確認/不一致」と書いて報告し、ここで本体を直さない。

## 6. 報告に残すもの

条件・実行したコマンド・出力の要点（件数、exit code）を、進捗ファイルに書く。API キー値・token は書かない。確認できなかった欄（版・実 API 成功など）は「未確認」と書く。効果・有意性は、測定が済むまで書かない。
