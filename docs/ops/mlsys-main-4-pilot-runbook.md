---
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
---
# mlsys-main-4 pilot 実行票: K4 A0→B0 各 1 件、通過後に K2 A0/B0 各 1 件

この票は main-4 の再 freeze（revision `final-freeze-2`、`campaign_id = measurement_id = mlsys-main-4-v4`）で pilot を始めるための手順と条件をまとめる。
pilot は問題ごとの manifest 2 件（`mlsys-main-4-v4-pilot-k4`・`mlsys-main-4-v4-pilot-k2`）で流す。
campaign binary は collect 修正入りに再固定し（repin）、その binary そのもので lock・preflight・無評価 LLM の通しを取り直した（nollm-pinned）。
**この票を更新した時点（2026-10-26）で pilot の LLM 試行は 1 件も実行していない**（人の指示。paid_calls_made = 0）。
主測定（12/20 試行）と追加予算はこの票の範囲外で、書かないし実行もしない。

比較の条件（A0/B0 の定義、Jev 無効、token の 4 欄、費用の 4 区分、CoS・共有消費の範囲、欠測の扱い）は
[mlsys-main-4-ab-contract.md](mlsys-main-4-ab-contract.md) を正とする。この票はそれを繰り返さない。手順の一般形は
[measure-run-final.md](measure-run-final.md)、人操作の記録は [eval-human-ops-runbook.md](eval-human-ops-runbook.md) にある。
秘密値（token・credential・API key）はこの票に書かない。token は file から環境変数に読み込み、表示も記録もしない。

以下では次の略記を使う（どれも main4-ready の下にある。読むだけ）:

```sh
W=/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready
R=$W/bundle/final-freeze-2                 # 再 freeze の bundle（revision R2）
P=$R/pinned/main                           # final.json・lock.json・pins.json・launcher.env
RR=$W/rerun                                # refreeze・nollm-rerun の機械向け記録（*.json。旧 binary 時点）
RP=$W/repin                                # repin・nollm-pinned の機械向け記録（repin.json・nollm-pass.json・consistency.json）
K4=$RP/pilot/k4                            # K4 だけの pilot manifest・launcher.env・lock・out
K2=$RP/pilot/k2                            # K2 だけの pilot manifest・launcher.env・lock・out
L=scripts/campaign/measure-launcher.sh     # subcommand: prepare/start/preflight/lock/watch/collect/grade/export/stop
B=$R/pinned/evidence/main-k2/bin/celeris-campaign-fixed   # check・usage・human-op・final-accounting は CLI を直接（repin で pin した collect 修正入り）
```

## 1. 試行の順序と構成

| 順 | 試行 | manifest | arm | 構成 | 通過の判定（次へ進む条件） |
|---|---|---|---|---|---|
| 1 | K4-A0 | `$K4/final.json`（`k4-cmp4-validity-analysis`） | A（A0） | 単体 Claude Code `2.1.303`、`claude-sonnet-5-5` | §3 の停止条件に当たらずに台帳が閉じ、記録と採点が揃う |
| 2 | K4-B0 | 同上 | B（B0） | Celeris release `a28184889f56`、全役割 `claude-sonnet-5-5`、`routing fixed`・`mlsys-min-1`、Jev 無効 | 同上。加えて B の行に Jev の消費が無い |
| — | K4 の評価 | — | — | grade → check → export → human-op report → final-accounting → 件数照合 | 全部 exit 0、checker・export・report の成功/失敗/分母が一致 |
| 3 | K2-A0 | `$K2/final.json`（`k2-bf-xattr-restart-campaign`） | A（A0） | K4-A0 と同じ | K4 が通過した後にだけ始める |
| 4 | K2-B0 | 同上 | B（B0） | K4-B0 と同じ | 同上。cluster job が全部終端している |

- 合否（成功契約の pass/fail）は通過の条件にしない。pilot で見るのは、試行が条件どおり走り、記録・採点・集計が欠けずに揃うことである。
- 予算・打ち切り（R2 の `final.json` と同じ。pilot manifest は problems を 1 件に絞り campaign_id を替えただけ）: 試行ごとに 1,500,000 tokens
  （`per_case`、`token_basis = all`）・1800 秒、腕ごとに 1,500,000 tokens、全体 3,000,000 tokens、`max_parallel = 1`、
  `max_attempts_per_trial = 1`、`stop_on_budget = true`、`approved_trials = 2`（各 manifest の試行数 2 と一致）。
