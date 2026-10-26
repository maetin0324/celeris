# MLSys 本測定: measure-launcher.sh で測定環境を起動・照合して回す手順（運用セッション用。worker・agent は実行しない）

---
tasks: [01M52D2AM5K6GCFGEJZW4REWM2, 01M56HCNS9SYAS7TAEA7Z3E4CZ, 01M58VFRBA6HG0SSF3CG5BKM91, 01M5A29H2319SHCSYS6EBYR90G]
---

根拠: ADR `agent-docs/adr/2026-10-18-measure-env-launcher.md`、条件 `agent-docs/progress/2026-10-16-mlsys-final/freeze-final.md`。実装は `scripts/campaign/measure-launcher.sh` と `crates/campaign/src/{env_check,env_lock,measure_env,preflight}.rs`。
**値（token・鍵）はこのファイルに書かない。** token は測定専用 file（`[api] token_file`）に launcher が 0600 で作り、CLI には環境変数の**名前**（`CELERIS_MEASURE_TOKEN`）だけが渡る。
承認枠: 試行 12 以内・tokens 全体 3,000,000・per_case 300,000・per_arm 1,300,000・`token_basis = all`・claude 7 日使用率 45% 以内（codex 21%）・fable 不使用（人の決定 2026-10-16）。枠を超える変更は新 campaign_id と新しい決定が要る。
**2026-10-21 の決定（最小実 pilot `mlsys-main-3`）**: A0/B0 × K4 の 2 試行、`per_case.max_tokens`・`per_arm.max_tokens` 各 1,500,000、tokens 全体 3,000,000（`token_basis=all`、fable 不使用、trial 数 2）。preflight の承認上限定数（`preflight::APPROVED_*`）はこの値に更新済み。上の 300,000 / 1,300,000 は 2026-10-16 の旧枠。

入口は 1 つ: `scripts/campaign/measure-launcher.sh <prepare|start|preflight|watch|collect|grade|export|stop> --env <file>`。標準の流れは prepare → start → preflight → watch → grade（評価側の観測収集 → 独立採点 → check）→ export。各 subcommand は再実行できる。
接続先（`--api-url`）・token・`--measure-config` 等の CLI 引数は、すべて env ファイル（と、その `DAEMON_CONFIG` の `[api] listen` / `token_file`）から launcher が 1 回だけ生成する。**手で `--api-url` や `--token-env` を書かない。**
launcher がやらないこと: systemd / `systemctl` / `pkill` / 本番 config・本番 DB・本番 release の変更、昇格、cluster job の cancel、account の作成・削除・書換え。停止は launcher 自身が起動した daemon の pid 1 つだけ（§7）。

## 0. 前提（人。本番 host 操作。launcher は行わない）
主比較（`mlsys-main-1`）では H1・H3・H4 は済み（§3a の承認値）。以下は一般手順と、副比較・新 campaign のときの前提。
| 項目 | 内容 |
|---|---|
| H1 | B/C の Celeris release を昇格し sha12 を控える（GUI / `promote.sh`）。launcher は昇格しない |
| H2 | 主比較（A0/B0）は Jev 無効で `jev_api_key` は不要（確認しない）。副比較（B vs C）を回すときだけ必要 |
| H3 | A 用の claude_oauth アカウントをログイン済みにして `account_id` を控える（`GET /api/v1/llm/accounts`） |
| H4 | 測定専用の daemon config（§3b の雛形から。`cap-probe-claude-code` 宣言入り。本番と別の port・db・workspace・llm_proxy 無効または別 port・`push=false`・Discord 通知なし）と、突き合わせ用の通常 config（読むだけ）を用意する |
| H5 | 7 日使用率の開始値 `U0`（codex / claude）を控える |
| H6 | cancel された試行の cluster job（`$OUT/*/cluster-cancel.jsonl` の `scancel`/`qdel`）の実行 |

## 1. 記号
```sh
cd <この repo の checkout>
export CARGO_TARGET_DIR=...                # 渡された値のまま
L=scripts/campaign/measure-launcher.sh
B=$CARGO_TARGET_DIR/debug/celeris-campaign # env の CAMPAIGN_BIN にも同じ path を書く（主比較は §3a の固定 binary を使い、`B=$R/pinned/evidence/main-k2/bin/celeris-campaign-fixed`）
P=agent-docs/progress/2026-10-16-mlsys-final   # final.json・task-template.json・daemon/env 雛形の置き場
ENVF=<measure-launcher.env>                # §2。主比較は $R/pinned/main/launcher.env（相対 path は env ファイルの dir 基準）
R=/local/celeris/data/workspaces/01M58VFRBA6HG0SSF3CG5BKM91/artifacts   # 主比較の成果物 root（絶対）。B は主比較では $R/pinned/evidence/main-k2/bin/celeris-campaign-fixed（§3a）
```

