---
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
---
# mlsys-main-4 pilot 実行票: K4 A0→B0 各 1 件、通過後に K2 A0/B0 各 1 件

この票は main-4 の最終 freeze（`campaign_id = measurement_id = mlsys-main-4-v3`）で pilot を始めるための手順と条件をまとめる。
**この票を作った時点で pilot の LLM 試行は 1 件も実行していない**（人の指示。`freeze-report.json` の `paid_calls_made = 0`）。
主測定（12/20 試行）と追加予算はこの票の範囲外で、書かないし実行もしない。

比較の条件（A0/B0 の定義、Jev 無効、token の 4 欄、費用の 4 区分、CoS・共有消費の範囲、欠測の扱い）は
[mlsys-main-4-ab-contract.md](mlsys-main-4-ab-contract.md) を正とする。この票はそれを繰り返さない。手順の一般形は
[measure-run-final.md](measure-run-final.md)、人操作の記録は [eval-human-ops-runbook.md](eval-human-ops-runbook.md) にある。
秘密値（token・credential・API key）はこの票に書かない。token は file から環境変数に読み込み、表示も記録もしない。

以下では次の略記を使う（どれも final-freeze の bundle の中にある。読むだけ）:

```sh
W=/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready
R=$W/bundle/final-freeze-1                 # freeze の bundle（revision）
P=$R/pinned/main                           # final.json・lock.json・pins.json・launcher.env
L=scripts/campaign/measure-launcher.sh     # subcommand: prepare/start/preflight/lock/watch/collect/grade/export/stop
B=$R/pinned/evidence/main-k2/bin/celeris-campaign-fixed   # check・usage・human-op・final-accounting は CLI を直接
```

## 1. 試行の順序と構成

| 順 | 試行 | case | arm | 構成 | 通過の判定（次へ進む条件） |
|---|---|---|---|---|---|
| 1 | K4-A0 | `k4-cmp4-validity-analysis` | A（A0） | 単体 Claude Code `2.1.303`、`claude-sonnet-5-5` | §3 の停止条件に当たらずに台帳が閉じ、記録と採点が揃う |
| 2 | K4-B0 | `k4-cmp4-validity-analysis` | B（B0） | Celeris（RELEASE は未定、§2）、全役割 `claude-sonnet-5-5`、`routing fixed`・`mlsys-min-1`、Jev 無効 | 同上。加えて B の行に Jev の消費が無い |
| — | K4 の評価 | — | — | grade → check → export → human-op report → final-accounting → 件数照合 | 全部 exit 0、checker・export・report の成功/失敗/分母が一致 |
| 3 | K2-A0 | `k2-bf-xattr-restart-campaign` | A（A0） | K4-A0 と同じ | K4 が通過した後にだけ始める |
| 4 | K2-B0 | `k2-bf-xattr-restart-campaign` | B（B0） | K4-B0 と同じ | 同上。cluster job が全部終端している |

- 合否（成功契約の pass/fail）は通過の条件にしない。pilot で見るのは、試行が条件どおり走り、記録・採点・集計が欠けずに揃うことである。
- 予算・打ち切り（`final.json`）: 試行ごとに 1,500,000 tokens（`per_case`、`token_basis = all`）・1800 秒（`trial_timeout_secs`・`per_case.max_wall_secs`）、
  腕ごとに 1,500,000 tokens、全体 3,000,000 tokens、`max_parallel = 1`、`max_attempts_per_trial = 1`、`stop_on_budget = true`。
- **順序の制約（未解決）**: 現 freeze の `final.json` は 1 つの manifest に K2・K4 × A・B の 4 試行を持つ。`problems` は正規化で
  整列される（`crates/campaign/src/manifest.rs` の正規化）ので、計画順では `k2-…` が `k4-…` より先に来る。さらに `approved_trials = 2`
  なので、この manifest のまま `watch` すると **K2 A/B の 2 件が先に走り、K4 に届かない**。K4 を先に走らせるには、K4 だけの
  pilot manifest（K2 用にもう 1 つ）を同じ `shared_ledger` で作り、validate・lock・`check-main-pins.py` を通し直す必要がある。
  これは freeze の変更なので、この票では作っていない（§6 の条件 C7）。

