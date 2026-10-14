---
tasks: [01M4VAAKFJHRSVKZ3996NFKSKS]
title: eval-run dryrun — 初回見積り（manifest-v2）、codex quota の pilot 計画、本番導入の人の手順
status: done
completed: 2026-10-14
---
# dryrun: 初回見積りと pilot 計画

本番 DB は読み取りだけで使った（runner は `mode=ro` で開く）。本番 API へは GET を 2 回だけ送った（`/api/v1/llm/accounts`・`/api/v1/routing-eval/trials`）。LLM は呼んでいない。本番の config・systemd・releases には触れていない。中間物は `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/`（以下 `E/`）に置いた。repo の外。

## 実行

```
cargo build -p routing-eval
$CARGO_TARGET_DIR/debug/celeris-routing-eval dry-run \
  --manifest E/manifest-v2/manifest.json \
  --db /local/celeris/data/db/celeris.sqlite3 --out E/dryrun-0.json
```

- exit 0。`E/dryrun-0.json`（sha256 `95a3ed27…f82ac3beb`）と、標準出力の写し `E/dryrun-0.stdout`。
- manifest sha256 `118b413e…7e7`（manifest-v2: 30 問、subset 12）。

### 初回に出た不具合と修正（runner）

修正前の初回出力では claude の quota がすべて「未測定」だった。USD は過小（opus 60 呼び出しで $7.87）。原因は 3 つ。

1. account の名前が合っていなかった。runner は `quota_estimated.account` を `claude`／`codex` として引いていたが、本番の値は `claude_max_personal`・`claude_max_lab`・`chatgpt_plus_personal`。
2. 窓の欄の名前が違っていた。runner は `per_run_pct` を読んでいたが、本番の event は窓ごとに `used_pct`（その run の前後差、単位は百分率の点）を持つ。
3. 定価換算に cache token が入っていなかった。`input_tokens`・`output_tokens` だけで計算していたが、本番の claude run は cache read が大半を占める。

`crates/routing-eval/src/dryrun.rs` を次のように直した。

- account を event の `source` で決める（`claude-oauth`→claude、`codex-oauth`→codex）。
- 率は (account, model, 窓) ごとに、窓の `method=measured` の `used_pct` を平均する。対象は直近 100 run で、run の model は `runs.model` から引く。
- 候補ごとの窓 % と標本数を出力に足した。account の合計は、全 model に率があるときだけ出す。
- `one_month` を monthly に読む。
- USD は `usage_json.cost_usd`（harness が報告する定価）の model 別中央値を使う。無い run は cache read 0.1 倍・cache write 1.25 倍で計算する。

試験も足した: `production_measured_quota_is_averaged_per_model_and_mapped_from_source`、`list_cost_prefers_reported_cost_and_prices_cache_tokens`。

## 見積り（dryrun-0.json）

### 候補ごと

| account | model | 問題 × k | 呼び出し | 定価換算 USD | 5h 窓 | 7d 窓 | 実測 run 数（5h/7d） |
|---|---|---:|---:|---:|---:|---:|---:|
| claude | claude-opus-5-5 | 30 × 2 | 60 | $37.96 | 76.20% | 8.40% | 100/100 |
| claude | claude-sonnet-5-5 | 30 × 2 | 60 | $15.87 | 69.68% | 7.74% | 31/31 |
| claude | claude-haiku-5-5 | 30 × 2 | 60 | $12.44 | 28.80% | 3.00% | 100/100 |
| claude | claude-fable-5-1 | 12 × 1 | 12 | $41.52 | 121.33% | 11.40% | 18/20 |
| codex | gpt-6.1-sol | 30 × 2 | 60 | 未算定 | 未測定 | (3.75%) | 0/32 |
| codex | gpt-6-luna | 30 × 2 | 60 | 未算定 | 未測定 | (0.60%) | 0/100 |
| codex | gpt-6-astra | 12 × 1 | 12 | 未算定 | 未測定 | (1.64%) | 0/22 |

### account ごと

