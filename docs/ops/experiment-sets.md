---
tasks: [01M59KXY8345HZEE06FVZ3FWVP, 01M5B1S1E914GRPQXCD2HK18DB]
---
# 実験セット（A/B のみ・B/C のみ）の宣言例と運用

根拠: ADR `agent-docs/adr/2026-10-19-experiment-sets.md`（D1–D10 と付記）。実装は `crates/campaign/src/{manifest,runner,checker,global_ledger,preflight}.rs`、
統合試験は `crates/campaign/tests/arm_set_cli_normal.rs`（実 CLI・通常 `POST /api/v1/tasks` の所有 stub・固定証拠と共有台帳を有効）、
`tests/arm_set_e2e.rs`、`tests/global_ledger_atomic.rs`、`tests/set_fixed_observed.rs`。
**値（token・鍵・承認枠の数値）はこのファイルに書かない。** 承認値は人の決定（campaign の budget / `approved_trials`）から転記する。

## 1. 宣言（manifest の最小形）
既存の arm ID A/B/C と runner は変えない。`experiment_sets` に有効 arm と比較組を宣言し、`selected_sets` で今回動かすセットを選ぶ。
宣言が無い旧形式 manifest は従来どおり A/B/C 全部（互換。固定欄の実測照合も従来の範囲のまま）。

主比較（A0/B0）— Jev・C の鍵は不要:
```json
"experiment_sets": {"main": {"arms": ["A","B"], "compare": [["A","B"]],
  "fixed": {"role_models": {"planner":"sonnet","reviewer":"sonnet","cos":"sonnet","worker":"sonnet"},
            "initial_info": "<材料 hash>",
            "approved_trials": <承認単位の試行数上限>, "shared_ledger": "<全体台帳 dir の絶対 path>"}}},
"selected_sets": ["main"]
```
Jev 副比較（B1/C1）— A の account・program 不要:
```json
"experiment_sets": {"jev": {"arms": ["B","C"], "compare": [["B","C"]],
  "fixed": {"policy": "<共通 tier 対応表>", "models": ["<候補1>","<候補2>"], "harnesses": ["<h>"],
            "approved_trials": <同じ承認単位の上限>, "shared_ledger": "<同じ dir>"}}},
"selected_sets": ["jev"]
```
別セットは**別の測定 ID（`campaign_id`）**にする（同じ測定 ID でセットを替えた再開は拒否される）。`secondary.subset` は B/C のセットでだけ使う。

## 2. 認証の区別
- **B は Celeris の通常 API 認証が要る**: B（と C）の試行は測定 daemon の通常 `POST /api/v1/tasks` で投入するので、
  `--api-url` と `--token-env <環境変数名>`（Bearer token。値は環境変数にだけ置き、argv・出力・台帳に出さない）が A/B でも B/C でも必須。
- **A/B のみでは Jev の鍵・C の credential・根拠表は要らない**（C の probe・API 要求は 0）。A には `--a-program` と `--a-account` が要る。
- **B/C のみでは A の program・account・probe は要らない**。C 側の Jev 準備（鍵を含む）は B/C のセットでだけ要る。

## 3. 運用（無課金部分は同じ手順で通る）
1. `celeris-campaign validate --manifest m.json` → `dry-run`（`llm_calls_made = 0`、`total_trials` と `per_config` が有効 arm だけ）。
2. `preflight` / `env-lock` / `lock`: 有効 arm の分だけ要求する（§2）。
3. `run` / `watch` に `--global-ledger <dir>` を渡す。**共有台帳は必須条件**: いずれかのセットが `allocation` か `approved_trials` を
   宣言したら、全セットが同じ絶対 path の `shared_ledger` を宣言しなければならず（違えば validate の段階で拒否）、`--global-ledger` の
   省略・別 path は preflight・投入より前に拒否される（非共有の fallback は無い）。A/B と B/C の測定 ID は同じ dir に数える。
4. `check` → `grade-out` → `export`: 選択していない比較表は出ない。未使用 arm は期待試行・分母・費用・表に入らない。
   分母は試行（case × repetition）で、task 内の retry run は分母に入らず費用だけが同じ試行に加わる。

