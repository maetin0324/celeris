---
title: "[監査修正6c] 既存承認の残予算内で最小 pilot を実経路で確認（部分実行・no-go）"
tasks: [01M542RGFR4WHVCFBPRMKWTD79]
status: done
updated: 2026-10-17
completed: 2026-10-17
---

# 監査修正 6c: 最小 pilot の実経路確認 — 部分実行（A 腕のみ）、本測定は no-go

**結論: 実経路で確かめられたのは A 腕の実行だけ。B/C・grader 結合・`--verify-connected`・Jev は未検証。本測定は開始しない（no-go）。**
人が release `05a398f1efc9` を昇格し（H1。celeris/web/credentiald とも 05a398f1efc9）、運用 shell で §5 の手順 2〜9 を実行した（mlsys-pilot-2、2026-10-17 05:20Z〜）。
消費は 487,628 tokens（cache 込み all）。pilot 上限 900,000 の内側で、人の決定により本測定の承認枠（3M・12 試行）の内側に数える（残り 2,512,372・9 試行）。
ただし A の 1 試行は per_case 300,000 を 187,628 超えた（事後に cancelled）。B/C は POST /tasks の 422 で 0 tokens・未作成。
見つかった欠け 6 件は修正 task 01M5456VVZR2VC5FC6P5J0V12Q で直し、その完了後に pilot をやり直す（§6・§8）。
大規模本測定の開始・再実行・予算増額はしていない。この branch のコード変更はない（文書と pilot 条件だけ）。

## 1. 着手時の再確認
- 着手時の branch base は 173988bb。origin/main は 05a398f1（6b: 通し受け入れ試験）まで進んでいたので fast-forward で取り込んだ
  （`git merge --ff-only origin/main`）。6b は `campaign_acceptance_e2e` 7 件・nextest 5595 passed（6b 進捗）。
- 本番 daemon は `celeris@289d220c1555`（run 環境の `MEMORY_PRESSURE_WATCH` の unit 名）。`git log 289d220c1555..05a398f1` は 38 commit で、
  daemon 側の修正を含む: `75caf0b2`（POST /tasks で campaign 識別を同一 transaction 保存）、`b4552084`（Jev 呼び出しを call_id で一意に記録・費用欠測は NULL）、
  `aaa5705a`（runs[].usage と決定ログの合算規則）、`b430d0cb`（routing 固定の子孫継承と実測照合）、`8a36631b`（CoS/介入の投影）。
  **migration の差分は無い**（`git diff --name-only 289d220c1555 HEAD -- crates/task-core/migrations` が空。両方 0086 まで）。
  → 今の本番で pilot を走らせると「修正前の daemon の出力」になり、受け入れの根拠にできない。新しい release の昇格が要る。
- 01M52D2AM5K6GCFGEJZW4REWM2（本測定実施）は task 上は done だが、記録は **not-run**（`2026-10-16-mlsys-final/result.md`・`pending.md`・`operator-run.md`）。
  `data/` は README.md のみで final-out・final-export・grade は存在しない。実測の成果として扱わない。
- 前回 pilot `mlsys-pilot-1`（2026-10-16 14:35–15:00Z、運用セッションが実施、出力 `/local/celeris/data/scratch/mlsys-pilot/`）は修正前の release 5effbca11c70 で、
  input+output 基準・grade 未実施。summary: A 2/2 passed、B 1 passed + 1 cancelled（tokens 欠測 1）、C 1 passed、`tokens_charged_total` 107,649（欠測 1 件を per_case 100k で計上）、
  cache 込みでは約 563k。**修正前の出力なので最終結果にも 6c の証拠にも使わない**（較正の参考のみ）。

## 2. worker が実行しなかった理由（worker の権限の外。実行は人が §5 で行った）
1. **release**: 修正（上記）を含む release が本番に無い。昇格は人の作業（指示どおり手順だけ書く。§5 H1）。
2. **A 腕は campaign CLI が `claude` を直接起動する**（`crates/campaign/src/main.rs` の `HarnessExecutor` + `ClaudeCodeBuilder`、`--a-program` 既定 `claude`）。
   worker の中から実行すると worker が LLM CLI を起こす形になり禁止（ADR 2026-10-07-worker-no-subagents-no-llm-cli）。B/C の POST /tasks も
   `CELERIS_CAMPAIGN_TOKEN` が要り、worker には渡されない（`env` で名前だけ確認、未設定）。
3. 本番 DB は開かない: `celerisctl routing evidence` は `--db` 必須で、本番 DB を指定しなかった（C の evidence_table_id は H1 で人が確認）。

