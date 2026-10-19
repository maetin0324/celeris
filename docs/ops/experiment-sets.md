# 実験セット（A/B のみ・B/C のみ）の宣言例と運用

---
tasks: [01M59KXY8345HZEE06FVZ3FWVP]
---

根拠: ADR `agent-docs/adr/2026-10-19-experiment-sets.md`（D1–D10）。実装は `crates/campaign/src/{manifest,runner,checker,global_ledger,preflight}.rs`、統合試験は `crates/campaign/tests/arm_set_e2e.rs`（実 CLI・所有 stub・全体台帳）。
**値（token・鍵・承認枠の数値）はこのファイルに書かない。** 承認値は人の決定（campaign の budget / `approved_trials`）から転記する。

## 1. 宣言（manifest の最小形）
既存の arm ID A/B/C と runner は変えない。`experiment_sets` に有効 arm と比較組を宣言し、`selected_sets` で今回動かすセットを選ぶ。宣言が無い旧形式 manifest は従来どおり A/B/C 全部（互換）。

主比較（A0/B0）— Jev・キー不要:
```json
"experiment_sets": {"main": {"arms": ["A","B"], "compare": [["A","B"]],
  "fixed": {"role_models": {"planner":"sonnet","reviewer":"sonnet","cos":"sonnet","worker":"sonnet"},
            "approved_trials": <承認単位の試行数上限>, "shared_ledger": "<承認単位の台帳 dir（絶対 path）>"}}},
"selected_sets": ["main"]
```
Jev 副比較（B1/C1）— A の account・program 不要:
```json
"experiment_sets": {"jev": {"arms": ["B","C"], "compare": [["B","C"]],
  "fixed": {"policy": "<共通 tier 対応表>", "models": ["<候補1>","<候補2>"], "harnesses": ["<h>"],
            "approved_trials": <同じ承認単位の上限>, "shared_ledger": "<同じ台帳 dir>"}}},
"selected_sets": ["jev"]
```
別セットは**別の測定 ID（`campaign_id`）**にする。`secondary.subset` は B/C のセットでだけ使う。

## 2. 運用（無課金部分は同じ手順で通る）
1. `celeris-campaign validate --manifest m.json` → `dry-run`（`llm_calls_made = 0`、`total_trials` と `per_config` が有効 arm だけ）。
2. `preflight` / `env-lock` / `lock`: 有効 arm の分だけ要求する。B/C のみでは A の program・account・probe を要求しない。A/B のみでは Jev・C・Celeris 側の認証を要求しない。
3. `run --global-ledger <dir>` に manifest の `shared_ledger` と同じ dir を渡す（承認単位ごとに 1 dir）。`allocation`/`approved_trials` を宣言したら全セットに同じ絶対 path の `shared_ledger` が要り、set 宣言 hash（lock の `sets_sha256`）に入る。共有承認の宣言が無い旧形式 manifest だけが台帳の省略（単独測定）・任意 dir の opt-in を続けられる。
4. `check` → `grade-out` → `export`: 選択していない比較表は出ない。未使用 arm は分母に入らない。

**Jev 未準備でも主比較（A/B）は進められる**。B/C のセットは A/B の完了を待たず、逆も同じ。論文の問題設定・設計・評価方法の執筆は Jev の準備に依存しない。

## 3. 拒否されるもの
- 同じ測定 ID で有効 arm・比較組・設定を変えた resume（`resume rejected: … changed`）。変えるなら新しい測定 ID。
- B0 の全役割（planner / reviewer / cos / worker）に Sonnet 以外が混じった run（checker が実測で拒否）。
- B1/C1 の候補集合・tier 対応表（`lower_policy`）・`run_harnesses` の不一致、C1 の単一モデル pin。
- 共有承認の manifest で `--global-ledger` の省略・`shared_ledger` と違う dir（別測定 ID・別 out でも同じ）、`shared_ledger` の無い・食い違う宣言、宣言を結び付けていない lock（旧 lock・別宣言の lock）。CLI は preflight より前、`runner::run` は台帳・out・投入より前に拒否する。
- 全体台帳: 承認単位の token / 試行数 / provider 枠の上限は**最初に採った値の最小**が残り、別測定 ID の manifest で増やせない。他測定 ID の既消費 + 予約 + 自分の取り分が超える投入は、out の作成・task 投入より前に拒否。判定と予約は flock の中で行い、同時投入で両方は通らない。resume / retry は自分の行を上書きするので二重計上しない。

## 4. 未実施（人の操作が要る）
実 pilot と最終セットの preflight は統合 01M58VG5C4XGK4E70M6WJ6MEX0 で行う。この手順書の範囲では実 LLM を呼ばない。
