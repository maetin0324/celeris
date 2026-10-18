---
title: "MLSys 本測定 run-final: launcher（measure-launcher.sh）で測定環境を起動・照合して回す手順"
tasks: [01M52D2AM5K6GCFGEJZW4REWM2, 01M56HCNS9SYAS7TAEA7Z3E4CZ]
unit: runbook
status: ready-for-operator
updated: 2026-10-18
---
# run-final（運用セッション用。worker・agent は実行しない）

根拠: ADR `agent-docs/adr/2026-10-18-measure-env-launcher.md`、実装記録 `../2026-10-18-measure-env-launcher/`（identity-design / daemon-identity-api / env-lock / env-check / launcher）、条件 `freeze-final.md`。
**値（token・鍵）はこのファイルに書かない。** token は測定専用 file（`[api] token_file`）に launcher が 0600 で作り、CLI には環境変数の**名前**（`CELERIS_MEASURE_TOKEN`）だけが渡る。
承認枠: 試行 12 以内・tokens 全体 3,000,000・per_case 300,000・per_arm 1,300,000・`token_basis = all`・claude 7 日使用率 45% 以内（codex 21%）・fable 不使用（人の決定 2026-10-16）。枠を超える変更は新 campaign_id と新しい決定が要る。

入口は 1 つ: `scripts/campaign/measure-launcher.sh <prepare|start|preflight|watch|grade|export|stop> --env <file>`。各 subcommand は再実行できる。
接続先（`--api-url`）・token・`--measure-config` 等の CLI 引数は、すべて env ファイル（と、その `DAEMON_CONFIG` の `[api] listen` / `token_file`）から launcher が 1 回だけ生成する。**手で `--api-url` や `--token-env` を書かない。**
launcher がやらないこと: systemd / `systemctl` / `pkill` / 本番 config・本番 DB・本番 release の変更、昇格、cluster job の cancel、account の作成・削除・書換え。停止は launcher 自身が起動した daemon の pid 1 つだけ（§7）。

## 0. 前提（人。本番 host 操作。launcher は行わない）
| 項目 | 内容 |
|---|---|
| H1 | B/C の Celeris release を昇格し sha12 を控える（GUI / `promote.sh`）。launcher は昇格しない |
| H2 | `GET /api/v1/secrets` で `jev_api_key` の有無を確認 |
| H3 | A 用の claude_oauth アカウントをログイン済みにして `account_id` を控える（`GET /api/v1/llm/accounts`） |
| H4 | 測定専用の daemon config（本番と別の port・db・workspace・llm_proxy 無効または別 port・`push=false`・Discord 通知なし）と、突き合わせ用の通常 config（読むだけ）を用意する |
| H5 | 7 日使用率の開始値 `U0`（codex / claude）を控える |
| H6 | cancel された試行の cluster job（`$OUT/*/cluster-cancel.jsonl` の `scancel`/`qdel`）の実行 |

## 1. 記号
```sh
cd <この repo の checkout>
export CARGO_TARGET_DIR=...                # 渡された値のまま
L=scripts/campaign/measure-launcher.sh
B=$CARGO_TARGET_DIR/debug/celeris-campaign # env の CAMPAIGN_BIN にも同じ path を書く
P=agent-docs/progress/2026-10-16-mlsys-final
ENVF=<measure-launcher.env>                # §2。相対 path は env ファイルの dir 基準
```

