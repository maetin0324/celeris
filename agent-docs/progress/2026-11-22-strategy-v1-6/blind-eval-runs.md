---
task: 01M7XQ9QWK58ESMW42Q1TGT3WZ
work_unit: blind-eval-runs
status: done
completed: 2026-11-23
---
# 子 6 WU blind-eval-runs: 本番の reviewer run の起動口で E-mid・D-done を評価する driver（stub で確認、実 LLM は人が流す）

人の判断（2026-11-23、案 A）による再計画の WU。基準は `$CELERIS_WU_BASE`（WU の base）。検査は提出時の HEAD で実行した。

## 結論

- `scripts/dev/parent-eval-blind.sh` を足した。時点ごとに一時 home（一時 DB・loopback）に worktree の build の daemon を起こし、slice から使い捨ての案件を seed して controller を enabled にする。
  controller の判断 `pc/checkpoint/evaluate` が `start_side_review`（`SideReviewKind::ParentEvaluation`）で reviewer run を 1 本起こし、root に最終の記録 `parent-evaluation/<判断 id>.json` が載るまで待つ。
- stub（LLM 0）で E-mid・D-done とも、本番の起動口の記録が `checked` で書かれた。各時点の run は side review の 1 本だけで、worker の run は 0 本。stub の判定（`unknown`・`ask_human`）は評価ではない。
- 実 LLM の評価（`--harness config`）は worker の run の中では起こせない（`CELERIS_RUN_ID` で exit 2）。下の「人の手順」で operator が流す。**独立の評価器が評価したことは、この WU ではまだ未確認。**

## 作ったもの

| file | 役割 |
|---|---|
| `scripts/dev/parent-eval-blind.sh` | 入口。build・control-ts の bundle・seed の試験の build・時点ごとの driver。`--harness config` は承認（`pilot_gate.py check-approval`、scope の arms は `parent-eval-blind`）と run 数（1〜4）を確かめる |
| `scripts/dev/parent-eval-blind/driver.py` | 1 時点の driver。seed → daemon → TS の配備（一時 DB だけ。long-horizon-pilot の driver と同じ手順）→ 承認済み Roadmap（既存の API）→ controller enabled・wake → 記録を待つ → `evaluation.json`・`reads.json`・`run.json` |
| `scripts/dev/parent-eval-blind/stub_reviewer.py` | fake adapter の reviewer（LLM 0）。作業 tree の file を読み、読んだ path を出し、出力の例と同じ形の `parent_evaluation` を書く |
| `scripts/dev/parent-eval-blind-compare.py` | 時点ごとの status・verdict・next と期待の一致、禁じた読み・外の読みを `compare.json` に書く（時刻を書かない） |
| `crates/task-api/tests/parent_checkpoint_live.rs` | `parent_checkpoint_blind_seed`（`#[ignore]`、driver だけが走らせる seed）と `parent_checkpoint_blind_live_checks`（`#[ignore]`） |

- seed は store の公開の関数だけを使う（`project_create`・`create_task`・`append_event`・`cos_operation_apply_at`）。子の行の status は cutoff の時点の値で、報告の本文は cutoff までの `WorkerFinished`。
  strategy root の正本（ADR D1.1）は gate と同じ `task.create` の操作の記録。時点で ready の子（E）は案件の外の draft の task に依存させ、daemon が起こさない（依存は評価器の入力に入らない）。
- reviewer の作業 tree は slice の file だけの git repo（`events.jsonl`〈cutoff まで〉・`goal.md`・`reports.json`・`slice.json`）。ADR・replay・試験・進捗・計画は置かない。driver の本文に answer key の path は書いていない。
- `reads.json` は run の `stdout.jsonl` から取る（claude-code の stream-json の `tool_use` の `file_path`・`path`・`command`、stub の `read:`）。

## 受け入れ条件と証拠（提出時の HEAD で実行）

0. stub の driver と compare
   - `bash scripts/dev/parent-eval-blind.sh --harness stub --out <artifacts>/parent-eval-blind-stub` → exit 0。E-mid・D-done とも `evaluation.json`・`reads.json`・`run.json` を書いた。
     記録は `status: checked`、effective `unknown`/`ask_human`、violations 1 件（`execution_mismatch`: stub の申告 partial・事実 succeeded）。reads 4 件（slice の 4 file）。run は fake・model-1・usage は stub の固定値。
     daemon.log の run は各時点で `starting reviewer run` 1 本（side review）だけ。
   - `python3 -I scripts/dev/parent-eval-blind-compare.py --out <dir> --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json` → exit 0（不一致 2/2 は stub なので想定どおり）。
     2 回目の実行の後に `sha256sum -c` が OK（同じ byte）。`compare.json` に時刻の文字列は 0 件。
   - 陰性: reads に `agent-docs/adr/` の path を足すと exit 1（forbidden 1・outside 1）。`D-done/evaluation.json` を消すと exit 1。