- 順序: K4 と K2 を別 manifest にしたので、`$K4/launcher.env` の `watch` を先に流せば K4 だけが走る（旧票の「K2 が先に走る」制約は解消。C7）。
  R2 の主 manifest（4 試行・`approved_trials = 2`）は pilot に使わない。

### 1.1 固定値（`$RP/repin.json`・`$RP/pilot.json`・`$RR/revision.json`・R2 の現物から写した値）

| 項目 | 値 |
|---|---|
| root / R2 の measurement_id | root `$R`、`mlsys-main-4-v4`（`$RP/repin.json`。repin で新 ID は不要だった＝needs_new_mid false。旧 `mlsys-main-4-v2` 系・`-v3` とは state dir も分けた） |
| R2 manifest `$P/final.json` | sha256 `e5ef430a767499e3affea705bd607cb5d8f32949b2c0f2aba6d9674862cb0a09`、campaign_sha256 `3756ff02244a5d6c8ee4e1c04d5041487d7ebbba9ba762543bc429dfa2e51c65` |
| R2 lock `$P/lock.json` | sha256 `e3bd04c582d288760b1e506a52ae77a2ee0884bc190d10e5f0de573b76231258`、seal `a76cb0766735cd172d1b669871421552d53b10ea8f26c8557bce36da89da3134` |
| K4 pilot manifest | `$K4/final.json`（repin で lock を取り直した。lock_exit 0）、measurement_id `mlsys-main-4-v4-pilot-k4`、sha256 `472594e3ceca7fc79a44d455347a41a755398f4f7a0f0ee67c668d99c2c505de`、campaign_sha256 `e9fb2320303a16d17e38ea6831d1d53b337bb5a869514a2a4bbfa39c0b6ceee0`、lock_sha256 `7b840f04399b0abdcc1ac0b209e775c492c60f73085af0b2987c17cd6b7f071b` |
| K2 pilot manifest | `$K2/final.json`（repin で lock を取り直した。lock_exit 0）、measurement_id `mlsys-main-4-v4-pilot-k2`、sha256 `0bd0db33718536c32f0e2665e6bbf4bb20953793b88002e550718952221654c7`、campaign_sha256 `3447ea282a8c19696aeae9a1793f863782a11a3083c3727cfd652c91cd683a0e`、lock_sha256 `0c236423339bc18dd875e96ea07c0fa791ccf11aa623477e33ad96321bc39938` |
| input manifest | file sha256 `e71e3e43860e8ae5411f451dda61d784d53cc172f6cb9dc1a6d463895ae090f4`（R1 から不変） |
| materials / KB | `810b9562a7b28e736dbe0ce7a7ee9079a6f79df245f2c9f9092a747bc7b14593` / `741f24146f5c02f44acf5291e23806416b9956809e55a1474488ecbcc9fe0702` |
| daemon（RELEASE） | `a28184889f56`、`/local/celeris/state/releases/a28184889f56/bin/celeris` sha256 `8f3ee98dc57ba9219a8b1db8987afb7f584ea14c13308cdd04821b281f67a3aa` |
| campaign binary | `$B`、sha256 `2344d2c47b001e66560a778e964d9e5c2f2b9f5108c552016b727f5105b2b402`（collect 修正入り。`$P/pins.json` の CAMPAIGN_BIN_SHA と一致、check-main-pins exit 0）。旧 binary（修正なし、sha256 `5132230995128dc745593546132dfb947e537d1913718bea482cbc83f5ff93dd`）は `$B.pre-repin` に残し、使わない |
| A（A0） | `claude-code` `2.1.303`、実体 `/home/rmaeda/.local/share/claude/versions/2.1.303`、sha256 `375988f457756524234695cf2d8f64ace99c5f68682e47ea24862a843c191450`、account `claude_max_lab` |
| B（B0） | `celeris`、`harness_version` `a28184889f56`、run harness `claude-code`、Jev 無効 |
| model | `claude-sonnet-5-5`（A・B・cos・planner・worker・reviewer の全部。d-model = sonnet） |
| K4 case / grader | case.json `9ae6231f00c0ad25ebd9a91b5c846157e5ef219eb5295c55deca22eff9ac9995`、grader private `46ea074be15202c4b3d2fca143563ce9c06822607f51e656469da6ad5f6871da`、grade-case `7751a8577dd09d3f8cd5729179bfbc819aaf87249bb46f16e3ecb751ce48ca54` |
| K4 入力（P0-1 採用版） | `main-v3-n2-43706-public2.tgz` sha256 `0671a85528b4a29a8f6fb5176986c178afb2174cb22e56b003c33ed5f2da3a1e`（adoption.json と一致、`$RR/revision.json` の k4_adoption_match） |
| K2 case / grader | case.json `e1d1e5936ffb60a7ce7709653004e02fdaf4e0d01bbf01bea9ee8a4b6b2d64ca`、grader private `d02126077a47495b121e1e678f055ba4e2493f92d05cd010f2bda32d30694760`、grade-case `f84089bcf161edab75bfc619574c112de446b75622f893323f43405190aca593` |
| K2 初期 tree（BenchFS） | commit `086c4618e2a4d94ea56da78ceeed0f8d6d568d67`、tree `56d2f021530c8f55c78e740a4823e53e01975965`。submodule 5 件は bundle（`/local/celeris/data/measure/main-4/k2-submodule-bundles/`）から入れた（`$RR/repo-src.json`） |

