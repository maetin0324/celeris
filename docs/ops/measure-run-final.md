# MLSys 本測定: measure-launcher.sh で測定環境を起動・照合して回す手順（運用セッション用。worker・agent は実行しない）

---
tasks: [01M52D2AM5K6GCFGEJZW4REWM2, 01M56HCNS9SYAS7TAEA7Z3E4CZ, 01M58VFRBA6HG0SSF3CG5BKM91]
---

根拠: ADR `agent-docs/adr/2026-10-18-measure-env-launcher.md`、条件 `agent-docs/progress/2026-10-16-mlsys-final/freeze-final.md`。実装は `scripts/campaign/measure-launcher.sh` と `crates/campaign/src/{env_check,env_lock,measure_env,preflight}.rs`。
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
| H4 | 測定専用の daemon config（§3b の雛形から。`cap-probe-claude-code` 宣言入り。本番と別の port・db・workspace・llm_proxy 無効または別 port・`push=false`・Discord 通知なし）と、突き合わせ用の通常 config（読むだけ）を用意する |
| H5 | 7 日使用率の開始値 `U0`（codex / claude）を控える |
| H6 | cancel された試行の cluster job（`$OUT/*/cluster-cancel.jsonl` の `scancel`/`qdel`）の実行 |

## 1. 記号
```sh
cd <この repo の checkout>
export CARGO_TARGET_DIR=...                # 渡された値のまま
L=scripts/campaign/measure-launcher.sh
B=$CARGO_TARGET_DIR/debug/celeris-campaign # env の CAMPAIGN_BIN にも同じ path を書く
P=agent-docs/progress/2026-10-16-mlsys-final   # final.json・task-template.json・daemon/env 雛形の置き場
ENVF=<measure-launcher.env>                # §2。相対 path は env ファイルの dir 基準
```

## 2. env ファイル（唯一の設定源。`KEY=VALUE` の sh 形式、値に秘密を書かない）
雛形: `$P/measure-launcher.example.env`（複製して `<operator 入力待ち: …>` を実値にする。残っていると `prepare`/`preflight` が拒否する）。
必須（欠けると `prepare` が `<KEY> is required` で非 0）: `MANIFEST` `DAEMON_CONFIG` `NORMAL_CONFIG` `CANARIES` `OUT` `STATE_DIR` `TASK_TEMPLATE` `A_ACCOUNT` `MATERIALS_SRC` `KB_SRC` `REPO_SRC` `INPUTS_SRC`。
`start` が加えて要る: `CELERIS_BIN`（測定 daemon の実行ファイル。`prepare` が sha256 を固定）`RELEASE`（B/C の sha12）。`CAP_ENV` は本測定では事実上必須（無いと preflight の cap-probe が拒否）。
任意: `CAMPAIGN_BIN`（既定 `celeris-campaign`。`DAEMON_CONFIG` の `cap-probe-*` の command と同じ実体にする）`A_PROGRAM`（既定 `claude`）`RUN_PATH` `WORKER_PROGRAMS`（`name=path` を空白区切り。env-lock に固定）`MEASUREMENT_ID` `CASES_ROOT` `GRADER_ROOT` `GRADE_CASES_ROOT`（`grade` に必須）`RECORDS`（export で final_accounting）`EXECUTOR`（既定 `celeris`）`INTERVAL_SECS`（60）`MAX_ITERATIONS`（1440）`HEALTH_WAIT_SECS`（60）。
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

## 3. 確定済みの構成と、H1 後に埋める値（`$P/final.json` は 1 回だけ更新。変えると hash が変わる）
確定済み（2026-10-19、`agent-docs/progress/2026-10-19-mlsys-final-config/`）:
- B・C は同じ `routing.models=["claude-sonnet-5-5"]`・`routing.run_harnesses=["claude-code"]`・`lower_policy=mlsys-min-1`。方策だけが違う（B=fixed、C=jev-1）。A は claude-code / claude-sonnet-5-5 で集合の内側。`cap-probe-*` は run_harnesses に入れない。
- C は `no_evidence_table=true`（根拠表なし。`effective_evidence_mode=none`）、B は根拠表欄なしで none。**`evidence_table_id` と `celerisctl routing evidence` の手順は無い（消した）。latest へ fallback しない。**
- 比較: primary A vs B、secondary B vs C。subset は K2・K4。K2 `judged_scope=[implementation, experiment]`（analysis / review は `not_judged`。維持する。ADR 2026-10-18-k2-judged-scope）、K4 は analysis。
- `$P/task-template.json`: `project_id=01M2WTS3DKNZBSZ2JMVB4CZMBW`、`repos:["scratch"]`、acceptance は reviewer。
- `budget.token_basis=all`。`lower_policy`/`online_update` は変えない。