## 2. env ファイル（唯一の設定源。`KEY=VALUE` の sh 形式、値に秘密を書かない）
雛形: `$P/measure-launcher.example.env`（複製して `<operator 入力待ち: …>` を実値にする。残っていると `prepare`/`preflight` が拒否する）。
必須（欠けると `prepare` が `<KEY> is required` で非 0）: `MANIFEST` `DAEMON_CONFIG` `NORMAL_CONFIG` `CANARIES` `OUT` `STATE_DIR` `TASK_TEMPLATE` `MATERIALS_SRC` `KB_SRC` `INPUTS_SRC`。`REPO_SRC` は初期 tree を固定する case（K2）を選んだときだけ要る（無ければ preflight の lock 検査が拒否する。K4 だけの選択で渡すと逆に拒否される）。
`A_ACCOUNT` は A が有効な構成（A/B・A/B/C）で要る。必須キーではなく、無いまま A が有効なら `preflight` が `--a-account` 欠落で拒否する。B/C のみの構成は A の認証も program も要らない（`--a-account` は渡されない）。
`start` が加えて要る: `CELERIS_BIN`（測定 daemon の実行ファイル。`prepare` が sha256 を固定）`RELEASE`（B/C の sha12）。`CAP_ENV` は本測定では事実上必須（無いと preflight の cap-probe が拒否）。
任意: `CAMPAIGN_BIN`（既定 `celeris-campaign`。`DAEMON_CONFIG` の `cap-probe-*` の command と同じ実体にする）`A_PROGRAM`（既定 `claude`）`RUN_PATH` `WORKER_PROGRAMS`（`name=path` を空白区切り。env-lock に固定）`MEASUREMENT_ID` `GLOBAL_LEDGER`（共有承認の台帳 dir。manifest の `shared_ledger` 宣言と同じ path。省略・別 path の watch は投入前に拒否される）`VARIANT_B0_SHA256`（manifest が variant を参照するとき、承認済み `variant-b0.json` の sha256。未指定・不一致は preflight が拒否）`INITIAL_TREES`（grade-out の `--initial-trees`。grader 側で組んだ初期 tree の置き場 `<dir>/<problem_id>`）`CASES_ROOT` `GRADER_ROOT` `GRADE_CASES_ROOT`（`grade` に必須）`RECORDS`（export で final_accounting）`EXECUTOR`（既定 `celeris`）`INTERVAL_SECS`（60）`MAX_ITERATIONS`（1440）`HEALTH_WAIT_SECS`（60）。
```sh
MANIFEST=final.json                # §3 の確定済み manifest（$P/final.json の複製でなく、$P を env ファイルの dir にするなら相対でよい）
DAEMON_CONFIG=/abs/measure/daemon.toml        # $P/measure-daemon.example.toml の複製（§3b）
NORMAL_CONFIG=~/.config/celeris/config.toml   # 突き合わせ専用。読むだけ
CANARIES=canaries.json             # measure-config の canary 値の宣言
TASK_TEMPLATE=task-template.json
A_ACCOUNT=<claude_oauth account_id>
CELERIS_BIN=/abs/measure/celeris   # prepare が sha256 を固定
RELEASE=<sha12>
OUT=/abs/local/final-out ; STATE_DIR=/abs/local/measure-state     # NFS 外
MATERIALS_SRC=… KB_SRC=… REPO_SRC=… INPUTS_SRC=…                   # §4
CASES_ROOT=crates/campaign/cases ; GRADER_ROOT=crates/campaign/fixtures/grader-private
GRADE_CASES_ROOT=crates/campaign/fixtures/grade-cases   # 実在。<root>/final/<problem>/<problem>.json（lock の相対 path もこの root 基準）
RECORDS=…/records.jsonl
CAP_ENV=…/cap-env.json             # celeris.campaign-cap-env/1（cap-runbook §0。toolchain 内容 pin → env-lock/3）
```

## 3. 確定済みの構成（主比較の承認済み実値は §3a）（`$P/final.json` は 1 回だけ更新。変えると hash が変わる）
確定済み（2026-10-19、`agent-docs/progress/2026-10-19-main-config/`）:
- B・C は同じ `routing.models=["claude-sonnet-5-5"]`・`routing.run_harnesses=["claude-code"]`・`lower_policy=mlsys-min-1`。方策だけが違う（B=fixed、C=jev-1）。A は claude-code / claude-sonnet-5-5 で集合の内側。`cap-probe-*` は run_harnesses に入れない。
- C は `no_evidence_table=true`（根拠表なし。`effective_evidence_mode=none`）、B は根拠表欄なしで none。**`evidence_table_id` と `celerisctl routing evidence` の手順は無い（消した）。latest へ fallback しない。**
- 比較: primary A vs B、secondary B vs C。subset は K2・K4。K2 `judged_scope=[implementation, experiment]`（analysis / review は `not_judged`。維持する。ADR 2026-10-18-k2-judged-scope）、K4 は analysis（採点はセル有効性の再計算: `planned-cells.json` の期待集合・`review28/qstat-43706.txt` の job 終了状態・実効 fsync/checkRead/api・`results/cells.csv` と件数 metric の照合。ADR 2026-10-23-k4-cells-validity-grading。grade-case の `input_archive` は launcher の `prepare` が `INPUTS_SRC` から grade-case の隣へ写し、preflight が実在・sha256・展開を確かめる）。
- `$P/task-template.json`: `project_id=01M2WTS3DKNZBSZ2JMVB4CZMBW`、acceptance は reviewer、`"max_turns": 100` を必ず書く（無い雛形は preflight・`--task-template` 読み込みで拒否される。書かないと task 既定の 10 に落ち、worker・reviewer が 10 turn で止まる。worker と reviewer は同じ task の max_turns を `--max-turns` に渡す。ADR 2026-10-21-grader-own-input-extraction-and-max-turns D2）。`repos`・`workspace_mode`・`max_tokens` は書かない（拒否される。実行器が trial workspace を作業場所にし、`repos: []`・`max_tokens=per_case` を埋める。ADR 2026-10-20-mlsys-main2-fixes D1・D2）。
- `budget.token_basis=all`。`lower_policy`/`online_update` は変えない。