## 3. pilot 条件（`2026-10-17-mlsys-audit-fix-6c/`）
| 項目 | 値 |
|---|---|
| campaign_id / split | `mlsys-pilot-2` / `dev`（本測定と別 id・別 split。較正専用、本測定の主張に混ぜない） |
| case | `k3-locusta-ab-analysis`（dev、cluster job 不要の analysis。grade-case `fixtures/grade-cases/dev/k3-locusta-ab-analysis/`、固定 hash `3216c548…`） |
| arm × 反復 | A / B / C × 1（試行 3）。primary A/B、secondary B/C subset = 同じ 1 case。model 全 arm `claude-sonnet-5-5`、fable なし |
| B / C routing | B: `fixed`・`lower_policy=mlsys-min-1`・`online_update=false`。C: `jev-1`・同 lower_policy・`evidence_table_id` は H1 で埋める |
| budget | token_basis `all`、max_tokens 900,000、per_case 300,000 / 3,600s、per_arm 300,000 / 3,600s、wall 10,800s、並列 1、trial_timeout 3,600s（case の wall 3,600s に合わせた） |
| 入力 | `pilot2-input-manifest.json`（sha256 `7eff17ec…36a1f5`）。材料は `$M/k3-locusta-ab-analysis/` の配置で `materials_sha256` `231623d0…ef1c6` |
| 未置換の placeholder | A `harness_version`、B/C `harness_version`（H1 の sha12）、C `evidence_table_id`、task-template の `project_id`。**残っている間は preflight が有料実行 0 件で止める**（確認済み、§4） |

### 予算の扱い（判断）
- 最新の既存承認は 2026-10-16 の人の決定: token_basis all、全体 3,000,000・per_case 300,000・per_arm 1,300,000、試行 12 以内、claude 7 日使用率 45% 以内、fable なし（`final.json`・`run-final.md`）。
  pilot 専用の別枠は旧決定（input+output 基準 1,000,000）しか無く、cache 込みで換算すると mlsys-pilot-1 で約 563k を使い残り約 437k。
  per_case 300k の予約 gate（fix 4）では残り 437k から 2 試行目以降が投入できないため、旧 pilot 枠では 3 arm の確認が成り立たない。
- そこで **pilot-2 は本測定の承認枠（3M・12 試行）の内側に数える**ものとして上限を置いた: 最大 900,000（3 × per_case）・3 試行。
  最悪でも本測定の残りは 2,100,000・9 試行で、本測定 6 試行の予約（6 × 300k = 1.8M）を下回らない。**枠の増額ではない**。
  実消費は dry-run の仮置き 151,590、mlsys-pilot-1 の実績からは 1 試行 11〜25 万（3 試行で 33〜75 万）を見込む。
- 本測定の `final.json` の `max_tokens` を「3M − pilot-2 の実消費」に下げるかは本測定の前に人が決める（今は変えていない。変えると final の hash が変わる）。

## 4. 無課金で確かめたこと（証拠）
`B=$CARGO_TARGET_DIR/debug/celeris-campaign`（05a398f1 で `cargo build -p celeris-campaign`）。`$O=$TMPDIR/p2` に placeholder を候補値
（B/C `05a398f1efc9`、A `claude-code@2.1.300` = `readlink ~/.local/bin/claude`、C の根拠表は mlsys-pilot-1 と同じ `118b413e…-2026-10-14`、project_id は仮）で埋めた写しを置いた。候補値は H1 で人が確定する。
- `$B validate --manifest agent-docs/progress/2026-10-17-mlsys-audit-fix-6c/pilot2.json` → exit 0、sha256 `4f80aea0…8551`（placeholder 入り。実条件の hash ではない）。
- 埋めた写し: `validate` → exit 0（`dabca7f8…65dcf`）。`dry-run` → exit 0、within_budget true、total_trials 3、推定 151,590 tokens、llm_calls_made 0。
- `preflight`（埋めた写し、`--api-url http://127.0.0.1:1` は形の検査用）→ exit 1、`paid_calls_made` 0、missing は **`CELERIS_CAMPAIGN_TOKEN` 未設定の 1 件だけ**
  （manifest・versions・input-manifest・case-inputs・graders・case-time・candidates・snapshot-src・budget は通過）。出力 `artifacts/pilot2-mock/preflight.json`。
