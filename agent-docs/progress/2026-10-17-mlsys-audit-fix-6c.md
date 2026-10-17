---
title: "[監査修正6c] 既存承認の残予算内で最小 pilot を実経路で確認（承認待ち・待機）"
tasks: [01M542RGFR4WHVCFBPRMKWTD79]
status: blocked
updated: 2026-10-17
---

# 監査修正 6c: 最小 pilot の実経路確認 — 準備完了、人の昇格と実行を待つ

**結論: 有料実行は 0 件。実経路の pilot はまだ走っていない（未検証）。** 実行に要る 2 つの前提が worker の権限の外にあり、
人の作業（新 release の昇格と運用 shell からの実行）を待って待機する。pilot 条件の固定・無課金の検査（validate / dry-run /
preflight / mock の run→check→grade-out→export）までは済ませた。大規模本測定の開始・再実行・予算増額はしていない。

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

## 2. 実行しなかった理由（worker の権限の外）
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
9. 結果（stdout JSON、`check`・`grade-out`・`export`・`--verify-connected` の exit と要点、tokens 合計）をこのファイル §6 に書き戻す。合計が 900,000 を超えることはない（gate と stop_on_budget）。

## 6. 区分（項目ごと）
| 項目 | 実装済み | mock 検証済み | 実経路検証済み | 承認待ち |
|---|---|---|---|---|
| 通常経路 POST /tasks → 子 task → 成果物（B/C） | 75caf0b2・b430d0cb ほか（main） | 6b 通し 7 件（実 HTTP + mock 実行内部） | **未検証**（修正後 release での実行なし） | H1 昇格と §5 の実行 |
| A 腕（claude-code 直接） | main | pilot-2 の mock run | **未検証**（修正後。mlsys-pilot-1 は修正前） | §5（運用 shell） |
| 独立 grader の成功判定への結合 | 9576eb45（fix 1） | 6b `wrong_artifact_…`・`ungraded_…`、本 task の mock grade-out→export | **未検証** | §5 の 6・7 |
| export `--verify-connected` | celerisctl campaign export | 6b `normal_path_…`・`campaign_connected.rs` | **未検証** | §5 の 7 |
| preflight gate（不足時 0 件で停止） | 32d7db7f | 本 task で pilot-2 に対し確認（不足 = token のみ） | 該当なし（無課金の検査） | — |
| 予算 gate（予約・停止） | 9bb5a4a8・6b の checker 修正 | 6b `budget_stop_…` | **未検証** | — |
| Jev の判断反映・usage・版 | b4552084（call_id・費用 NULL 保持）、jev_smoke 判定器 | `lane_assist` 単体試験 | **未検証**。mlsys-pilot-1 の jev_ok 観測は修正前 release で、`real_api_verified=false`・費用と版の欠測・route decision shadow の所見あり（`2026-10-16-mlsys-pilot-wiring-fixes.md`）。作り直さず §5 の 8 で再観測する | §5 の 8 |
| pilot-2 の消費（≦ 900,000、本測定 3M 枠の内側） | — | dry-run 151,590（仮置き） | **0（未実行）** | pilot-2 を本測定枠に数える扱いの確認（§3） |

## 7. 未解決・提案
- 実経路の確認はすべて人の H1 昇格と運用 shell の実行待ち。終わるまで本 task は待機（done にしない）。
- 提案: A 腕を worker から走らせない現構成では、実経路の確認は毎回人の shell が要る。A も daemon 経由（run・quota の記録内）で起動する executor にすれば、
  承認済みの小規模確認を Celeris 自身が実行できる（別 task。本 task では作らない）。
- identity 側の未適用 patch の取り込み／破棄は本 task の範囲で確認していない（6b と同じく要確認）。
- 本測定の `final.json` の `max_tokens` を pilot-2 の実消費分下げるかは本測定の前に人が決める。

## 証拠コマンド
- `git merge --ff-only origin/main` → 173988bb..05a398f1。
- `git diff --name-only 289d220c1555 HEAD -- crates/task-core/migrations` → 空。
- validate / dry-run / preflight / mock の run・check・grade-out・export → §4 のとおり。
- `bash scripts/dev/test-parallel.sh` → exit 0（nextest passed 5595, failed 0, ignored 15）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → ok。