### 1.1 固定値（`freeze-report.json` から写した値）

| 項目 | 値 |
|---|---|
| campaign_id / measurement_id | `mlsys-main-4-v3`（旧 `mlsys-main-4-v2` 系の pilot1・nollm1 とは混ぜない） |
| manifest `$P/final.json` | sha256 `2cec0de37c16659a2a5589766c7ecb872debabf330c14c99fb05131547521d68`、campaign_sha256 `be513f522ec2e6441f31b843cfda25ae626095d40ffc2f9f23df5bae98c5f3a2` |
| input manifest | file sha256 `e71e3e43860e8ae5411f451dda61d784d53cc172f6cb9dc1a6d463895ae090f4`、宣言 sha256 `62871a59d5cef7171410dc8b0299ba818a42d35f8d5f54ff199dafb2b9fc0b98` |
| lock `$P/lock.json` | sha256 `e5079f55712664614ac45e6fcc244d49c8b470bf5ca73c123bef1df981317b69`、seal `545761e94988915358168038a2cf3ab26d48a69a8850c682bee412e338a9f9bf` |
| materials / KB | `810b9562a7b28e736dbe0ce7a7ee9079a6f79df245f2c9f9092a747bc7b14593` / `741f24146f5c02f44acf5291e23806416b9956809e55a1474488ecbcc9fe0702` |
| 研究 source | `663d5283c2fbb8ce100299bd55241ce2c8f56466`（main `d817adbbd092…` を祖先に持つ。post-merge で main に入った commit へ読み替える） |
| campaign binary | `$B`、sha256 `5132230995128dc745593546132dfb947e537d1913718bea482cbc83f5ff93dd` |
| 候補 daemon `$R/bin/celeris` | sha256 `99af23117cff6ec4fd5851ad13629c867dee66faac96604c50d5dad8ab471640`（RELEASE 昇格後に release の `bin/celeris` へ差し替え） |
| A（A0） | `claude-code` `2.1.303`、実体 `/home/rmaeda/.local/share/claude/versions/2.1.303`、sha256 `375988f457756524234695cf2d8f64ace99c5f68682e47ea24862a843c191450`、account `claude_max_lab` |
| B（B0） | `celeris`、`harness_version` は仮値 `641f334b6d64`（RELEASE 未昇格）、run harness `claude-code`、Jev 無効 |
| model | `claude-sonnet-5-5`（A・B・cos・planner・worker・reviewer の全部。d-model = sonnet） |
| K4 case / grader | case.json `9ae6231f00c0ad25ebd9a91b5c846157e5ef219eb5295c55deca22eff9ac9995`、grader private `46ea074be15202c4b3d2fca143563ce9c06822607f51e656469da6ad5f6871da`、grade-case `7751a8577dd09d3f8cd5729179bfbc819aaf87249bb46f16e3ecb751ce48ca54` |
| K4 入力（P0-1 採用版） | `main-v3-n2-43706-public2.tgz` sha256 `0671a85528b4a29a8f6fb5176986c178afb2174cb22e56b003c33ed5f2da3a1e` |
| K2 case / grader | case.json `e1d1e5936ffb60a7ce7709653004e02fdaf4e0d01bbf01bea9ee8a4b6b2d64ca`、grader private `d02126077a47495b121e1e678f055ba4e2493f92d05cd010f2bda32d30694760`、grade-case `f84089bcf161edab75bfc619574c112de446b75622f893323f43405190aca593` |
| K2 初期 tree（BenchFS） | commit `086c4618e2a4d94ea56da78ceeed0f8d6d568d67`、tree `56d2f021530c8f55c78e740a4823e53e01975965`、content sha256 `2862119f1c33fc0d4481bec0fd83eda8fbb5a4c7014f5868834b53275ab7ab30`（submodule 5 件の content hash も `freeze-report.json` にある） |