- `preflight`（repo の雛形そのまま）→ exit 1: A の placeholder・token 未設定・task-template の project_id を不足として出す（未置換で走らないことの確認）。
- mock の通し: `run --executor mock` → executed 3、`check` → ok true・violations 空・complete、`grade-out --cases crates/campaign/fixtures/grade-cases/dev/k3-locusta-ab-analysis` → graded 3・missing_cases 空・final_pass 0、
  `export` → exit 0、`success_definition: executor outcome passed AND independent grade final_pass (bound by campaign hash, case id, split, grader hash, artifact hash)`、
  `evidence_kind mock`・`real_performance false`。mock の成果物は grader 不合格（A timeout、B/C failed）で成功 0。**mock の数値は実性能ではない**。出力 `artifacts/pilot2-mock/`。
- 途中で見つけて直した条件の不整合（pilot2.json のみ、コード変更なし）: case の wall 3,600s > per_case 1,800s、disk 見積り 2GiB > case の 1GiB、材料 hash の配置。

## 5. 人が実行する手順（運用 shell。worker・agent は実行しない）
記号: `B=<05a398f1 以降の build>/celeris-campaign`、`CT=~/.local/celeris/current/bin/celerisctl`、`P=agent-docs/progress/2026-10-17-mlsys-audit-fix-6c`、`O=<NFS 外の作業 dir>`。
1. **H1 昇格**: main（05a398f1 以降。この branch の取り込み後でもよい。差分は文書と pilot 条件だけ）の release を作り昇格する（GUI か `scripts/selfdeploy` の promote。migration 差分なし）。
   確認: `systemctl --user list-units 'celeris@*'` の sha12 が新 release、`GET /health` の `release`（または GUI の版表示）が同じ sha12。
2. **H2/H4 と使用量**: `GET /api/v1/secrets` で `jev_api_key` の fingerprint 有無（無ければ GUI `/accounts` で登録。値は貼らない）。自分の shell で `export CELERIS_CAMPAIGN_TOKEN=…`。
   `GET /api/v1/llm/accounts` で claude / codex の 7 日使用率の開始値 `U0` を控える（claude 45% 以内であること）。
3. **置換**: `cp $P/*.json $O/`。`$O/pilot2.json` の B/C `harness_version` を H1 の sha12、A を `claude-code@$(basename $(readlink -f ~/.local/bin/claude))`、
   C の `evidence_table_id` を `$CT --db <本番 DB の写し> routing evidence` の最新版に置換。`pilot2-task-template.json` の `project_id` を案件 id に。
   材料: `mkdir -p $O/M $O/K && cp -r crates/campaign/cases/dev/k3-locusta-ab-analysis/materials $O/M/k3-locusta-ab-analysis && cp -r crates/campaign/cases/dev/k3-locusta-ab-analysis/kb/. $O/K/`。
   k3 の原票 `main-43223.tgz`（sha256 `bdb155d4…`）は case.json の `inputs[].fetch` に従って用意する。
4. **固定と preflight**: `$B validate --manifest $O/pilot2.json`（sha256 を控える）、`$B dry-run …`（within_budget true）、
   `$B preflight --manifest $O/pilot2.json --api-url http://127.0.0.1:<task-api port> --token-env CELERIS_CAMPAIGN_TOKEN --task-template $O/pilot2-task-template.json --materials-src $O/M --kb-src $O/K` → **exit 0 でなければ止める**。
5. **実行（有料、3 試行）**: `$B watch --executor celeris` に 4 と同じ引数 + `--out $O/out` を付けて、台帳が閉じるまで（`waiting=0`）。stdout JSON を保存。
6. **検査・独立採点**: `$B check --manifest $O/pilot2.json --out $O/out`（ok true）、
   `$B grade-out --manifest $O/pilot2.json --out $O/out --cases crates/campaign/fixtures/grade-cases/dev/k3-locusta-ab-analysis`（graded 3、missing_cases 空）。
7. **export と接続確認**: `$B export --manifest $O/pilot2.json --out $O/out --dest $O/export`（成功 = executor passed AND grade final_pass）、
   `$CT db backup $O/db.sqlite3 --src <本番 DB path>` → `$CT --db $O/db.sqlite3 campaign export mlsys-pilot-2 --out $O/exp-db --verify-connected --manifest-hash <4 の sha256>` → exit 0。
8. **Jev と usage**: C の task 詳細 `GET /api/v1/tasks/<id>` の `routing_decided.lane_assist` が `jev_ok` か fallback か、Jev の消費行に費用（欠測は NULL）と版（model_snapshot）があるか、
   `routing_override`（B: mlsys-min-1・online_update false、C: jev-1）が子孫まで残るか。終了時の 7 日使用率 − `U0` を控える。