## 2. env ファイル（唯一の設定源。`KEY=VALUE` の sh 形式、値に秘密を書かない）
必須（欠けると `prepare` が全件を挙げて非 0）: `MANIFEST` `DAEMON_CONFIG` `NORMAL_CONFIG` `CANARIES` `OUT` `STATE_DIR` `TASK_TEMPLATE` `A_ACCOUNT` `MATERIALS_SRC` `KB_SRC` `REPO_SRC` `INPUTS_SRC`。
`start` が加えて要る: `CELERIS_BIN`（測定 daemon の実行ファイル）`RELEASE`（B/C の sha12）。
任意: `CAMPAIGN_BIN`（既定 `celeris-campaign`）`A_PROGRAM`（既定 `claude`）`RUN_PATH`（先頭に足す PATH）`WORKER_PROGRAMS`（`name=path` を空白区切り。env-lock に固定）`MEASUREMENT_ID` `CASES_ROOT` `GRADER_ROOT` `GRADE_CASES_ROOT`（grade に必須）`RECORDS`（export で final_accounting）`EXECUTOR`（既定 `celeris`）`INTERVAL_SECS`（60）`MAX_ITERATIONS`（1440）`HEALTH_WAIT_SECS`（60）。
```sh
MANIFEST=final.json                # §3 で置換済みの manifest
DAEMON_CONFIG=/abs/measure/daemon.toml
NORMAL_CONFIG=~/.config/celeris/config.toml   # 突き合わせ専用。読むだけ
CANARIES=canaries.json             # measure-config の canary 値の宣言
TASK_TEMPLATE=task-template.json
A_ACCOUNT=<claude_oauth account_id>
CELERIS_BIN=/abs/measure/celeris   # prepare が sha256 を固定する
RELEASE=<sha12>
OUT=/abs/local/final-out ; STATE_DIR=/abs/local/measure-state     # NFS 外
MATERIALS_SRC=… KB_SRC=… REPO_SRC=… INPUTS_SRC=…                   # §4
CASES_ROOT=crates/campaign/cases ; GRADER_ROOT=crates/campaign/fixtures/grader-private
GRADE_CASES_ROOT=$P/grade-case ; RECORDS=…/records.jsonl
```

## 3. manifest と task template の確定（H1 後に 1 回だけ。置換すると hash が変わる）
- `$P/final.json`: B・C の `harness_version` の `<release sha12 at H1>` → 昇格した sha12。A の `claude-code@<claude-code version at H1>` → 実際の版（alias 不可）。C の `routing.evidence_table_id` → `celerisctl routing evidence` の版。`lower_policy`/`online_update` は変えない。`budget.token_basis` は `all` 固定。
- `$P/task-template.json`: `project_id`（campaign 専用案件。`auto_integrate=false`、repo はローカルの空 git repo かなし。作成は人が `POST /api/v1/projects` と `/projects/<id>/repos`）、`repos:["scratch"]`、acceptance は `{"type":"reviewer","text":…}`。
- 検証（launcher を経ず直接。API を使わない）:
```sh
$B validate --manifest $P/final.json      # sha256 を控える。以後 manifest・入力を変えない（変えるなら新 campaign_id）
$B dry-run  --manifest $P/final.json      # within_budget true、total_trials 6
```

## 4. 入力・sandbox 資産の配送（試行へ渡す公開部だけ。grader-private は渡さない）
```sh
M=$TMPDIR/final-materials; K=$TMPDIR/final-kb; R=$TMPDIR/final-repo; I=$TMPDIR/final-inputs   # NFS 外
mkdir -p $M $K $R $I
for c in k2-bf-xattr-restart-campaign k4-cmp4-validity-analysis; do cp -r crates/campaign/cases/final/$c/materials $M/$c; done
cp -r crates/campaign/cases/final/k2-bf-xattr-restart-campaign/kb/. $K/
# R: k2 の初期 tree（BenchFS 086c4618。case.json の initial.fetch の手順）。k4 は repo 不要
cp <main-v3-n2-43706.tgz の実物> $I/main-v3-n2-43706.tgz   # sha256 が case.json の値と一致しなければ preflight/watch が拒否
```
`$M` の hash は `final.json` の `snapshot.materials_sha256` と一致しなければ拒否される。env の `MATERIALS_SRC/KB_SRC/REPO_SRC/INPUTS_SRC` にこの 4 つを書く。
A は bubblewrap の中で動く（`bwrap` 必須。preflight が実際に起動して確かめる）。A は `A_ACCOUNT` の `CLAUDE_CONFIG_DIR` で動き、既定の `~/.claude` は使わない。