K4 だけ・K2 だけの pilot manifest を作ったら（C7）、manifest・campaign_sha256・lock の値はそちらに替わる。替わった値は
その時の freeze report に記録し、この表の値を流用しない。

## 2. 出力先

| 物 | path |
|---|---|
| launcher 設定 | `$P/launcher.env`（RELEASE 行はまだ無い。§6 C1） |
| campaign out（`trials.jsonl`・`ledger.jsonl`・`grades.jsonl`・`human-ops.jsonl`・`export/`） | `$R/out`（`launcher.env` の `OUT`） |
| watch・usage の出力 JSON | `$R/runs/watch-N.json`・`$R/runs/usage-N.json`（`$OUT` の外。`$OUT` に余計な file を置くと拒否される） |
| 測定 daemon の state / DB | `/local/celeris/data/measure/main-4/state/v3` |
| human-op report | `$R/human-ops-report/` |
| 件数照合の結果 | `$W/consistency.json` を上書きせず `$R/runs/consistency-<段>.json` に置く |

K4 用・K2 用に manifest を分けたら、`OUT` もそれぞれ別の dir にする（別 hash で同じ `OUT` は拒否される）。

## 3. 停止条件

次のどれかに当たったら、その試行の後に進まない（同じ試行の再実行もしない）。`sh $L stop --env $P/launcher.env` で測定 daemon を止め、
事実を進捗ファイルに書いて人に返す。

1. `preflight`（`watch` が毎回通す preflight・env-check・env-lock 照合を含む）が非 0。非 0 の preflight を ready として扱わない。
2. `usage` が非 0（claude の 7 日使用率 45% 超、codex 21% 超、または使用率が観測できない null）。
3. 台帳の上限（§4）か予算（`per_case`・`per_arm`・全体）に達した。`stop_on_budget` で止まった試行は打ち切りとして分母に残す。
4. 試行の timeout（1800 秒）か token 上限で kill された。process・task 木・cluster job の終端を確認するまで台帳を閉じない。
5. B の行に Jev の消費が 1 件でも出た（Jev 無効の条件違反。主比較から外し、未整合として残す）。
6. grader-private の内容が試行の workspace に出た、または秘密値が out・log に出た。
7. `grade`・`check`・`export`・`final-accounting`・件数照合のどれかが非 0、または tokens・費用が欠測になった
   （欠測は欠測のまま残し、0 に読み替えない。main-3 では B の `observed_usage.tokens` が null だった）。
8. K2 で、cluster job が試行後も終端していない（`cluster-cancel.jsonl` の手順で人が消す。自動では消さない）。
9. 人の操作が記録されないまま入った（§5 の記録を後で足せないもの）。

## 4. 共有台帳・既承認枠・残枠

| 項目 | 現在値（2026-10-25 に読んだ値） |
|---|---|
| 共有台帳の path | `/local/celeris/data/measure/main-4/global-ledger`（`final.json` の `experiment_sets.main.fixed.shared_ledger`、`launcher.env` の `GLOBAL_LEDGER`） |
| 台帳の状態 | **未初期化**（dir が無い。`/local/celeris/data/measure/main-4/` には `state/` だけ）。消費の行が 0 件なのは記録が無いためで、「消費 0 を観測した」ではない |
| main-4 の既承認枠 | **未取得**。承認の出典・範囲・期限の記録が見つからない。`final.json` の `approved_trials = 2` は manifest の宣言で、承認の証拠ではない。4 試行には足りない |
| main-4 の残枠 | **未取得**（既承認枠が未取得のため。推測しない） |
| main-4-v2 の試行 | pilot1・nollm1 は無 LLM（stub）で、有料試行は 0（env-nollm の記録） |
| 過去消費（参考。別台帳） | main-2: `/local/celeris/data/measure/main-2/global-ledger/global-ledger.jsonl` の最終 measurement 行が consumed 1,359,694 tokens・2 試行・221 秒、caps 3,000,000 tokens・12 試行。main-3: 共有台帳なし。`/local/celeris/data/measure/main-3/out/ledger.jsonl` の K4 A は 1,192,762 tokens（input 40・output 15,583・cache_read 1,093,204・cache_creation 83,935）・135 秒、K4 B は tokens 欠測（null）・201 秒 |