## 3a. 主比較（測定 ID `mlsys-main-1`、A0 vs B0 のみ）の承認済み固定値
成果物 root（絶対 path。相対 `artifacts/` や親推測は使わない）: `R=/local/celeris/data/workspaces/01M58VFRBA6HG0SSF3CG5BKM91/artifacts`。固定資産は `$R/pinned/main/`（`final.json` `final-input-manifest.json` `daemon.toml` `canaries.json` `task-template.json` `launcher.env` `lock.json` `env-lock.json` `fixed/{cases,grader-private,grade-cases}`）と `$R/pinned/cap-env.json`・`$R/measure-src/main/`。`launcher.env` の `CASES_ROOT` `GRADER_ROOT` `GRADE_CASES_ROOT` は WU の寿命に依存しない `$R/pinned/main/fixed/*` の絶対 path。private grader（`grader-private`）は公開入力（`MATERIALS_SRC` 等）に混ぜない。
| 項目 | 承認済み値 |
|---|---|
| release / binary | `c9654bff5633`、`CELERIS_BIN=/local/celeris/state/releases/c9654bff5633/bin/celeris` sha256 `be62f43d803c92881ead3a27d10a5337a749657cc1ba34029807101cbeeda3d2` |
| A（claude-code） | `harness_version` `2.1.303`（`claude --version` が `2.1.303 (Claude Code)`）、`A_ACCOUNT=claude_max_lab`（参照名のみ。認証材料は出さない） |
| 全役割 | A も B も `claude-sonnet-5-5` のみ（B の planner・reviewer・CoS 等も同じ。`routing.models`・`run_harnesses=[claude-code]`・`online_update=false`・根拠表なし）。**Jev は disabled で `jev_api_key` は不要**、C の呼出しは 0 |
| CAP_ENV | `$R/pinned/cap-env.json`（A と worker が同一 toolchain）。launcher env の `RUN_PATH` は Rust toolchain の `bin` |
| campaign_sha256 | `8ca0e1984bdaf1140f4a849c53c1f01522099a1ec555816bb2472f5c039ea999`（`env-lock.json` と `lock.json` で一致） |
| 固定入力 hash | final-input-manifest 内部 `6fa8f0ec…06e4b`、`snapshot.materials_sha256` `dab3ca05…800d`、`snapshot.kb_snapshot` `741f2414…9fe0702`。K2 初期 commit `086c4618e2a4d94ea56da78ceeed0f8d6d568d67` |
| 実 binary | `celeris-campaign` 修正後 `$R/pinned/evidence/main-k2/bin/celeris-campaign-fixed` sha256 `de339f1c4af25bcb2b3ec8d53222ca05c010a11fb7343f16f69d514559161394`（K2 tree 内 symlink を許す ADR 2026-10-19-campaign-inner-symlink）。`$R/pinned/main/launcher.env` の `CAMPAIGN_BIN`・`daemon.toml` の `cap-probe-claude-code` command・実 K2 preflight の argv はすべてこの 1 本（`check-main-pins.py` が path・実在・sha256 を照合する） |
| 試行数・予算 | 主比較は A・B × K2・K4 × 1 rep = 4 試行。承認枠（§冒頭）は **主副で共有する 1 本**で、副比較（B vs C）の枠を別に足さない。`per_arm` 1,300,000 は A・B 各腕の上限で、全体 3,000,000 を超える分の二重計上はしない |
| K2 / K4 | K2 `judged_scope=[implementation, experiment]`（analysis / review は `not_judged`）、K4 は analysis。維持する |

固定資産の整合は `python3 scripts/campaign/check-main-pins.py --root $R`（読むだけ。別の測定 bundle は `--pins <json>`（または `$R/pinned/main/pins.json`）で承認値を渡す。既定は mlsys-main-1 の値。grade-case の `input_archive` が grade-case の隣に固定 sha256 で在ることも検査する。設定・実在 path・lock/hash・公開側への private 混入・実 K2 preflight 証跡を内容検証し、不足があれば非 0。**手順を始める前と、変えた後に必ず通す**）。

