---
unit: estimate
task: 01M4VAAKFJHRSVKZ3996NFKSKS
status: done
completed: 2026-10-14
---
# estimate: 実行直前の見積り（受信箱で人が承認する）

## 人が承認する表（continue = この表で本評価を投入してよい）

| account | 呼び出し | 定価換算 USD | 7d 見積り | 5h 見積り | 縮小した model と問題数 |
|---|---:|---:|---:|---:|---|
| claude（claude-oauth） | 144 | $52.80 | **14.96%** | 132%（1 つの 5h 窓に入らない。2〜3 窓に分かれて流れる） | opus・sonnet・haiku を 30×2 → **24×2**（D1 の最小）。**fable は今回 0**（12×1 だけで 7d 11.4% のため。決定 `eval-fable-window`） |
| codex（codex-oauth） | 132 | 未算定（単価なし） | **13.2%**（上限側） | 観測なし（codex は 5h 窓を返さない） | 縮小なし（sol・luna 30×2、astra 12×1） |
| 計 | 276 試行（pilot 10 を含む） | | | | |

- fable の 5h 窓の分け方: 今回の計画には fable が無い。次の週窓で 12×1 を流すときは、1 run が 5h 窓の約 10% を使うので 4 run ずつ 3 組に分け、組の間を 5h 空ける（1 組は同時 2 本で約 20 分）。
- 見込み時間: 上限で約 12 時間。本番 run の model 別の壁時計の中央値（opus 381 s・sonnet 220 s・haiku 457 s・sol 263 s・luna 217 s・astra 521 s）× 呼び出し数 ÷ 同時 2 本（`reserved_runs = 2`）。pilot の luna の試行は本番 run の約 0.3 倍の時間だったので、実際は約 4 時間の見込み。
- pilot の合否: **合格**。batch `01M4W7JDSJVGS8R14D9QA5TEPZ` の 10 試行はすべて終端（passed 6・failed 4・error 0）。旧 batch `01M4W35QEM…` は使っていない。

### continue の前に人が確かめること

1. **haiku が本番で起こせない。** 本番 `~/.config/celeris/config.toml` の `[llm_sources.claude_oauth] models` に `claude-haiku-5-5` が無い。このままでは haiku の 48 試行が「source does not declare model」で起動できない。人の手順（決定 `eval-haiku-route`）:
   1. `models = ["claude-fable-5-1", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]` にする。
   2. `systemctl --user list-units 'celeris@*'` で稼働中の unit を確かめ、`systemctl --user restart celeris@<sha12>.service` で再起動する。
   3. `journalctl --user -u celeris@<sha12>.service -n 50` に config の読み込みエラーが無いことを見る。
2. claude の 7d 14.96% は、claude の全試行が 1 つの subscription に載った場合の値。admission は空いている account を前から取るので、`claude_max_personal`・`claude_max_lab` に分かれれば 1 account あたりはもっと小さい。どの分かれ方でも 15% を越えない。
3. 今の 7d 使用率（2026-10-14 03:5x Z）: claude_max_personal 9%・claude_max_lab 9%・chatgpt_plus_personal 2%。

## 見積りの根拠

中間物は `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/`（以下 `E/`）。本番 DB は読み取りだけ（`mode=ro`）。本番 API へは GET だけ。LLM は呼んでいない。

### pilot の確認

- `GET /api/v1/routing-eval/trials?batch_id=01M4W7JDSJVGS8R14D9QA5TEPZ` → 10 件、status は passed 6・failed 4（`E/pilot2-trials-now.json`）。最後の試行の終了は 03:46:27Z。
- 1 試行（trial `01M4W7JDSJTFTSZXM8R7M3SB58`、run `01M4W7JE2Q730WH0Z16YC5VSHN`）の run log を manifest と照合した。`prompt.txt` に manifest の問題文があり、check の command は `prompt.txt`・`request.json`・`stdout.jsonl` のどこにも無い。
- 混入: 03:33〜03:47 に `chatgpt_plus_personal` で走った run は pilot の 10 run だけ（`runs` 表）。