過去消費を main-4 の残枠から引くかどうかは、承認の範囲の決め方による。承認記録が無いので、この票では引き算をしない。
人が承認を記録するとき（§6 C5）に、main-2/main-3 の消費を同じ枠に含めるかを書く。

## 5. 人操作の記録方法

[eval-human-ops-runbook.md](eval-human-ops-runbook.md) の手順を A0/B0 で同じ command 列にして使う。違うのは `--arm` と `--case` だけ。

```sh
F=$R/out/human-ops.jsonl
ID="--file $F --campaign mlsys-main-4-v3 --manifest $P/final.json --case k4-cmp4-validity-analysis --arm A0 --rep 1"
# 開始時（live）。op_start が残る
$B human-op record $ID --op-kind status_check --actor eval1 --mode manual --intent monitoring \
  --trigger scheduled --logical-op pilot/k4/look --started-at now
# 開始時刻を読む
$B human-op list --file $F --arm A0 --case k4-cmp4-validity-analysis
# 終わったら同じ identity・同じ開始時刻で終了を足す（op_end が 1 行足される）
$B human-op record $ID --op-kind status_check --actor eval1 --mode manual --intent monitoring \
  --trigger scheduled --logical-op pilot/k4/look --started-at <list の start> --ended-at now --outcome ok
```

- 分からない時刻は flag ごと省く（op_start だけ残り、end は missing のまま）。0 や推測の時刻を入れない。
- 人待ちは A だけ `human-op span --kind wait` で書く（B は event から出す）。K2 の外部 job は `--kind` の job 記録を A/B 同じ書式で書く。
- 評価者名は仮名（`eval1`）。メール・本名は検証で拒否される。
- 記録の後、`$B human-op report --file $F --events $R/out --out $R/human-ops-report` で表を作り直す。

## 6. 開始可否

**判定: 開始不可**（2026-10-25 時点）。`freeze-report.json` の `measurement_ready = false`、`check-main-pins.py` は exit 1（`RELEASE: pending`）。
次の条件が全部満たされたら開始できる。C1〜C5 は人の操作、C6〜C8 は人の操作の後に worker が実行して証跡を残す。