### env-lock/3 と campaign lock（通常の生成関数。watch 不要・無試行）
実 root の `pinned/main/{env-lock.json,lock.json}` は `celeris-campaign offline-lock`（通常の `preflight::expected_lock` → `env_lock::establish` → `lock_exec::save` を呼ぶ最小入口。LLM に接続せず `paid_calls_made=0`）で作る。**モデル一覧（`--models-evidence`）は launcher.env の測定 daemon の `GET /api/v1/llm/models` から取る。本番 daemon の一覧は使わない**（source が違えば watch と preflight の env-lock 照合で拒否される。2026-10-21 の mlsys-main-3 は本番一覧 72 件で作った lock が測定 daemon 14 件と食い違い watch で停止した。ADR `agent-docs/adr/2026-10-21-preflight-env-lock-check.md`）。生成は launcher の `lock` 段で行う（測定 daemon を start した後。保存→sha256→offline-lock を 1 段で行い、`<LOCK_OUT>.measure-llm-models.json` に証跡を残す）:
```sh
LOCK_OUT=$R/pinned/lock-gen-<日付> scripts/campaign/measure-launcher.sh lock --env $R/pinned/main/launcher.env
```
生成後、中身を `pinned/main/` へ複写し checker を通す。preflight は `<out>/env-lock.json` が在れば測定 daemon の一覧と照合し、不一致なら `missing[].check = "env-lock"`・`failure_class = "env_lock_mismatch"` で失敗する。再照合（何も書かない。証跡は lock 段が保存した測定 daemon 由来の file）:
```sh
$B offline-lock --verify --manifest $R/pinned/main/final.json --out <生成時の out dir。`$R/pinned/lock-gen` と同内容> \
  --a-program <claude 実体> --models-evidence <LOCK_OUT>.measure-llm-models.json --models-evidence-sha256 <lock 段が出した sha256> \
  --cap-env $R/pinned/cap-env.json --cases-root $R/pinned/main/fixed/cases --grader-root $R/pinned/main/fixed/grader-private \
  --grade-case-root $R/pinned/main/fixed/grade-cases --measurement-id mlsys-main-1 \
  --repo-src $R/measure-src/main/repo --materials-src $R/measure-src/main/materials --kb-src $R/measure-src/main/kb --inputs-src $R/measure-src/main/inputs
```
（`--out` は空 dir が必須で既存 data は取り込まない。生成し直すときは新しい空 dir に作り、中身を `pinned/main/` へ複写し、checker を通す。）

### 実 K2 無課金 preflight の証跡（取得済み）
operator が測定専用 daemon（release `c9654bff5633`、pid 3909077）を起動し、修正後 binary で実 K2 tree・正解資産に対し `preflight` を実行した（2026-10-19）: exit 0、`ok true`、`missing` 空、`paid_calls_made 0`、`campaign_sha256` 承認値、`checked` に manifest・case-inputs・graders・capability・env-check・trial-state・lock・budget を含む。B/C × K2/K4 の 4 probe が通った。証跡: `$R/pinned/evidence/main-k2/run/{argv.txt,exit.txt,stdout.json,stderr.txt,hashes.txt}`、検証後の正本 `$R/pinned/main/evidence/k2-preflight.json`、手順 `$R/pinned/evidence/main-k2/operator-check.md`。`check-main-pins.py` がこの内容を検証する。
**これは本タスクの無試行検証であり、運用手順としての試行開始（§5 の `watch`）ではない。** `watch`・実 LLM・試行投入は人の決定（本測定の開始）があるまで行わない。選択機構（01M59KXY8345HZEE06FVZ3FWVP）が main に統合されたら、セット別の最終 preflight だけを task 01M58VG5C4XGK4E70M6WJ6MEX0 が取り直す。
修正後 preflight の実行経路は `sh $L preflight --env $R/pinned/main/launcher.env` で再現できる（`measure-k2.env` は証跡取得用で、CAMPAIGN_BIN だけ argv 記録 shim 経由。それ以外は launcher.env と同値）。
JSON の確認に `jq` は使わない（この host に無い）。`python3 -c 'import json,sys; …'` を使う（例: `python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["ok"])' stdout.json`）。

置換後の確認（launcher を経ず直接。API を使わない）:
```sh
$B validate --manifest $R/pinned/main/final.json      # exit 0。campaign sha256 が上の承認値と一致
$B dry-run  --manifest $R/pinned/main/final.json      # within_budget true、total_trials 4
```
以後 manifest・入力を変えない（変えるなら新 campaign_id と新しい決定）。

補足（ADR 2026-10-20 mlsys-main2-fixes D2-4）: input manifest の各 problem の `prompt` は、公開 case 定義を試行 workspace の `materials/<case>/case.json` として参照する（repo 内の `crates/campaign/cases/final/<case>/case.json` は trial workspace に無いので、実行前検査が拒否する）。2026-10-16 の凍結済み入力は旧い参照なので、次の campaign は prompt を直した input manifest と新 campaign_id で再 freeze する。`MATERIALS_SRC` は `<case>/` dir を含むこと。