9. 結果（stdout JSON、`check`・`grade-out`・`export`・`--verify-connected` の exit と要点、tokens 合計）をこのファイル §6 に書き戻す。合計は 900,000 を超えない想定（gate と stop_on_budget）。実際は合計 487,628 で上限内、ただし A の 1 試行が per_case を事後に超えた（§6）。

## 6. pilot-2 の実行結果（人の運用 shell。2026-10-17）
詳細は運用者の記録 `/local/celeris/data/scratch/mlsys-pilot-2/RESULTS.md`、出力は同 dir の `out/`・`*.json`（本番 DB は読み取り専用の写しのみ）。
worker はこれらの file を読んで要点を確かめた（`watch-stdout.json`・`check.json`・`grade-out.json`・`export.json`・`verify-connected.err`）。

| 手順 | exit | 要点 |
|---|---|---|
| H1 昇格 | — | 本番 `celeris@05a398f1efc9`（`GET /api/v1/health` の release も 05a398f1efc9） |
| 置換 | — | A `claude-code@2.1.300`、B/C `05a398f1efc9`、C `evidence_table_id` `118b413e…-2026-10-14`（DB 写しの唯一の根拠表）。task-template の `project_id` は BenchFS 案件 `01M35WRV77A2JPYGERQGXF6V7K`（auto_integrate=false）にし `repos` を外した |
| validate | 0 | sha256 `dabca7f88d80f9106850315eec3cdfd2144b78ab666e55a6f129f7763af65dcf`（§4 の候補写しと同じ） |
| dry-run | 0 | within_budget true、total_trials 3、推定 151,590 |
| preflight | **0** | ok true、missing []、paid_calls_made 0 |
| watch `--executor celeris` | — | executed 3、tokens_total **487,628**、tokens_missing 0、case_budget_cancelled 1、executor_errors 2、max_overshoot_tokens 187,628、ledger cancelled 2 / done 1 |
| check | 0 | ok true、violations []、complete true、records 3 |
| grade-out | 0 | graded 3、missing_cases []、**final_pass 0**（全腕 recompute 不合格: 原票が `<workspace>/inputs/` に無い） |
| export | 0 | success_definition = executor passed AND grade final_pass。primary A/B・secondary B/C とも neither 1。complete true・real_performance true（2 試行が未実行なのに、§8 B3） |
| `campaign export --verify-connected` | **1** | `not connected: export is empty: no runs for this campaign`（B/C の task が無く、A は設計上 DB に載らない） |

- **A**: claude-code 2.1.300 / claude-sonnet-5-5 を campaign CLI が直接起動。14 turn・117 秒・0.275 USD。input 28・output 7,637・cache_read 441,342・cache_creation 38,621。
  report.md と results/table.csv を出したが合計が per_case を超え、事後に `cancelled(case_budget)`。
- **B/C**: `POST /api/v1/tasks` が HTTP 422（local workspace と remote repo `benchfs` の組合せ）。有料呼び出し 0、task 未作成。
  原因は運用者が auto-integrate on の agent-platform 案件を避けて BenchFS 案件に替えたこと。preflight はこの組合せを検査しない（§8 B2）。
- **Jev・routing_override 継承**: C が作られなかったので未観測。Jev の鍵は登録済み（fingerprint のみ確認）。
- **7 日使用率**: 開始 claude 0.39 / 0.37・codex 0.00、終了も同じ（差 0）。A は Celeris のアカウント外（運用者の `~/.claude`）で走るため、この差は A の消費を表さない。
- **B/C を再実行しなかった理由**: 残り 412,372 に対し B/C の予約は 600,000 で、A は予約を 187k 超えた。pilot 上限 900,000 を破りうるので止めた（人の判断）。

### 本測定の判定: no-go
k3 では原票が届かず全腕の grader が落ちる。A 腕は FS 隔離が無く、private grader の所在や過去の分析結果に届く。per_case 予算は A 腕を事前に縛らない。
これらが残ったまま本測定（6 試行・3M）を走らせても有効な比較にならないので、本測定は開始しない。修正 task 01M5456VVZR2VC5FC6P5J0V12Q の完了後に pilot をやり直す。