| account | 呼び出し | 定価換算 USD | 5h 窓 | 7d 窓 | 7d > 15% | quota の出所 |
|---|---:|---:|---:|---:|---|---|
| claude | 192 | $107.79 | 296.01% | **30.54%** | **越える** | `quota_estimated` method=measured の実測（model 別、直近 100 run の平均） |
| codex | 132 | 未算定（単価なし） | 未測定（5h 窓の観測なし） | 参考 5.99% | 未判定（pilot で決める） | 本番の一般 run の実測（7d のみ）。粒度 1 点 |

- claude の % は 1 account に全部載せた場合の値。claude の subscription は 2 つ（`claude_max_personal`・`claude_max_lab`）。均等に分かれれば 1 account あたり 15.27% になり、それでも 15% をわずかに越える。
- 5h 窓は消費の速さを見るための数字で、予算の上限ではない（予算は週 15%）。fable は 1 run で 5h 窓の約 10% を使う。12 run を 1 つの 5h 窓に入れると枠を食い切るので、何回かの 5h 窓に分けて流れる前提で見る（`reserved_runs = 2` で同時 2 本）。
- codex の 7d は「参考」とした。本番の一般 run の `used_pct` は 1 点刻みで、luna は 100 run のうち 1 run だけ +1 点だった。1 呼び出しあたりの率は 1 点を大きく下回るので、この刻みでは率を確定できない。ADR D1 の決定どおり、codex は pilot の実測で判定する。
- USD は本番 run の中央値 × 呼び出し数で出した。評価の試行は本番の WU より短い可能性があり、その場合この値は上振れしている。

### 縮小案（claude、週 15%／account）

runner が自動で出した縮小案（opus 30・sonnet 17・haiku 0・fable 0）は、前から順に枠を埋める方式なので、ADR D1 の最低条件（30×2 は有効 24 問以上、12×1 は 12 問全部）を破る。採らない。D1 を守る案を手で出した。1 呼び出しあたりの 7d 率は opus 0.140・sonnet 0.129・haiku 0.050・fable 0.950。

| 案 | opus／sonnet／haiku | fable | 呼び出し | 7d 合計 | 2 account に均等割り | 1 account あたり 15% 以下 |
|---|---|---|---:|---:|---:|---|
| 全量 | 30 × 2 | 12 × 1 | 192 | 30.54% | 15.27% | × |
| B | 28 × 2 | 12 × 1 | 180 | 29.26% | 14.63% | ○（2 account に分ける場合） |
| C（D1 の最小） | 24 × 2 | 12 × 1 | 156 | 26.71% | 13.36% | ○（2 account に分ける場合） |

- 1 つの account だけでは、D1 の最小（案 C）でも 26.71% になり 15% に収まらない。2 account への配分が要る。配分を固定する経路が eval の admission にあるかどうかは、estimate の unit で確かめる。
- codex は参考値 5.99% で枠内だが、pilot の結果で引き直す。

## codex の quota 実測 pilot の計画

- 候補: `codex-oauth`／`codex`／`gpt-6-luna`。k=2。
- 問題は層の違う 5 問 × k=2 = 10 試行（design・investigate・test・implement・repair 各 1 問。dev 4・held-out 1）。問題 id は `E/pilot-plan.json` にある。
- これらは本評価の試行として数える。`make_trials` と同じ欄（problem_id・trial_no 1..2・source_id・model_id・harness_id・repo・base_commit・objective・check）で投入するので、後の本投入でも `eval_trials` の UNIQUE（manifest_sha, problem_id, trial_no, source_id, model_id, harness_id）が効き、同じ試行は重複しない。
- runner の `run` には問題を絞る option が無い。pilot の unit で `--problems <id,...>` を足す（`make_trials` の結果を id で絞るだけ。欄は変えない）。

### quota の取り方（決定: usage の観測値の前後差を主にする）