| # | 条件 | 実行する人 | 手順（コマンド） | 確認方法 |
|---|---|---|---|---|
| C1 | RELEASE の昇格（d-release = post-merge） | 人 | 本 task を main に統合 → `scripts/selfdeploy/release.sh`（[selfdeploy.md](selfdeploy.md)）→ 昇格し sha12 を控える。`pins.json` に `RELEASE=<sha12>`、`launcher.env` に `RELEASE=<sha12>` と `CELERIS_BIN=/local/celeris/state/releases/<sha12>/bin/celeris`、`final.json` の B `harness_version` を `<sha12>` に替え、`$B validate --manifest $P/final.json` → `sh $L lock --env $P/launcher.env`（`MEASUREMENT_ID=mlsys-main-4-v3`）で campaign_sha256・lock を作り直し、`pins.json` の `CAMPAIGN_SHA` を更新 | `python3 scripts/campaign/check-main-pins.py --root $R --pins $P/pins.json` に `RELEASE` の NG が出ない |
| C2 | A の account login | 人 | 通常の login 経路で `claude_max_lab` に login（credential を写さない） | 測定 daemon の `GET /api/v1/llm/accounts` で logged-in |
| C3 | claude OAuth の更新と env-lock | 人 → worker | token を通常経路で更新 → `sh $L lock --env $P/launcher.env` | `$P/env-lock.json` ができ、`env-lock --verify` が exit 0 |
| C4 | sirius の ControlMaster（K2 と cap-probe に要る） | 人 | TOTP で sirius の ControlMaster を張り直す（`/run/celeris-cap/sirius-ssh.sock`） | `ssh -O check -S /run/celeris-cap/sirius-ssh.sock sirius` が exit 0。その後 cap-probe を取り直し `pinned/evidence/cap-a-k2.json`・`cap-a-k4.json`・`cap-b.json` を置く |
| C5 | 承認の記録と共有台帳の初期化 | 人 | 承認の出典・範囲（試行数・tokens・過去消費を含めるか）・期限を記録 → 承認数を manifest の `approved_trials` に入れる（変えたら lock を作り直す）→ `sh $L prepare --env $P/launcher.env` で台帳を初期化 | `/local/celeris/data/measure/main-4/global-ledger/global-ledger.jsonl` ができ、caps が承認値と一致 |
| C6 | 実測定構成の preflight | worker | `sh $L prepare` → `sh $L start` → `sh $L preflight --env $P/launcher.env`。`task-template.json` の `project_id` は新 DB で `POST /api/v1/projects` から作り直す（旧 DB の id は使わない） | preflight が exit 0・`ok true`・`missing` 空・`paid_calls_made 0`。`pinned/main/evidence/k2-preflight.json` と `pinned/evidence/main-k2/run/argv.txt` が残る |
| C7 | K4 先行の順序を表せる manifest | worker（freeze の変更） | K4 だけ・K2 だけの pilot manifest を同じ `shared_ledger` で作り、validate・lock・`check-main-pins.py` を通す（§1 の順序の制約） | 計画の期待試行数が各 2（A・B）で、K4 の manifest が先に `watch` される |
| C8 | 新 revision の out で件数照合 | worker | 無 LLM の通しを新 revision で取り、`python3 scripts/campaign/check-out-consistency.py --out <out> --campaign-bin $B --check-arg=…`（launcher と同じ `--measurement-id`・資産の引数） | exit 0（consistent） |

K2 にはさらに、BenchFS の submodule 5 件の commit object を含む clone か bundle を読める場所に置き、`REPO_SRC` を作り直して
preflight で照合することが要る（`freeze-report.json` の `repo-src-submodule-commits`。content hash は一致、commit object は手元に無い）。
K4 の pilot はこれを待たない。

### 6.1 開始後の 1 試行ずつの流れ（条件が揃った後）

```sh
ENVF=<K4 pilot の launcher.env>      # C7 で作る
RUNS=$(dirname "$OUT")/runs; mkdir -p "$RUNS"
export CELERIS_MEASURE_TOKEN="$(cat <DAEMON_CONFIG の [api] token_file>)"   # 値は表示・記録しない
$B usage --manifest $MANIFEST --out $OUT --api-url http://<DAEMON_CONFIG の [api] listen> \
  --token-env CELERIS_MEASURE_TOKEN --a-account claude_max_lab > $RUNS/usage-1.json      # exit 0 を確かめてから
sh $L watch --env $ENVF > $RUNS/watch-1.json      # max_parallel=1 なので A → B の順に 1 件ずつ
sh $L grade --env $ENVF                           # collect → grade-out → check
sh $L export --env $ENVF
$B human-op report --file $OUT/human-ops.jsonl --events $OUT --out $R/human-ops-report
$B final-accounting --manifest $MANIFEST --out $OUT --records <RECORDS>
python3 scripts/campaign/check-out-consistency.py --out $OUT --campaign-bin $B --check-arg=…
```

各段の exit code・`watch.end`・`tokens_missing`・`max_overshoot_*` を進捗ファイルに写す。K4 の全段が exit 0 で §3 に当たらなかったら、
K2 の launcher.env で同じ流れを繰り返す。