## 7. 区分（項目ごと）
| 項目 | 実装済み | mock 検証済み | 実経路検証済み | 承認待ち・次 |
|---|---|---|---|---|
| preflight gate（不足時 0 件で停止） | 32d7db7f | 本 task で pilot-2 の不足を検出（§4） | **検証済み**（実 API で exit 0・有料 0 件）。ただし project/workspace の組合せは見ない（B2） | B2 は 01M5456VVZR2VC5FC6P5J0V12Q |
| A 腕（claude-code 直接） | main | pilot-2 の mock run | **検証済み（実行のみ）**: 実 claude で成果物と usage（cache 込み・費用）を台帳に記録 | 隔離（G2）・事前の per_case 制限（B1）・使用量の可視化は修正 task |
| 予算 gate（予約・停止） | 9bb5a4a8・6b の checker 修正 | 6b `budget_stop_…` | **部分**: 事後の case_budget cancel と合計の上限内は実経路で動いた。A の事前制限は無い（超過 187,628） | 修正 task（B1） |
| 通常経路 POST /tasks → 子 task → 成果物（B/C） | 75caf0b2・b430d0cb ほか | 6b 通し 7 件 | **未検証**（422 で task 未作成） | 専用案件を作って pilot やり直し（修正 task 後） |
| 独立 grader の成功判定への結合 | 9576eb45（fix 1） | 6b・本 task の mock grade-out→export | **未検証**: grade-out→export の結合は実出力で動いたが、原票未配送（G1）で全腕不合格のため合格側の結合を確かめていない | 修正 task（G1）後の pilot |
| export `--verify-connected` | celerisctl campaign export | 6b `normal_path_…`・`campaign_connected.rs` | **未検証**（exit 1: B/C の run 無し） | 同上 |
| 未実行試行の complete 判定 | — | — | 実出力で欠けを発見（complete true・real_performance true を 2 試行未実行で出す、B3） | 修正 task |
| Jev の判断反映・usage・版 | b4552084、jev_smoke 判定器 | `lane_assist` 単体試験 | **未検証**（C 未作成）。作り直さず次の pilot で再観測 | 次の pilot |
| routing_override の子孫継承 | b430d0cb | 6b | **未検証**（B/C 未作成） | 次の pilot |
| pilot-2 の消費 | — | dry-run 151,590 | **実測 487,628**（≦ 900,000、本測定 3M 枠の内側に数える。人の決定） | 本測定枠の残り 2,512,372・9 試行 |

## 8. 未解決・提案
修正 task 01M5456VVZR2VC5FC6P5J0V12Q で直す 6 件（証拠は RESULTS.md §8）:
- G1 外部入力の配送: `case.json inputs[].fetch` の asset store も `--inputs-src` も無く、原票が workspace の `inputs/` に届かない（k3、たぶん本測定の k2/k4 も）。
- G2 A 腕の FS 隔離: A が `find /` で原票・case.json・`fixtures/grader-private` のある tree・過去の分析成果物に届き、`/tmp` に書いた。
- B1 A 腕の per_case 事前制限: 300k の予約に対し 487,628（事後 cancel のみ）。
- B2 preflight の task-template 検査と専用案件: project の repos と local workspace の組合せを preflight で検査する。auto_integrate=false の campaign 専用案件を用意する。
- B3 未実行試行の complete 判定: executor_error で run 0 の試行があるのに complete true・real_performance true。
- 使用量: A 腕の消費が `GET /llm/accounts` の 7 日使用率に出ない。

その他:
- 本測定の `final.json` の `max_tokens` を pilot-2 の実消費（487,628）分下げるかは本測定の前に人が決める（今は変えていない）。
- identity 側の未適用 patch の取り込み／破棄は本 task の範囲で確認していない（6b と同じく要確認）。
- 提案（§2 から継続）: A 腕も daemon 経由（run・quota の記録内、sandbox 付き）で起動する executor にすれば、G2・使用量・worker からの実行の問題がまとめて解ける。

## 証拠コマンド
- `git merge --ff-only origin/main` → 173988bb..05a398f1。
- `git diff --name-only 289d220c1555 HEAD -- crates/task-core/migrations` → 空。
- validate / dry-run / preflight / mock の run・check・grade-out・export → §4 のとおり。
- `bash scripts/dev/test-parallel.sh` → exit 0（nextest passed 5595, failed 0, ignored 15。§6 記入後に再実行しても同じ）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → ok。
- pilot-2（人の運用 shell）: validate 0・dry-run 0・preflight 0・watch tokens_total 487,628・check 0・grade-out 0（final_pass 0）・export 0・`--verify-connected` 1（§6）。
  worker は `/local/celeris/data/scratch/mlsys-pilot-2/` の `watch-stdout.json`・`check.json`・`grade-out.json`・`export.json`・`verify-connected.err` を読んで値を照合した。