- **主**: 本番 API の `GET /api/v1/llm/accounts` から、`chatgpt_plus_personal` の `usage.seven_day.utilization` と `observed_at` を読む。Celeris が codex の `account/rateLimits/read` で観測した値で、usage endpoint と同じ数字になる。読むのは 2 回で、最初の試行が `running` になる前に 1 回、最後の試行が終端になった後に 1 回。2 回目は `observed_at` が最後の試行の `finished_at` より後であることを確かめる。差を Δ（点）とする。
- **照合**: 本番 DB で、pilot の run_id の `quota_estimated` の 7d `used_pct`（method=measured）を足す。
- **主にした理由**:
  - (1) 観測値の刻みは 1 点で、run ごとの差を 10 回足すと丸めの誤差も 10 回分たまる。前後差なら丸めは 1 回で済む。
  - (2) `quota_estimated` は ADR D4 で「新規には書かない」legacy の event なので、selector の release 後には増えなくなる。
  - (3) 5h 窓は codex の観測に無く（`five_hour: null`）、どちらの取り方でも測れない。
- **混入を除く**: pilot の間に同じ account で走った評価以外の codex run を `runs` から列挙する（`account='chatgpt_plus_personal'`、時間帯が重なるもの）。その run の measured `used_pct` を Δ から引く。measured でない run が混ざったら、pilot を「混入あり」と記録し、Δ を上限として扱う。
- **判定**: `--measured` の JSON（`runs: 10`、`accounts.codex.7d = Δ`）で dry-run を引き直す。Δ=0〜1 点なら 1 呼び出しあたり 0.1% 以下で、132 呼び出しは 13.2% 以下になる。Δ≥2 点なら 15% を越えるので縮小案を出す。
- **注意**: luna は本番の実測で codex の中で最も軽い（1 呼び出しあたり sol は約 6 倍、astra は約 14 倍。どちらも粒度が粗い参考値）。luna の率を account の全呼び出しに掛けると過小になりうる。estimate の unit で model の比を掛けた値も併記する（下の未解決 2）。

## 本番導入の人の手順（決定 eval-deploy=main-first）

schema 77 は main からの通常の release で入った。2026-10-14 に読み取りで確かめた: `SELECT MAX(version) FROM schema_migrations` → 77、`GET /api/v1/routing-eval/trials` → 200、`eval_trials` は 0 行。残っている人の操作は予約枠だけ。今の `~/.config/celeris/config.toml` には `[routing_eval]` が無く、既定の `reserved_runs = 0` なので評価 run は dispatch されない。

1. `~/.config/celeris/config.toml` に次を足す。
   ```toml
   [routing_eval]
   reserved_runs = 2
   ```
2. 稼働中の unit を確かめ（`systemctl --user list-units 'celeris@*'`）、その unit を再起動する（`systemctl --user restart celeris@<sha12>.service`）。
3. 次の 2 つで確かめる。
   - `curl -sf -H "Authorization: Bearer $(cat $HOME/.config/celeris/api.token)" http://127.0.0.1:7710/api/v1/routing-eval/trials` が 200（exit 0）を返す。
   - `sqlite3 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro' 'SELECT MAX(version) FROM schema_migrations;'` が 77 以上を返す。
   - 再起動後の journal に config の読み込みエラーが無いことも見る。

## 検査

- `cargo test -p routing-eval`: unit 28 passed、pipeline_stub 1 passed。
- 全体検査（`test-parallel.sh`）と clippy の結果は下の「全体検査」に書く。

## 未解決

1. claude を週 15% に収めるには 2 account への配分が要る（案 B・C）。eval の admission が 2 account に分けるか、分けを固定できるかは、estimate の unit で確かめる。
2. codex の pilot は luna だけを測るので、sol と astra の率は本番の参考値の比で外挿するしかない。比の粒度は粗い（1 点刻み）。
3. `run` に `--problems` が無い（pilot の unit で足す）。

## 提案

- runner の自動の縮小案を、前から順に埋める方式から、model ごとの比例縮小に変え、ADR D1 の最低条件（24 問／12 問）を下限にして、2 account への配分を出力に含める。
- 評価の試行は WU より短い可能性がある。pilot の claude 版（定価・quota の実測）で USD の上振れを確かめる。