repin で新 binary の lock を取り直した結果、R2 の lock.json・env-lock.json と pilot manifest 2 件の lock は旧 binary の物と byte 一致した（`$RP/lock-probe.json`・`$RP/live.json`・`$RP/pilot.json`）。そのため manifest・lock の sha256 は前の値のままである。

## 2. 出力先

| 物 | path |
|---|---|
| launcher 設定 | `$K4/launcher.env`・`$K2/launcher.env`（R2 の `$P/launcher.env` の写しで MANIFEST・MEASUREMENT_ID・OUT を替えた。K4 は REPO_SRC を外す） |
| campaign out（`trials.jsonl`・`ledger.jsonl`・`grades.jsonl`・`human-ops.jsonl`・`export/`） | `$K4/out`・`$K2/out`（各 `launcher.env` の `OUT`。別 hash で同じ `OUT` は拒否される） |
| watch・usage の出力 JSON | `$K4/runs/watch-N.json`・`$K4/runs/usage-N.json`（K2 も同様。`$OUT` の外に置く） |
| 測定 daemon の state / DB | `/local/celeris/data/measure/main-4/state/v4`（port 7741） |
| human-op report | `$K4/human-ops-report/`・`$K2/human-ops-report/` |
| 件数照合の結果 | `$K4/runs/consistency.json`・`$K2/runs/consistency.json`（`$RR/consistency.json` は無評価 LLM の記録なので上書きしない） |

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

| 項目 | 現在値（2026-10-26 に `$RR/ledger.json` と台帳の現物から読んだ値） |
|---|---|
| 共有台帳の path | `/local/celeris/data/measure/main-4/global-ledger`（R2・pilot manifest 2 件の `shared_ledger`、`launcher.env` の `GLOBAL_LEDGER`） |
| 台帳の状態 | 初期化済み（`ledger-init` exit 0、`global-ledger.jsonl` 1 行、承認記録 `approval.json`） |
| 承認の出典・範囲・期限 | 利用者の 2026-10-26 の記録（「試行回数はループせずに前進している場合必要ならどれだけやっても構いません」）。範囲 main-4 の pilot と測定（K4・K2、A0/B0、claude-sonnet-5-5）。期限 2026-11-13 |
| 既承認枠 | 台帳の上限 40 試行（固定上限なしの承認を台帳の有限値で表した。不足時は operator が新しい台帳で引き上げる）。tokens は台帳に上限なし、manifest の予算が効く |
| 消費 | 0 試行・0 tokens（有料試行なし。v2 の stub の消費は別台帳で、本台帳に入れない） |
| 残枠 | 40 試行。実効は manifest との小さい方: pilot manifest は各 2 試行（K4 2・K2 2）で pilot 4 試行に足りる |
| 停止の約束 | 同じ原因で 2 回続けて失敗したら止めて人に上げる。各試行が前進していること。claude の 7 日 usage が尽きそうなら止める |
| 過去消費（参考。別台帳） | main-2: consumed 1,359,694 tokens・2 試行（`/local/celeris/data/measure/main-2/global-ledger/global-ledger.jsonl`）。main-3: K4 A 1,192,762 tokens・K4 B tokens 欠測（`/local/celeris/data/measure/main-3/out/ledger.jsonl`）。main-4 の枠からは引かない（承認記録に含めていない） |