1. worker の run の中の拒否
   - `CELERIS_RUN_ID=x bash scripts/dev/parent-eval-blind.sh --harness config` → exit 2、stderr `refusing to start LLM runs inside a Celeris worker run (CELERIS_RUN_ID is set)`。
     未知の引数を先に置いても（`--bogus --harness config`）同じ exit 2 と文言（引数の検査より前）。
   - 不備の承認（`{}`）は exit 2（`the approval does not allow this evaluation`）。時点が 5 件なら exit 2（`1 to 4 points`）。
2. 試験 `parent_checkpoint_blind_live_checks`
   - `PARENT_CHECKPOINT_BLIND_DIR=<stub の出力> cargo test -p task-api --test parent_checkpoint_live -- --ignored --exact parent_checkpoint_blind_live_checks` → 1 passed（E-mid・D-done とも Checked、slice の外の参照 0）。
   - env 無し（既定の `fixtures/parent_checkpoint_live/blind/`。file が無い）→ 1 failed（`No such file or directory`）。
   - 既定の dir に stub の出力を写すと 1 failed（`既定の dir に stub の出力がある`）。写した file は消した。
   - 同じ file の既存の試験: `cargo test -p task-api --test parent_checkpoint_live` → 5 passed、3 ignored。
3. この進捗に人の手順と stub の結果がある（下）。

## 人の手順（operator が host で流す）

前提: 流すのは worker の run の外（`CELERIS_RUN_ID` が無い shell）。本番の DB・daemon・config には触れない（一時 home は `$TMPDIR` の下で、終わると消える）。

1. 承認の記録（json）を作る。例:
   ```json
   {"approval_id": "parent-eval-blind-20261123", "approved_by": "human:<名前>", "approved_at": "2026-11-23T12:00:00Z",
    "expires_at": "2026-11-24T12:00:00Z", "account": "<account>", "model": "<model>", "harness": "claude_code",
    "scope": {"cases": ["E-mid", "D-done"], "arms": ["parent-eval-blind"], "environment": "non-production"},
    "limits": {"max_usd": 15, "max_wall_secs": 1800}}
   ```
   `limits.max_usd` は一時 daemon の `max_usd_per_day` になる。`max_wall_secs` は時点ごとの待ち（既定 600 秒）以上にする。
2. `--llm-config` の TOML を書く（long-horizon-pilot-llm.sh の `--llm-config` と同じ形: `[llm_sources]`・`[harnesses]`・`[route_defaults]`・`[harness_selection]`・`[accounts]`・model の `[budget_rates]`）。
   reviewer の run が実 LLM の harness に routing されること。
3. 実 LLM の評価（run は 2 本。E-mid と D-done に 1 本ずつ）:
   ```sh
   cd <agent-platform の checkout（この WU が入った commit）>
   export CARGO_TARGET_DIR=<ローカルの scratch>
   bash scripts/dev/parent-eval-blind.sh --harness config --approval <承認の json> --llm-config <toml> \
       --out /local/celeris/data/workspaces/01M7XQ9QWK58ESMW42Q1TGT3WZ/artifacts/parent-eval-blind/ --points E-mid,D-done
   ```
   exit 0 で `<out>/E-mid/`・`<out>/D-done/` に `evaluation.json`・`reads.json`・`run.json`（と daemon.log）ができる。
4. 比べる:
   ```sh
   python3 -I scripts/dev/parent-eval-blind-compare.py --out /local/celeris/data/workspaces/01M7XQ9QWK58ESMW42Q1TGT3WZ/artifacts/parent-eval-blind/ \
       --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json
   PARENT_CHECKPOINT_BLIND_DIR=/local/celeris/data/workspaces/01M7XQ9QWK58ESMW42Q1TGT3WZ/artifacts/parent-eval-blind \
       cargo test -p task-api --test parent_checkpoint_live -- --ignored --exact parent_checkpoint_blind_live_checks --nocapture
   ```