**Jev 未準備でも主比較（A/B）は進められる**。B/C のセットは A/B の完了を待たず、逆も同じ。論文の問題設定・設計・評価方法の執筆は Jev の準備に依存しない。

## 4. 全体台帳（`global-ledger.jsonl`、schema `celeris.campaign-global-ledger/2`、追記専用）
- **承認の引継ぎ**: 承認上限（tokens = `budget.max_tokens`、試行数 = 宣言した `approved_trials` の最小、provider 枠）は台帳の全行（置換された古い行も含む）と
  manifest の最小で、別測定 ID の manifest や再開で増やせない。取り分（`allocation`）は全体の内数で、publish で消えない。
- **投入前の拒否**: run 開始時（`admit_start`）に他測定 ID の既消費 + 未精算予約 + 自分の取り分が上限を超えれば、out の作成・task 投入より前に拒否。
  さらに毎投入の前に（`admit_trial`）tokens・試行数・provider 枠を検査して予約を fsync してから投入する。判定と予約は flock の中で行い、同時投入で両方は通らない。
- **障害時の予約回復（安全側）**: 予約は Reserved → Submitted → Settled と永続化する。Reserved / Submitted のまま止まった予約と枠は黙って解放しない。
  再開時に試行 identity を冪等鍵として実行器へ照会し、投入が無いと確かめた予約だけを解放し、投入があれば終了を追ってから精算する。
  同期 executor のエラーと実行中の停止は予約額を消費として保守的に精算し、tokens 欠測は予約額と既観測の大きい方で精算する（0 にしない）。
  resume / retry は同じ identity の行を置換するので二重計上しない（別 attempt の確定消費は残る）。
- **旧形式互換**: 旧 `/1` 行は読取のみ。消費は全部を確定消費として持ち（下げない）、cap 欄の無い行は上限を与えない（承認は増えない）、
  provider 枠は同じ測定 ID の持ち主が未完了試行が無いことを確かめて解放するまで占有し続ける。読めない行・不明 schema は破損として拒否する。

## 5. 拒否されるもの
- 同じ測定 ID で有効 arm・比較組・set 宣言・有効設定を変えた resume（`resume rejected: … changed`、同じ out では saved lock の gate でも拒否）。変えるなら新しい測定 ID。
- B0 の全役割（planner / reviewer / cos / worker）に Sonnet 以外が混じった run、model 未観測の run（checker が実測で拒否）。
- 固定欄（`policy` / `models` / `harnesses` / `role_models` / `initial_info` / `success`）を宣言したセットの**観測不足**: 空 role・空/重複 run_id・
  model / harness / lower_policy の欠測・observed が空・worker run が無い試行、snapshot / lock / 材料 hash の欠測は宣言で補わず違反（fail closed）。
- B1/C1 の候補集合・tier 対応表（`lower_policy`）・`run_harnesses` の不一致、B/C の片方だけが走った case、根拠表の使用、C1 の単一モデル pin。
- 共有承認の宣言で `--global-ledger` を省略・別 path にした run、台帳の上限・取り分を超える投入（§3, §4）。

## 6. 状態の区別
- **実装済み・無 LLM 検証済み**: 上記すべて（実 CLI・通常 POST stub・固定証拠と共有台帳を有効にした A/B と B/C、改変 resume の拒否、未選択比較・分母）。
- **実 pilot 未実施**: 実 LLM・実 daemon の run、実 token の費用照合、実 daemon での preflight probe。
- **統合 task で最終 preflight 予定**: 最終セットの preflight と本測定は統合 01M58VG5C4XGK4E70M6WJ6MEX0 で行う。
- **承認待ち・任意延期**: 承認値（試行数・token 上限）は人の決定待ち。B0 の `cos` run が実 daemon の task API で観測できるか（ADR 付記の未解決）は実 pilot で確認する（試験の stub は cos run を返す）。
  in-flight 予約の拡張などは任意で延期。この手順書の範囲では実 LLM を呼ばない。