## 5. 人操作の記録方法

[eval-human-ops-runbook.md](eval-human-ops-runbook.md) の手順を A0/B0 で同じ command 列にして使う。違うのは `--arm` と `--case` だけ。

```sh
F=$K4/out/human-ops.jsonl            # K2 は $K2/out/human-ops.jsonl
ID="--file $F --campaign mlsys-main-4-v4-pilot-k4 --manifest $K4/final.json --case k4-cmp4-validity-analysis --arm A0 --rep 1"
# 開始時（live）。op_start が残る
$B human-op record $ID --op-kind status_check --actor-kind human --actor eval1 --mode manual --intent monitoring \
  --trigger scheduled --logical-op pilot/k4/look --started-at now
# 開始時刻を読む
$B human-op list --file $F --arm A0 --case k4-cmp4-validity-analysis
# 終わったら同じ identity・同じ開始時刻で終了を足す（op_end が 1 行足される）
$B human-op record $ID --op-kind status_check --actor-kind human --actor eval1 --mode manual --intent monitoring \
  --trigger scheduled --logical-op pilot/k4/look --started-at <list の start> --ended-at now --outcome ok
```

- 分からない時刻は flag ごと省く（op_start だけ残り、end は missing のまま）。0 や推測の時刻を入れない。
- 人待ちは A だけ `human-op span --kind wait` で書く（B は event から出す）。K2 の外部 job は `--kind` の job 記録を A/B 同じ書式で書く。
- 評価者名は仮名（`eval1`）。メール・本名は検証で拒否される。
- 記録の後、`$B human-op report --file $F --events $K4/out --out $K4/human-ops-report` で表を作り直す。

## 6. 開始可否

**判定: 開始可**（2026-10-26 時点。C1〜C9 すべて済）。運用条件: 承認枠（ループせず前進する限り試行数の固定上限なし、台帳の承認 40）、
同じ原因で 2 回続けて失敗したら停止して人に上げる、期限 2026-11-13。