## 3b. 測定専用 daemon の宣言（人が配置。本番 config・release は触らない）
`$P/measure-daemon.example.toml` を複製し `<...>` を実値にする（db・workspace・KB・memory・token_file・port は本番と別、NFS 外）。内容: 実 harness は `claude-code` だけ、`cap-probe-claude-code`（kind=fake・`env_of="claude-code"`・command=`CAMPAIGN_BIN`・args `cap-worker`）、`[llm_sources.cap-probe]`、`[route_defaults.cap-probe-claude-code]`、`harness_selection.rules` 先頭に分野 `cap-probe-claude-code` の規則。probe は分野 `cap-probe-*` の task にだけ結ばれ、実試行の候補に入らない。宣言を欠くと preflight の B/C cap-probe が投入前に `env_failure` で拒否する。`NORMAL_CONFIG` の `[[clusters]]` を複写する。

## 3c. 実値で測定専用 daemon を立てるときの追加（2026-10-19 実機リハーサルで判明。値は pin-values の成果物 `pinned/main/` に実例）
- `[selfdeploy] releases_dir = "/local/celeris/state/releases"`（読むだけ）。無いと daemon が既定の `~/.local/celeris/releases` を見て `release … is not promoted` で exit 4 する。
- `[accounts] claude_dir`（通常運用と同じ pool）と `[adapters.claude_code] command` / `env = { PKG_CONFIG_PATH = … }`。B/C の cap-probe は `env_of=claude-code` のこの adapter env を worker に渡し、CAP_ENV の `PKG_CONFIG_PATH` が無いと `claude-code/worker-env: worker env lacks PKG_CONFIG_PATH`。
- CAP_ENV は K2 の `rustc-min` / `min-build-locusta` のため Rust toolchain を `ro_mounts` に、その `bin` を `PATH`（`:/usr/local/bin:/usr/bin:/bin` を後ろに足す。置換されるため）に入れる。launcher env に同じ `bin` を `RUN_PATH` として書く（daemon の PATH にも要る）。
- 測定 DB は新規なので task-template の `project_id` の案件は存在しない。`start` 後に `POST /api/v1/projects`（title・request。repo は登録しない — 試行の task は trial workspace だけを作業場所にする）で作り、返った id を task-template の `project_id` に書いて `preflight` を再実行する（token は `$STATE_DIR/token`、値は表示しない）。
- 副比較（`pinned/sub/`、測定 ID mlsys-sub-1）の `project_id` は sub の STATE_DIR の DB に属するので、主比較の id とは別に上の手順で sub の案件を作って `pinned/sub/task-template.json` に入れる（未設定の間 `check-sub-pins.py` は非0。旧 id は無効）。sub の preflight 証跡 `pinned/sub/evidence/k2-preflight.json` と campaign lock `pinned/sub/lock.json` も同様に必須で、取得は統合 01M58VG5C4XGK4E70M6WJ6MEX0。
- 通常 config の `[[clusters]]` を複写すると測定 daemon も pegasus の forward（127.0.0.1:18000）の tunnel を張ろうとする。通常 daemon と重なる点に注意（人が判断）。

## 4. 入力・sandbox 資産の配送（試行へ渡す公開部だけ。grader-private は渡さない）
```sh
M=$TMPDIR/final-materials; K=$TMPDIR/final-kb; R=$TMPDIR/final-repo; I=$TMPDIR/final-inputs   # NFS 外
mkdir -p $M $K $R $I
for c in k2-bf-xattr-restart-campaign k4-cmp4-validity-analysis; do cp -r crates/campaign/cases/final/$c/materials $M/$c; done
cp -r crates/campaign/cases/final/k2-bf-xattr-restart-campaign/kb/. $K/
# R: k2 の初期 tree（BenchFS 086c4618。case.json の initial.fetch の手順）。k4 は repo 不要
cp <main-v3-n2-43706-public2.tgz の実物（公開入力 v2。ADR 2026-10-25-k4-public-input-v2）> $I/main-v3-n2-43706-public2.tgz   # prepare が同じ tgz を sha256 照合のうえ GRADE_CASES_ROOT/final/k4-…/ へ写す（grader が自分で展開する。ADR 2026-10-23-k4-cells-validity-grading D3）
# R（K2 初期 tree）の組立: bundle を clone → commit 086c4618 を checkout → submodule 5 件を lock の path へ snapshot tar 展開 → .git を消す
# （手順の実例: pinned/build-srcs.sh。asset_assembly::assemble_initial と同じ）
# K2 の組立後 tree には tree 内 symlink が 3 件ある（lib/locusta/CLAUDE.md・…/centos6/run-test.sh・…/ucx1-sys/ucx/README）。修正後 binary（§3a）は tree 内に閉じた symlink を許し、lock の content_sha256 と両立する（解決済み。実 K2 preflight exit 0 の証跡あり）。修正前の binary では preflight が exit 1 になる
# 主比較の入力は固定済み: $R/measure-src/main/{materials,kb,repo,inputs}（再組立不要。上の cp は新規に作り直すときの参考）   # sha256 が case.json の値と一致しなければ preflight/watch が拒否
```
`$M` の hash は `final.json` の `snapshot.materials_sha256` と一致しなければ拒否される。env の `MATERIALS_SRC/KB_SRC/REPO_SRC/INPUTS_SRC` にこの 4 つを書く。
A は bubblewrap の中で動く（`bwrap` 必須。preflight が実際に起動して確かめる）。A は `A_ACCOUNT` の `CLAUDE_CONFIG_DIR` で動き、既定の `~/.claude` は使わない。