## 5. 起動 → 照合 → 実行
```sh
sh $L prepare   --env $ENVF   # 必須入力の欠落を全件拒否 / manifest validate / 測定専用 token を 0600 で作成 / CELERIS_BIN の sha256 を固定
sh $L start     --env $ENVF   # 固定 hash 一致の binary を `--config $DAEMON_CONFIG` だけで起動 → health 待ち → env-check。生きた自 daemon があれば再起動せず接続
sh $L preflight --env $ENVF   # celeris-campaign preflight（env-check 込み）。有料実行 0 件（paid_calls_made 0）
echo $?                       # 0 = 全件 OK。非 0 は stderr JSON の `missing`（check 名 + 内容）を直してやり直す
sh $L watch     --env $ENVF   # preflight → env-lock（初回は作成、再開は照合）→ watch。どれか不一致なら外部実行 0 件で非 0
```
- `measure-config`（`DAEMON_CONFIG`）・`normal-config`・`canaries`: preflight が測定 config が本番と別の port・db・llm_proxy・releases_dir・通知先であることと、canary 値を照合する。`env-check` は `GET /api/v1/config` の `identity`（instance・state・release・実効構成。秘密なし）が `DAEMON_CONFIG` と一致すること、`overrides` が空であることを確かめる。別 daemon の URL・起動後の config 書換え・版不一致・必須入力欠落・daemon 実行ファイルの差替えは、いずれも測定用外部実行 0 件で非 0。
- inputs: `--inputs-src` に case の宣言した file が全て在り sha256 が一致すること（k4 の `main-v3-n2-43706.tgz`。k2 は `REPO_SRC`）を preflight が確かめ、watch が各試行 workspace の `inputs/<name>` に A/B/C 共通で置く。
- env-lock（`$OUT/env-lock.json`）: A の絶対 path・sha256・`--version`、B/C の release sha12、arm ごとの lower_policy・evidence_table_id・models、worker 実行ファイル、`campaign_sha256` の固定記録。秘密値は持たない。**同一 path で binary が差し替わると `--version` が同じでも sha256 で拒否される。**
- account 状態: 試行 identity 別の account 対応（作業 1）は**未統合**（下の状態区分）。現状は env の `A_ACCOUNT` 1 件。launcher は account を作らない・消さない・書き換えない。preflight が `GET /llm/accounts` でログイン済みと `account_dir_residue`（残留）を検査する。
- 標準起動は `watch`（`run` の 1 回走査を台帳が閉じるまで繰り返す。出力 JSON の `watch.end` が `settled` なら終端、`max_iterations` なら未完了が残る）。`budget.max_parallel`（1）により 1 回の走査で投入する試行は 1 つ。`executor_errors>0` なら `$OUT` の `trials.jsonl` の `error` 欄を見る。
- 出力 JSON（`executed / waiting / parallel_deferred / no_artifact / executor_errors / blocked / budget_cancelled / tokens_total / tokens_missing / max_overshoot_*`）は `sh $L watch … > $OUT/final-run-N.json`（N は実行回数）に保存する。

## 6. 7 日使用率（run のたびに。launcher の subcommand に無いので CLI を直接。引数は launcher と同じ接続先）
```sh
export CELERIS_MEASURE_TOKEN="$(cat <DAEMON_CONFIG の [api] token_file>)"   # 値は表示・記録しない
$B usage --manifest $MANIFEST --out $OUT --api-url http://<DAEMON_CONFIG の [api] listen> --token-env CELERIS_MEASURE_TOKEN --a-account $A_ACCOUNT > $OUT/usage-N.json
```
exit 0 = A のアカウントを含む claude が 45%・codex が 21% 以内（`--max-7d` / `--max-7d-codex`。承認枠を超える変更は新しい決定）。使用率が観測できない（null）ときは非 0。腕ごとの tokens は `campaign_tokens_by_arm`。

## 7. 停止と再開
- 停止: `sh $L stop --env $ENVF`。signal を送るのは `$STATE_DIR/launcher.pid` の pid 1 つだけで、`launcher.meta` の starttime と cmdline の `--config $DAEMON_CONFIG` が一致しなければ**何も送らず非 0**（pid 再利用・別 process を守る）。SIGTERM → 上限で SIGKILL。本番 daemon・systemd・account には触れない。
- `watch` の停止（Ctrl-C・host 再起動・`max_iterations` 到達）・`waiting>0`・daemon 再起動の後は、`start`（必要なら）→ **同じ `watch` コマンドをそのまま**再実行する。毎回 preflight と env-lock の照合を通り、台帳から同じ試行に再接続する（A の process は `harness-state/<key>/meta.json` の pid・start ticks・上限で同じ物を追う）。二重投入はせず観測済み usage は消えない。同じ out に watch を同時に 2 つ走らせない。別 hash で同じ `OUT` は拒否される。
- 停止中の A の process は上限なしで走り続けうる。再開後の最初の走査（poll）が同じ上限で判定して kill する。実行側の歯止めは `trial_timeout_secs` と `per_case.max_wall_secs`。token 上限は「観測した累積が上限を超えたら止める」で絶対ではない（最大超過は `summary.json` の `max_overshoot_*`）。
- kill 後に process が消えたこと、B/C は task 木と cluster job の終端を確認するまで台帳を閉じない。確認できない間 `watch` は終わらない。