H1 後に operator が置換する値（`validate` が placeholder を拒否する。秘密値は書かない）:
1. B・C の `harness_version` の release sha12（12 桁 hex。final.json の B・C 欄）。
2. A の `harness_version` = `claude-code@<実際の版>`（alias 不可）。
3. env の `A_ACCOUNT`・`RELEASE`・`CELERIS_BIN`・`CAP_ENV`・各 path。

置換後（launcher を経ず直接。API を使わない）:
```sh
$B validate --manifest $P/final.json      # exit 0。campaign sha256 を控える。以後 manifest・入力を変えない（変えるなら新 campaign_id）
$B dry-run  --manifest $P/final.json      # within_budget true、total_trials 6
```
固定 hash（`manifest-fix.md` 参照）: final-input-manifest.json 内部 sha256 `6fa8f0ec…06e4b`、`snapshot.materials_sha256` `dab3ca05…800d`、`snapshot.kb_snapshot` `741f2414…9fe0702`。これらは置換で変わらない。final.json 本体と campaign sha256 は置換後に取り直す。env-lock/3 と campaign lock は `watch` の初回が `$OUT` に作る（旧測定の out に上書きしない）。

## 3b. 測定専用 daemon の宣言（人が配置。本番 config・release は触らない）
`$P/measure-daemon.example.toml` を複製し `<...>` を実値にする（db・workspace・KB・memory・token_file・port は本番と別、NFS 外）。内容: 実 harness は `claude-code` だけ、`cap-probe-claude-code`（kind=fake・`env_of="claude-code"`・command=`CAMPAIGN_BIN`・args `cap-worker`）、`[llm_sources.cap-probe]`、`[route_defaults.cap-probe-claude-code]`、`harness_selection.rules` 先頭に分野 `cap-probe-claude-code` の規則。probe は分野 `cap-probe-*` の task にだけ結ばれ、実試行の候補に入らない。宣言を欠くと preflight の B/C cap-probe が投入前に `env_failure` で拒否する。`NORMAL_CONFIG` の `[[clusters]]` を複写する。

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
sh $L prepare   --env $ENVF   # 必須入力の欠落を全件拒否 / manifest validate / config-check / 測定専用 token を 0600 で作成 / CELERIS_BIN の sha256 を固定
sh $L start     --env $ENVF   # config-check（不適切なら daemon を起こさず非 0）→ 固定 hash 一致の binary を `--config $DAEMON_CONFIG` だけで起動 → health 待ち → daemon 実体照合 → env-check。生きた自 daemon があれば再起動せず、同じ実体照合を通して接続
sh $L preflight --env $ENVF   # daemon 実体照合 → celeris-campaign preflight（env-check 込み）。有料実行 0 件（paid_calls_made 0）
echo $?                       # 0 = 全件 OK。非 0 は stderr JSON の `missing`（check 名 + 内容）を直してやり直す
sh $L watch     --env $ENVF   # daemon 実体照合 → preflight → env-lock（初回は作成、再開は照合）→ 実行直前に実体照合を再度 → watch。どれか不一致なら外部実行 0 件で非 0
```
- `measure-config`（`DAEMON_CONFIG`）・`normal-config`・`canaries`: **起動前**の `config-check`（`prepare` と `start` が daemon を起こす前に実行。daemon には接続しない）と preflight が、測定 config が本番と別の port・db・workspace・KB・記憶・llm_proxy・releases_dir・通知先であること、自動整理が無効であること、canary 値を照合する。不適切なら daemon を起動せず非 0。起動後は `env-check` が `GET /api/v1/config` の `identity`（instance・pid・release・config sha256・実効構成。秘密なし）を照合する: `identity.pid` が launcher 所有 daemon の PID（`--daemon-pid`）と一致、`config_sha256` が `DAEMON_CONFIG` の現物と一致、`overrides` が空、release が manifest の B/C と一致（`dev` は不可）、実効構成が `DAEMON_CONFIG` と一致。別 daemon の URL・起動後の config 書換え・版不一致・必須入力欠落・daemon 実行ファイルの差替えは、いずれも測定用外部実行 0 件で非 0。
- inputs: `--inputs-src` に case の宣言した file が全て在り sha256 が一致すること（k4 の `main-v3-n2-43706.tgz`。k2 は `REPO_SRC`）を preflight が確かめ、watch が各試行 workspace の `inputs/<name>` に A/B/C 共通で置く。
- daemon 実体の照合（`start` の新規起動・既存 process への再接続、`preflight`、`watch` の全経路。`watch` は実行の直前にもう一度）: ① `CELERIS_BIN` の現物が `prepare` の固定 sha256 と一致、② launcher 所有 process（`$STATE_DIR/launcher.pid`・`launcher.meta` の starttime と cmdline の `--config`）が生きている、③ その実行像 `/proc/<pid>/exe` が起動時に記録した path・sha256 と一致し、その path の現物も同じ。同じ path の binary を差し替えると、再開時（watch・再接続）も `--version` が同じでも拒否される。
- env-lock（`$OUT/env-lock.json`）: A の絶対 path・sha256・`--version`、B/C の release sha12、arm ごとの lower_policy・evidence_mode（none）・models・run_harnesses と**接続先 daemon の `GET /api/v1/llm/models` から取った候補集合と階層（catalog）**、worker 実行ファイル（`WORKER_PROGRAMS`）、`campaign_sha256` の固定記録。再開時は現物から作り直して突き合わせ、追加・削除・階層移動を key 名で挙げて拒否する。秘密値は持たない。
- account 状態（試行 identity 別）: A は `A_ACCOUNT` の認証材料 1 件（`.credentials.json`）だけを read-only で試行に重ね、`CLAUDE_CONFIG_DIR` は試行ごとに `$OUT/trial-state/<campaign_id>/A/<case>/rep<N>/` へ分ける。各 dir の `trial-identity.json`（campaign_id・manifest sha256・arm・case・rep・account_id）が identity marker で、別 identity の試行・別 lock・別 account は同じ dir を使えない。preflight（`trial-state` 検査）は、台帳に行がある試行は marker 一致、未開始の試行は dir が空（認証 mount 先の空 file と自分の marker だけ可）であることを要求し、CLAUDE.md・projects・transcripts・他試行の marker があれば拒否する。保存 lock が別 campaign のものでも拒否する。何も消さず、書き換えず、記憶も無効にしない。launcher は account を作らない・消さない・書き換えない。preflight は `GET /llm/accounts` でログイン済みと `account_dir_residue`（残留）も検査する。
- 標準起動は `watch`（`run` の 1 回走査を台帳が閉じるまで繰り返す。出力 JSON の `watch.end` が `settled` なら終端、`max_iterations` なら未完了が残る）。`budget.max_parallel`（1）により 1 回の走査で投入する試行は 1 つ。`executor_errors>0` なら `$OUT` の `trials.jsonl` の `error` 欄を見る。
- 出力 JSON（`executed / waiting / parallel_deferred / no_artifact / executor_errors / blocked / budget_cancelled / tokens_total / tokens_missing / max_overshoot_*`）は `sh $L watch … > $OUT/final-run-N.json`（N は実行回数）に保存する。

## 6. 7 日使用率（run のたびに。launcher の subcommand に無いので CLI を直接。引数は launcher と同じ接続先）
```sh
export CELERIS_MEASURE_TOKEN="$(cat <DAEMON_CONFIG の [api] token_file>)"   # 値は表示・記録しない
$B usage --manifest $MANIFEST --out $OUT --api-url http://<DAEMON_CONFIG の [api] listen> --token-env CELERIS_MEASURE_TOKEN --a-account $A_ACCOUNT > $OUT/usage-N.json
```
exit 0 = A のアカウントを含む claude が 45%・codex が 21% 以内（`--max-7d` / `--max-7d-codex`。承認枠を超える変更は新しい決定）。使用率が観測できない（null）ときは非 0。腕ごとの tokens は `campaign_tokens_by_arm`。

## 7. 停止と再開
- 停止: `sh $L stop --env $ENVF`。signal を送るのは `$STATE_DIR/launcher.pid` の pid 1 つだけで、`launcher.meta` の starttime と cmdline の `--config $DAEMON_CONFIG` が一致しなければ**何も送らず非 0**（pid 再利用・別 process を守る）。止める前に実行像も記録と照合する。SIGTERM → 上限で SIGKILL。本番 daemon・systemd・account には触れない。
- `watch` の停止（Ctrl-C・host 再起動・`max_iterations` 到達）・`waiting>0`・daemon 再起動の後は、`start`（必要なら）→ **同じ `watch` コマンドをそのまま**再実行する。毎回 daemon 実体照合・preflight（env-check）・env-lock の照合を通り（`start` の再接続も同じ実体照合。別 daemon・差し替えられた binary・変わった catalog は外部実行 0 件で拒否）、台帳から同じ試行に再接続する（A の process は `harness-state/<key>/meta.json` の pid・start ticks・上限で同じ物を追う）。二重投入はせず観測済み usage は消えない。同じ out に watch を同時に 2 つ走らせない。別 hash で同じ `OUT` は拒否される。
- 停止中の A の process は上限なしで走り続けうる。再開後の最初の走査（poll）が同じ上限で判定して kill する。実行側の歯止めは `trial_timeout_secs` と `per_case.max_wall_secs`。token 上限は「観測した累積が上限を超えたら止める」で絶対ではない（最大超過は `summary.json` の `max_overshoot_*`）。
- kill 後に process が消えたこと、B/C は task 木と cluster job の終端を確認するまで台帳を閉じない。確認できない間 `watch` は終わらない。

## 8. 検査・独立採点・結果回収
```sh
sh $L grade  --env $ENVF   # grade-out（GRADE_CASES_ROOT の <problem_id>.json で独立採点 → $OUT/grades.jsonl）+ check
sh $L export --env $ENVF   # export を $OUT/export へ。RECORDS があれば final_accounting（総消費量）も含める
```
- `grade` は 6 試行すべてに判定を付ける。失敗・timeout・cancelled・未完了も分母に残す。grade-case が無い problem（root 直下 `<problem_id>.json` か `final/<problem>/<problem>.json`）は `missing_cases` で非 0。`check` は `ok true`・violations 空。`complete=false` なら `incomplete_reasons` を読む。
- `grade-out` 単体の再判定が要るときは `$B grade --case crates/campaign/fixtures/grade-cases/final/<case-id>/<case-id>.json --artifact $OUT/trials/<A|B|C>/<case-id>/rep1/workspace`（`celeris-campaign grade`）。
- final_accounting を単独で見る: `$B final-accounting --manifest $MANIFEST --out $OUT --records $RECORDS`。
- Celeris 側の消費・判断ログ（人。本番 DB は読むだけ、写しに対して）:
```sh
celerisctl db backup $TMPDIR/final-db.sqlite3 --src <本番 DB path>
celerisctl --db $TMPDIR/final-db.sqlite3 campaign export mlsys-final-1 --out $OUT/exp-db --verify-connected --manifest-hash <§3 の campaign sha256>
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
| 実経路確認済み | なし（実 `celeris` release の daemon・実 claude・実 B/C を launcher 経由で動かした確認は未実施。旧 pilot の結果は流用しない） |
| 承認待ち（operator 入力） | 本測定の開始（人の決定）、H1 昇格（release sha12・claude-code 版）、H3 account ID、H4 測定専用 daemon の配置・起動、CAP_ENV の承認値。preflight の出力と不足一覧: `agent-docs/progress/2026-10-19-mlsys-final-config/preflight.md` |
| 未検証 / 未実装 | `/proc/net` による port 所有の照合、試行ごとの A 実体 sha256 取り直し（A は preflight と env-lock の時点で固定。起動直前の再取得は無い）、`usage` の launcher subcommand 化（§6 は CLI 直接）、`--env` 一本化と旧 CLI 欄の削除（launcher は env ファイルから CLI 引数を 1 回だけ生成して使う） |

修正 commit（main 取り込み前の branch 上）: `dd7c5564` daemon identity、`0cd8eab5` env-check、`4d09f56c` launcher と無 LLM e2e、`5adf1ab2` env-lock catalog・pid 照合、`93948ed4` 起動前 config-check・全経路の実体照合・成功系試験。