5. 確認の方法:
   - compare が exit 0（評価がある・`checked`・禁じた読みと外の読みが 0）。exit 1 なら `compare.json` の `failed` を読む。
   - `compare.json` の `verdict_match`・`next_match` が answer key との一致。不一致でも exit 0（結果として報告する）。
   - `run.json` の `harness` が `config`、`usage`・`usd` が入っている。試験が 1 passed。
   - 2〜4 回の範囲で、もう 1 回流すなら `--out` を別の dir にする（同じ dir は時点の dir を消して書き直す）。
6. close-gate の WU が出力を `crates/task-api/tests/fixtures/parent_checkpoint_live/blind/` に写し、env 無しの試験で判定する（この WU の範囲外）。

### run 数と費用の見積もり（推定。実測ではない）

- run 数: 1 回の実行で 2 本（時点ごとに side review の 1 本。stub の確認で、他の run が 0 本であることを daemon.log で見た）。人の認めた上限 4 本までに、もう 1 回流せる。
- 入力の量: prompt 約 15 KB、作業 tree の file は events.jsonl 約 200 KB（E-mid 201,119 byte・D-done 210,182 byte）・goal.md 12,912 byte・reports.json 約 8 KB。全部を読むと 1 run で約 8 万 token。
- 費用: 20 turn 前後で cache の読みを含む入力が 0.5〜1.5M token、出力 1 万 token 前後と見て、1 run 約 1〜3 USD、2 本で約 2〜6 USD、4 本でも 12 USD 以下。model の単価で変わる（未確認）。
- 時間: 1 時点あたり build の後 1〜10 分（stub は 1 時点約 30 秒）。

## 試験の追加の理由（守る不変条件と、既存の試験で検出できない理由）

- `parent_checkpoint_blind_live_checks`: 本番の起動口（controller の evaluate → `start_side_review` → `settle`）が書いた E-mid・D-done の最終の記録が `checked` で、全参照が slice の中にあること。既定の dir では stub の出力を拒む。
  既存の `parent_checkpoint_live_sample_checks` は WU の run が手で書いた E-mid の 1 件だけを見て、本番の起動口の出力と D-done を見なかった。
- `parent_checkpoint_blind_seed`: 試験ではなく driver の seed（`#[ignore]`）。案件・root・子・操作の記録を store の公開の関数で置く。

## LoC（`git diff --numstat $CELERIS_WU_BASE`）

- production（Rust）: 0 行。production の Rust・TS・設定は変えていない。
- test（Rust）: `crates/task-api/tests/parent_checkpoint_live.rs` +約 270 行（seed と live の検査）。
- script（Python・shell、開発用）: `parent-eval-blind.sh` 96 行・`parent-eval-blind-compare.py` 101 行・`parent-eval-blind/driver.py` 約 390 行・`stub_reviewer.py` 43 行。
- 純増の理由: 実 LLM を本番の起動口で流す driver と、その出力を判定する試験が無かった（案 A の受け入れ条件）。新しい表・event の種類・migration・Manager・trait・状態機械は足していない。

## 回帰条件

- `cargo test -p task-api --test parent_checkpoint_live` の既存 5 件が pass のまま（入力の fixture は変えていない）。
- strategy の無い task・controller off では reviewer run が 0 本（子 6 の既存の試験。この WU は production を変えていない）。

## 未解決・未確認

- 実 LLM の run（`--harness config`）は未実行（worker は LLM の CLI を起こせない）。claude-code の `stdout.jsonl` から `reads.json` を取る処理は、実の出力では未確認（stub の形でだけ確かめた）。
- root の作業場所が slice の repo（mode shared）なので、reviewer の run の dir（`runs/<run>/request.json`・`stdout.jsonl`）と root の成果物の dir（`artifacts/`。試行の記録）も作業 tree の中にできる。
  どれも評価器自身の入力か出力で、answer key ではない。compare は作業 tree の中の読みとして通す。
- 時点で reviewing の子（E-mid の D）を daemon が final review に回すかは、stub の 2 回の実行では起きなかった（run は side review の 1 本だけ）。待ちが長い実 LLM の run で起きるかは未確認。
- 案件の Roadmap の Outcome（o1「循環の削減」）は driver が置いた要約で、史実の Roadmap ではない（無いと controller が `pc/no-outcomes` で人に返す）。評価器の入力の `goal.outcomes` は空のまま（dispatcher の現行の挙動）。

## 提案

- close-gate の WU は、operator の出力を `fixtures/parent_checkpoint_live/blind/` に写し、env 無しの `parent_checkpoint_blind_live_checks` と compare の結果で第二ゲートを判定する。