## 8. 検査・独立採点・結果回収
```sh
sh $L grade  --env $ENVF   # grade-out（GRADE_CASES_ROOT の <problem_id>.json で独立採点 → $OUT/grades.jsonl）+ check
sh $L export --env $ENVF   # export を $OUT/export へ。RECORDS があれば final_accounting（総消費量）も含める
```
- `grade` は 6 試行すべてに判定を付ける。失敗・timeout・cancelled・未完了も分母に残す。grade-case が無い problem は `missing_cases` で非 0。`check` は `ok true`・violations 空。`complete=false` なら `incomplete_reasons` を読む。
- `grade-out` 単体の再判定が要るときは `$B grade --case $P/grade-case/<case-id>.json --artifact $OUT/trials/<A|B|C>/<case-id>/rep1/workspace`（`celeris-campaign grade`）。
- final_accounting を単独で見る: `$B final-accounting --manifest $MANIFEST --out $OUT --records $RECORDS`。
- Celeris 側の消費・判断ログ（人。本番 DB は読むだけ、写しに対して）:
```sh
celerisctl db backup $TMPDIR/final-db.sqlite3 --src <本番 DB path>
celerisctl --db $TMPDIR/final-db.sqlite3 campaign export mlsys-final-1 --out $OUT/exp-db --verify-connected --manifest-hash <§3 の sha256>
```
`summary.json` の `shared_overhead`・`elapsed_ms` と `run_wall_sum_ms`・`total.cash_cost_usd`（取れなければ null）・終了時の 7 日使用率を進捗に転記する。

## 9. 注意
- 本番 host 操作（昇格・systemctl・config 変更・cluster job の cancel）は launcher の対象外。この文書では §0 の人の手順だけ。
- pilot の結果は較正専用で、本測定の主張に混ぜない。dev の case・入力・設定を final に持ち込まない。
- 旧い手書き手順（`--api-url`/`--token-env` を手で書く run/watch、手書きの A 起動、手での daemon 起動）は消した。置き換えは上の launcher。

## 10. 状態区分（2026-10-18。修正 commit は下）
| 区分 | 項目 |
|---|---|
| 実装済み | `measure-launcher.sh`（prepare/start/preflight/watch/grade/export/stop）、`env-lock`、`env-check`、`GET /api/v1/config` の `identity`、preflight への結合 |
| 無 LLM 検証済み | `crates/campaign/tests/{launcher,env_lock,env_check}.rs`、daemon-identity-api の試験（stub は A の claude・bwrap・測定 daemon 実行ファイルのみ。実 CLI・実 task-api router）。別 daemon・config 相違・binary 差替え・版不一致・必須入力欠落が外部実行 0 件で拒否、一致時は watch→採点→export、再開は固定記録と一致確認、固定記録とログに秘密なし、stop は他 process に届かない |
| 実経路確認済み | なし（launcher 経由で実 `celeris` daemon・実 claude・実 B/C を動かした確認は未実施。旧 pilot の結果は流用しない） |
| 承認待ち | 本測定の開始（人の決定）、H1 昇格、H3 account、H4 測定専用 config |
| 未検証 / 未実装 | 試行 identity 別 account 状態（作業 1）との結合（現状は `A_ACCOUNT` 1 件）、ADR の `--env` 一本化と旧 CLI 欄の削除（launcher は旧 CLI 欄を生成して使う）、D2-5/6（/proc・port 所有）、試行ごとの A 実体 sha256 取り直し、launcher 経由の celeris executor 実 daemon での確認、`usage` / `final-accounting` の launcher subcommand 化 |

修正 commit（この branch）: `f7dfa49f` daemon identity API、`d08f8b35` env-check、`0ee51a36` measure-launcher.sh と無 LLM e2e（env-lock は `619dc480` で統合）、runbook 更新は本 WU の commit。