| # | 条件 | 状態 | 証跡（2026-10-26） |
|---|---|---|---|
| C1 | RELEASE の昇格（d-release = post-merge）と pins・launcher.env・B の harness_version・lock の作り直し | 済 | release `a28184889f56`（health・current・promoted.json、`$RR/revision.json`）。新 binary の pin 後に `check-main-pins.py --root $R --pins $P/pins.json` → exit 0（`$RP/repin.json` の check_main_pins_exit、`$RP/notes/record-check-main-pins.out`） |
| C2 | A の account login | 済 | `claude_max_lab` logged_in（task comment 2026-10-26 06:26Z）。実 preflight の auth-env・capability が通過 |
| C3 | claude OAuth の更新と env-lock | 済 | model catalog 14 件（`$RR/env-lock.json`）。新 binary で `lock` exit 0、`env-lock --verify` exit 0（`$RP/live.json`、`$RP/repin.json` の env_lock_verify_exit） |
| C4 | sirius の ControlMaster と cap-probe | 済 | `ssh -O check -S /run/celeris-cap/sirius-ssh.sock sirius` exit 0（`$RR/cap-probe.json`）。新 binary で cap-probe A/B × K2/K4 exit 0・paid_calls 0（`$RP/live.json`） |
| C5 | 承認の記録と共有台帳の初期化 | 済 | `ledger-init` exit 0、承認 40・消費 0・残 40（`$RR/ledger.json`）。pilot manifest は各 2 試行で `approved_trials = 2` と一致 |
| C6 | 実測定構成の preflight | 済 | 新 binary・R2 で prepare・start・preflight・stop exit 0、run `01M5WHQT0TVY435VY470C2KNYB`、ok=true・missing なし・checked 17・paid_calls 0、config=real、release `a28184889f56`（`$RP/live.json`・`$RP/repin.json`）。project `01M5W9014BJF29SS6VQDFX0Z18`。旧 binary での preflight run `01M5WANNTJSBHTQA364VMBC5YB`（`$RR/preflight.json`）は参考 |
| C7 | K4 先行の順序を表せる manifest | 済 | `$K4/final.json`・`$K2/final.json` を同じ shared_ledger で作り、新 binary で validate・start・lock・stop exit 0（`$RP/pilot.json`） |
| C8 | 新 revision の out で件数照合 | 済 | pin した binary そのもの（sha256 `2344d2c47b001e66560a778e964d9e5c2f2b9f5108c552016b727f5105b2b402`）で無評価 LLM の 12 段 exit 0、paid_calls 0（`$RP/nollm-pass.json`）。`check-out-consistency.py` exit 0・consistent=true・mismatches []、final-accounting も一致（`$RP/consistency.json`、campaign_bin_matches_pin true）。旧 binary 時代の通し（`$RR/nollm-pass.json`、未 pin の sha256 `2dd77ed2a28772999878a65bd122450dc9eb1fb3eea6dc0d4c49cb7d39b18e6d`）は参考 |
| C9 | campaign binary を collect 修正入りに再固定 | 済 | 件名 `collect-stub: collect-observed-out は not_submitted だけなら記録を書いて exit 0` の commit を含む task HEAD から `cargo build --release -p celeris-campaign` → `$B` に置き、`$P/pins.json` の CAMPAIGN_BIN_SHA を `2344d2c47b001e66560a778e964d9e5c2f2b9f5108c552016b727f5105b2b402` に更新、check-main-pins exit 0。lock・env-lock --verify・preflight・pilot lock 2 件を取り直し全部 exit 0、新 measurement_id 不要（`$RP/repin.json`）。同じ binary の無評価 LLM 通しで collect exit 0（not_submitted 2・blocking 0、`$RP/nollm-pass.json`） |

注意: pin した binary は task branch の HEAD から build した物で、collect 修正はまだ main と release gate を通っていない。
main 取り込み後に別の build の binary に差し替えるときは、repin と同じ手順（build-pin → lock-probe → live → pilot）と無評価 LLM の通しをやり直す。差し替えない限り、この票の値で開始してよい。

開始後の運用条件: §3 の停止条件、§4 の台帳の約束（同じ原因で 2 回続けて失敗したら停止、期限 2026-11-13）、§5 の人操作の記録。
pilot manifest には pilot 用の pins が無いので `check-main-pins.py` は通していない。lock の照合は `watch` の preflight が毎回行う。

### 6.1 開始後の 1 試行ずつの流れ（条件が揃った後）

```sh
ENVF=$K4/launcher.env               # K2 は $K2/launcher.env
RUNS=$(dirname "$OUT")/runs; mkdir -p "$RUNS"
export CELERIS_MEASURE_TOKEN="$(cat <DAEMON_CONFIG の [api] token_file>)"   # 値は表示・記録しない
$B usage --manifest $MANIFEST --out $OUT --api-url http://<DAEMON_CONFIG の [api] listen> \
  --token-env CELERIS_MEASURE_TOKEN --a-account claude_max_lab > $RUNS/usage-1.json      # exit 0 を確かめてから
sh $L watch --env $ENVF > $RUNS/watch-1.json      # max_parallel=1 なので A → B の順に 1 件ずつ
sh $L grade --env $ENVF                           # collect → grade-out → check
sh $L export --env $ENVF
$B human-op report --file $OUT/human-ops.jsonl --events $OUT --out $(dirname "$OUT")/human-ops-report
$B final-accounting --manifest $MANIFEST --out $OUT --records <RECORDS>
python3 scripts/campaign/check-out-consistency.py --out $OUT --campaign-bin $B --check-arg=…
```

各段の exit code・`watch.end`・`tokens_missing`・`max_overshoot_*` を進捗ファイルに写す。K4 の全段が exit 0 で §3 に当たらなかったら、
K2 の launcher.env で同じ流れを繰り返す。