## 5. 起動 → 照合 → 実行
```sh
sh $L prepare   --env $ENVF   # 必須入力の欠落を全件拒否 / manifest validate / config-check / 測定専用 token を 0600 で作成 / CELERIS_BIN の sha256 を固定
sh $L start     --env $ENVF   # config-check（不適切なら daemon を起こさず非 0）→ 固定 hash 一致の binary を `--config $DAEMON_CONFIG` だけで起動 → health 待ち → daemon 実体照合 → env-check。生きた自 daemon があれば再起動せず、同じ実体照合を通して接続
sh $L preflight --env $ENVF   # daemon 実体照合 → celeris-campaign preflight（env-check 込み）。有料実行 0 件（paid_calls_made 0）
echo $?                       # 0 = 全件 OK。非 0 は stderr JSON の `missing`（check 名 + 内容）を直してやり直す
sh $L watch     --env $ENVF   # daemon 実体照合 → preflight → env-lock（初回は作成、再開は照合）→ 実行直前に実体照合を再度 → watch。どれか不一致なら外部実行 0 件で非 0
```
- `measure-config`（`DAEMON_CONFIG`）・`normal-config`・`canaries`: **起動前**の `config-check`（`prepare` と `start` が daemon を起こす前に実行。daemon には接続しない）と preflight が、測定 config が本番と別の port・db・workspace・KB・記憶・llm_proxy・releases_dir・通知先であること、自動整理が無効であること、canary 値を照合する。不適切なら daemon を起動せず非 0。起動後は `env-check` が `GET /api/v1/config` の `identity`（instance・pid・release・config sha256・実効構成。秘密なし）を照合する: `identity.pid` が launcher 所有 daemon の PID（`--daemon-pid`）と一致、`config_sha256` が `DAEMON_CONFIG` の現物と一致、`overrides` が空、release が manifest の B/C と一致（`dev` は不可）、実効構成が `DAEMON_CONFIG` と一致。別 daemon の URL・起動後の config 書換え・版不一致・必須入力欠落・daemon 実行ファイルの差替えは、いずれも測定用外部実行 0 件で非 0。
- inputs: `--inputs-src` に case の宣言した file が全て在り sha256 が一致すること（k4 の `main-v3-n2-43706-public2.tgz`。k2 は `REPO_SRC`）を preflight が確かめ、watch が各試行 workspace の `inputs/<name>` に A/B/C 共通で置く。
- daemon 実体の照合（`start` の新規起動・既存 process への再接続、`preflight`、`watch` の全経路。`watch` は実行の直前にもう一度）: ① `CELERIS_BIN` の現物が `prepare` の固定 sha256 と一致、② launcher 所有 process（`$STATE_DIR/launcher.pid`・`launcher.meta` の starttime と cmdline の `--config`）が生きている、③ その実行像 `/proc/<pid>/exe` が起動時に記録した path・sha256 と一致し、その path の現物も同じ。同じ path の binary を差し替えると、再開時（watch・再接続）も `--version` が同じでも拒否される。
- 主比較（mlsys-main-1）は env-lock/3 と campaign lock が §3a で生成・照合済み（`pinned/main/`）。`watch` 初回が作る旧記述は一般手順で、主比較では `$OUT` へ複写した固定 lock と照合する。
- env-lock（`$OUT/env-lock.json`）: A の絶対 path・sha256・`--version`、B/C の release sha12、arm ごとの lower_policy・evidence_mode（none）・models・run_harnesses と**接続先 daemon の `GET /api/v1/llm/models` から取った候補集合と階層（catalog）**、worker 実行ファイル（`WORKER_PROGRAMS`）、`campaign_sha256` の固定記録。再開時は現物から作り直して突き合わせ、追加・削除・階層移動を key 名で挙げて拒否する。秘密値は持たない。
- account 状態（試行 identity 別）: A は `A_ACCOUNT` の認証材料 1 件（`.credentials.json`）だけを read-only で試行に重ね、`CLAUDE_CONFIG_DIR` は試行ごとに `$OUT/trial-state/<campaign_id>/A/<case>/rep<N>/` へ分ける。各 dir の `trial-identity.json`（campaign_id・manifest sha256・arm・case・rep・account_id）が identity marker で、別 identity の試行・別 lock・別 account は同じ dir を使えない。preflight（`trial-state` 検査）は、台帳に行がある試行は marker 一致、未開始の試行は dir が空（認証 mount 先の空 file と自分の marker だけ可）であることを要求し、CLAUDE.md・projects・transcripts・他試行の marker があれば拒否する。保存 lock が別 campaign のものでも拒否する。何も消さず、書き換えず、記憶も無効にしない。launcher は account を作らない・消さない・書き換えない。preflight は `GET /llm/accounts` でログイン済みと `account_dir_residue`（残留）も検査する。
- 標準起動は `watch`（`run` の 1 回走査を台帳が閉じるまで繰り返す。出力 JSON の `watch.end` が `settled` なら終端、`max_iterations` なら未完了が残る）。`budget.max_parallel`（1）により 1 回の走査で投入する試行は 1 つ。`executor_errors>0` なら `$OUT` の `trials.jsonl` の `error` 欄を見る。
- 出力 JSON（`executed / waiting / parallel_deferred / no_artifact / executor_errors / blocked / budget_cancelled / tokens_total / tokens_missing / max_overshoot_*`）は `RUNS=$(dirname "$OUT")/runs; mkdir -p "$RUNS"` で作る `$OUT` の外の dir に、`sh $L watch … > $RUNS/watch-N.json`（N は実行回数）として保存する。`$OUT` に空 file でも置くと「out has data but no lock.json」で拒否される（lock 判定は変えない。ADR 2026-10-20-mlsys-main2-fixes D4）。