### codex の quota（`E/measured.json`）

- 前値 `E/pilot2-before.json`: `seven_day.utilization = 0.02`（observed_at 03:32:05Z）。
- 後値 `E/accounts-now.json`: `0.02`（observed_at 03:52:11Z。最後の試行の終了より後）。
- Δ = 0 点。値は 1 点刻みなので、本当の消費は 1 点未満。上限の 1 点を 10 run で割り、luna は 1 run あたり 0.1% 以下とした。`measured.json` は `{"runs":10,"accounts":{"codex":{"7d":1.0}}}`。
- `dry-run --measured E/measured.json --out E/dryrun-1.json`: exit 0。codex の 7d は 13.2%（132 呼び出し × 0.1%）、`weekly_over_budget: false`。
- model 別に見ると、sol（0.0625%/run）と astra（0.136%/run）は本番の実測、luna は pilot の上限を使って 11.4%。表には大きい方の 13.2% を書いた。

### claude の quota と USD（`E/measured-quota-1.json`）

- runner の dry-run は claude の quota を読めない（dryrun.md 未解決 4）。`dryrun-1.json` の claude は「未測定」のまま。claude の値は読み取り script `E/measured_quota.py` を本番 DB に対して流し直して出した。
- 1 呼び出しあたりの 7d: opus 0.130・sonnet 0.132・haiku 0.050・fable 0.950。USD の中央値: opus $0.633・sonnet $0.260・haiku $0.207・fable $3.239。
- 全量（30×2 ×3 + fable 12）だと 7d 30.1% で越える。縮小は D1 の最小（30×2 は 24 問）に揃えた。3 model を 24×2 にすると 14.96%。fable を足す余地は無い。
- 縮小した 24 問: subset 12 問は全部残した。subset 外の 18 問から、層ごとの比で 6 問を落とした（design 2・investigate 2・implement 1・test 1）。どの層も 1 問以上残る。落とした id は `E/run-plan.json` の `dropped_from_30`。
- rate は本番の WU の run から取った。評価の試行は本番より短い（pilot の luna は token が本番の約 0.54 倍）ので、この見積りは上振れ側。

### 試行計画（`E/run-plan.json`、生成 script `E/gen_run_plan.py`）

- 276 試行。opus・sonnet・haiku は 24 問 × trial 1〜2、sol は 30 問 × 1〜2、astra は subset 12 問 × 1。luna は 25 問 × 1〜2 と、pilot の 5 問 × 3〜4（pilot の 10 試行。旧 batch が trial 1〜2 を使っていて UNIQUE に当たるため）。
- submit の unit が投入するのは、pilot の 10 を除いた 266 試行。

## 人への決定

- `eval-fable-window`: fable 12×1 をどこで流すか。推奨は、claude の 7d が戻った後（lab は 10-18 23:00Z、personal は 10-19 23:00Z）に別途承認して流す。
- `eval-haiku-route`: haiku を本番の claude_oauth に足すか（上の手順）。推奨は足す。

## 未解決事項

- runner の dry-run が claude の quota・USD を読めない（`E/dryrun-runner-fix.patch` が未取り込み）。
- runner の `run` に試行計画（`run-plan.json`）を渡す入口が無い。submit の unit で、部分集合の manifest にするか、runner に `--plan` を足すかを決める必要がある。
- claude の 5h 窓は 132% 相当なので、本番の仕事と重なると quota で deferred が出る（試行は queued に戻るだけ）。

## 検査

- コードは変えていない（変更はこの file だけ。`sh "$CELERIS_WU_SCOPE_PATHS"` の出力もこの 1 path）。
- `TMPDIR=/local/x bash scripts/dev/test-parallel.sh`: exit 0。nextest 5082 tests run: 5082 passed, 12 skipped。CELERIS_TEST_SUMMARY は passed 5085・failed 0・ignored 13・doctest_exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- run-plan の検査: 各 account の weekly_pct ≤ 15（claude 14.956・codex 13.2）、calls 合計 276 = trials 276。