## 6. 7 日使用率（run のたびに。launcher の subcommand に無いので CLI を直接。引数は launcher と同じ接続先）
```sh
export CELERIS_MEASURE_TOKEN="$(cat <DAEMON_CONFIG の [api] token_file>)"   # 値は表示・記録しない
$B usage --manifest $MANIFEST --out $OUT --api-url http://<DAEMON_CONFIG の [api] listen> --token-env CELERIS_MEASURE_TOKEN --a-account $A_ACCOUNT > $RUNS/usage-N.json
```
exit 0 = A のアカウントを含む claude が 45%・codex が 21% 以内（`--max-7d` / `--max-7d-codex`。承認枠を超える変更は新しい決定）。使用率が観測できない（null）ときは非 0。腕ごとの tokens は `campaign_tokens_by_arm`。

## 7. 停止と再開
- 停止（実在引数は `--env` のみ）: `sh $L stop --env $ENVF`。signal を送るのは `$STATE_DIR/launcher.pid` の pid 1 つだけで、`launcher.meta` の starttime と cmdline の `--config $DAEMON_CONFIG` が一致しなければ**何も送らず非 0**（pid 再利用・別 process を守る）。止める前に実行像も記録と照合する。SIGTERM → 上限で SIGKILL。本番 daemon・systemd・account には触れない。
- `watch` の停止（Ctrl-C・host 再起動・`max_iterations` 到達）・`waiting>0`・daemon 再起動の後は、`start`（必要なら）→ **同じ `watch` コマンドをそのまま**再実行する。毎回 daemon 実体照合・preflight（env-check）・env-lock の照合を通り（`start` の再接続も同じ実体照合。別 daemon・差し替えられた binary・変わった catalog は外部実行 0 件で拒否）、台帳から同じ試行に再接続する（A の process は `harness-state/<key>/meta.json` の pid・start ticks・上限で同じ物を追う）。二重投入はせず観測済み usage は消えない。同じ out に watch を同時に 2 つ走らせない。別 hash で同じ `OUT` は拒否される。
- 停止中の A の process は上限なしで走り続けうる。再開後の最初の走査（poll）が同じ上限で判定して kill する。実行側の歯止めは `trial_timeout_secs` と `per_case.max_wall_secs`。token 上限は「観測した累積が上限を超えたら止める」で絶対ではない（最大超過は `summary.json` の `max_overshoot_*`）。
- kill 後に process が消えたこと、B/C は task 木と cluster job の終端を確認するまで台帳を閉じない。確認できない間 `watch` は終わらない。

## 8. 検査・独立採点・結果回収
```sh
sh $L collect --env $ENVF  # = `celeris-campaign collect-observed-out`（実験 case の trial ごとに評価側の job 終了記録を <OUT>/observed/ へ。単独でも実行できる）
sh $L grade  --env $ENVF   # = collect-observed-out（失敗は警告のみ）+ `celeris-campaign grade-out --cases $GRADE_CASES_ROOT [--initial-trees $INITIAL_TREES]`（独立採点 → $OUT/grades.jsonl）+ `check`。主比較は GRADE_CASES_ROOT=$R/pinned/main/fixed/grade-cases
sh $L export --env $ENVF   # export を $OUT/export へ。RECORDS があれば final_accounting（総消費量）も含める。無ければ警告し、表の consumption は欠測（0 ではない）
```
- `grade` は主比較の 4 試行（A・B × K2・K4）すべてに判定を付ける。失敗・timeout・cancelled・未完了も分母に残す。grade-case が無い problem（root 直下 `<problem_id>.json` か `final/<problem>/<problem>.json`）は `missing_cases` で非 0。`check` は `ok true`・violations 空。`complete=false` なら `incomplete_reasons` を読む。
- 評価側の観測（`collect`）: 台帳に Celeris が残した job ID（`scheduler:cluster:job_id`、素の `<n>.<server>` は case の cluster の PBS とみなす）を `ssh <cluster> qstat -x -f`（read-only）で照会し、記録に campaign sha256・arm・problem・rep・trial_no（`trial`）と scheduler 応答の控え（`observed/<problem>/scheduler/`）の sha256 を結ぶ。job 未終了（`job_state` が `F` 以外）・台帳に job が無い・成果物が実行終了時の hash と違う、のどれでも記録は作られず `collect` は非 0（grade は警告のみで続き、その trial は観測なしの実験証拠として成功にならない）。`grade-out` は記録の `trial` が当該 trial と違う・job 集合が台帳と違う・控えが改変されている記録を拒否する（`graded:false`、非 0）。
- `grade-out` 単体の再判定が要るときは `$B grade --case $R/pinned/main/fixed/grade-cases/final/<case-id>/<case-id>.json --artifact $OUT/trials/<A|B|C>/<case-id>/rep1/workspace`（`celeris-campaign grade`）。
- final_accounting を単独で見る: `$B final-accounting --manifest $MANIFEST --out $OUT --records $RECORDS`。
- 件数と分母の照合: `python3 scripts/campaign/check-out-consistency.py --out $OUT --manifest $MANIFEST --campaign-bin $B --check-arg=…（launcher と同じ資産の引数）--human-ops-report <report dir>`。checker・export・human-op report の成功/失敗/欠測の件数を生記録からの再計算と突き合わせ、一致で exit 0。checker は launcher と同じ `--measurement-id`・資産で呼ぶ（省くと期待 lock が合わず採点が外れ、report とずれる）。
- 結果回収の固定 hash 一覧（§3a と同じ。回収前後で変わったら採点・export を無効として止める）: campaign_sha256 `8ca0e198…a999`、`pinned/main/lock.json` file sha256 `8ddc8384…2343`、`env-lock.json` `6c2275f3…986c`、daemon release `c9654bff5633` / binary `be62f43d…a3d2`、campaign binary `de339f1c…1394`、final.json sha256 `0aaa6a23…fa36`。確認は `python3 scripts/campaign/check-main-pins.py --root $R`。
- Celeris 側の消費・判断ログ（人。本番 DB は読むだけ、写しに対して）:
```sh
celerisctl db backup $TMPDIR/final-db.sqlite3 --src <本番 DB path>
celerisctl --db $TMPDIR/final-db.sqlite3 campaign export mlsys-main-1 --out $RUNS/exp-db --grades $OUT/grades.jsonl --verify-connected --manifest-hash <§3 の campaign sha256>
```
`summary.json` の `shared_overhead`・`elapsed_ms` と `run_wall_sum_ms`・`total.cash_cost_usd`（取れなければ null）・終了時の 7 日使用率を進捗に転記する。

## 9. 注意
- 本番 host 操作（昇格・systemctl・config 変更・cluster job の cancel）は launcher の対象外。この文書では §0 の人の手順だけ。
- pilot の結果は較正専用で、本測定の主張に混ぜない。dev の case・入力・設定を final に持ち込まない。
- 旧い手書き手順（`--api-url`/`--token-env` を手で書く run/watch、手書きの A 起動、手での daemon 起動）は消した。置き換えは上の launcher。

## 10. 状態区分（2026-10-19 更新）
| 区分 | 項目 |
|---|---|
| 実装済み | `measure-launcher.sh`（prepare/start/preflight/watch/grade/export/stop）、起動前 `config-check`、daemon 実体照合（start 新規・再接続・preflight・watch）、`env-lock`（catalog・階層を実 daemon の `GET /api/v1/llm/models` から固定・照合）、`env-check`（identity.pid と launcher 所有 PID の照合を含む）、`GET /api/v1/config` の `identity`、試行 identity 別の `trial-state` と preflight 検査、`final-accounting` |
| 無 LLM 検証済み | `crates/campaign/tests/{launcher,env_lock,env_check}.rs`、daemon identity の試験。launcher 試験は launcher 所有 process が実 task-api router を serve し、実 CLI で task 投入→watch→採点→export まで進む成功系と、不適切な config の起動拒否・別 daemon・binary 差替え・版不一致・必須入力欠落の拒否（外部実行 0 件）、再開時の固定記録一致、stop が他 process に届かないことを含む（stub は A の claude・bwrap・実行内部のみ） |
| 実経路確認済み（主比較・無課金） | 実 release `c9654bff5633` の測定専用 daemon に対し、修正後 binary で実 K2 tree・正解資産の `preflight` が exit 0・`paid_calls_made 0`（証跡 §3a、`check-main-pins.py` が検証）。試行（`watch`）・実 claude・実 B/C の実行は未実施。旧 pilot の結果は流用しない |
| 承認待ち（人の決定） | 本測定の開始（§5 の `watch`）。release・A 版・account・CAP_ENV・daemon 配置は主比較では承認・配置済み（§3a）。選択機構統合後のセット別最終 preflight は task 01M58VG5C4XGK4E70M6WJ6MEX0 |
| 未検証 / 未実装 | `/proc/net` による port 所有の照合、試行ごとの A 実体 sha256 取り直し（A は preflight と env-lock の時点で固定。起動直前の再取得は無い）、`usage` の launcher subcommand 化（§6 は CLI 直接）、`--env` 一本化と旧 CLI 欄の削除（launcher は env ファイルから CLI 引数を 1 回だけ生成して使う） |

修正 commit（main 取り込み前の branch 上）: `dd7c5564` daemon identity、`0cd8eab5` env-check、`4d09f56c` launcher と無 LLM e2e、`5adf1ab2` env-lock catalog・pid 照合、`93948ed4` 起動前 config-check・全経路の実体照合・成功系試験。
