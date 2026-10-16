---
title: "MLSys 本測定 実行手順書（runbook）: mock → pilot → 条件固定 → 本測定 → 再開 → 独立採点 → export"
tasks: [01M51QNB8EC4EG0D5GA4BDVWHQ]
unit: release
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# MLSys 本測定 実行手順書

人が実行する手順書。**ここに書いたコマンドは、この repo の build（`$CARGO_TARGET_DIR/debug/`）で `--help` または mock の通し実行により実在を確かめたものだけ**（「確認」節）。
本番 host の操作（昇格・config 変更・secret 登録・本番 DB の写し取り）は **Part H（人だけが行う）** に分けた。worker・agent は Part H を実行しない。

以降の記号: `B=$CARGO_TARGET_DIR/debug/celeris-campaign`、`CT=$CARGO_TARGET_DIR/debug/celerisctl`、`F=crates/campaign/fixtures`、`O=$TMPDIR/campaign`（出力は NFS の worktree に置かない）。

## 0. 状態の区分（実装済み／mock 検証済み／実経路検証済み／本測定未実施）

| 項目 | 実装済み | mock 検証済み | 実経路検証済み | 本測定未実施 |
|---|---|---|---|---|
| A 実行器（`--executor harness`、既存 claude-code ハーネス、後付け制限なし） | ○ `harness_exec.rs` | ○ stub 8 件 | なし | 実 LLM 未実施 |
| B/C 実行器（`--executor celeris`、`POST /api/v1/tasks` の通常経路・待機・再開・子 task 追跡・cancel） | ○ `celeris_exec.rs`、`curl_transport.rs` | ○ 偽 task-api 7 件 | なし（実 HTTP は未使用） | 未実施 |
| 試行単位の routing 固定（B=fixed、C=decision_assisted、`routing_override`） | ○ | ○ `routing_pin` | なし | 未実施 |
| 冪等な試行台帳（(manifest hash, case, arm, repetition)→task/run、再開） | ○ `ledger.rs` | ○ 応答喪失＋再起動で 1 試行 1 task | なし | — |
| 実行中の予算（per_case／per_arm／全体）、反復分離、分母に cancelled を残す | ○ | ○ `budget_cap`、`denominator` | なし | 実消費では未実施 |
| 独立 grader（実装テスト・実行証拠・生データ再計算・既知欠陥。未実行の Markdown は不合格） | ○ `grader.rs` | ○ 16 件 | CLI の判定 JSON と exit code のみ | K1〜K4 実 case の採点は未実施 |
| 代表 case 5 件（dev: k0 control/k1/k3、final: k2/k4） | ○ `cases/`、`fixtures/grader-private/` | ○ ローダ拒否試験 | 外部原票 hash を実ファイルで固定 | 受入 command の pre-fix/fix 実行は未実施 |
| campaign 識別の伝搬と消費計測、`campaign export --verify-connected` | ○ | ○ `campaign_connected` ほか | なし | 本番 DB での接続確認は未実施 |
| API キー未登録 → 受信箱通知 → 登録 → 自動再開 | ○ | ○ `credential_wait_notify`（偽 webhook） | なし | 本番では未観測 |
| Jev（`classifier_status=jev_ok`） | ○ | ○ | 本番 task の routing_decided で jev_ok を確認（CoS 再確認）。ただし `real_api_verified` は定数 false、費用・版の記録は未確認、引き上げ時の basis=`fixed_rule` の帰属は未確認 | Jev smoke（WU jev-live）は未実施 |
| 少量 pilot（2 ケース×A/B 各 1 回） | 手順のみ（Part P） | — | なし | **未実施** |
| 本測定 | 手順のみ（Part M） | mock の通しのみ | なし | **未実施**（`final-measure-budget` の人の承認待ち） |

**未接続（この手順書を書いた時点の欠け。後続 WU か人が埋める）**
1. `grade` CLI は評価 command を実行しない（`NoRunner`、fail-closed）。実 case の実装テストを走らせる runner を CLI に接続する必要がある。それまで `grade` の `implementation` は fail になり得る（成果物が正直でも）。
2. `cases/` の `case.json`（公開部）と `grade --case` が読む `celeris.grade-case/1` は別 schema。K1〜K4 用の grade-case ファイルは未作成（`grade --case cases/dev/k0-…/case.json` は `unknown field budget` で拒否される）。
3. 実 case を `run` に載せる manifest（`problems` が入力 manifest の problem を指す）の生成 CLI は無い。Part P の manifest は手で書き、`validate` で hash を得る。
4. 実 Celeris 実行器の消費は完了後判定が主（`CelerisExecutor::usage` は未実装）。実行中の per_case 打ち切りは A 側 または 同期判定のみ。実 B/C は完了後に超過が分かる（`max_overshoot_*` に出る）。pilot で必ず実測する。
5. `run` の arm 選択（C だけ除外）の option は無い。

## Part V. mock 確認（無課金・何度でも）

```sh
B=$CARGO_TARGET_DIR/debug/celeris-campaign; F=crates/campaign/fixtures; O=$TMPDIR/campaign; mkdir -p $O
$B validate --manifest $F/pilot.json                          # campaign sha256 を出す（exit 0）
$B dry-run  --manifest $F/pilot.json                          # 見積り（llm_calls_made 0、全値 estimated）
$B run --executor mock --manifest $F/pilot.json --out $O/mock # executed 12、tokens_total 113000
$B check --manifest $F/pilot.json --out $O/mock               # 生記録からの再計算と整合（ok true）
$B check-report --manifest $F/pilot.json --out $O/mock --report $F/review/report-good.md
$B export --manifest $F/pilot.json --out $O/mock --dest $O/mock-export   # table.json / table.csv
$B run --executor mock --manifest $F/pilot.json --out $O/mock # 再実行: executed 0, resumed_skipped 12
cargo test -p celeris-campaign                                # mock 統合（mock_e2e 5、budget_cap、grader_independent 16 …）
bash scripts/dev/test-parallel.sh
```
mock の出力は `run-meta.json` / `summary.json` / export に `evidence_kind: mock`、`real_performance: false` が付く。**実性能として引用しない**。

## Part P. 少量 pilot（2 ケース × A/B 各 1 回）

承認済み予算の中の最小実測。目的は性能比較ではなく、**消費係数の実測・配線確認・上限の決定**。C と Jev smoke は WU jev-live、本 pilot は A/B のみ。

1. **前提（人）**: Part H の H1（昇格）と H2（secret 確認・未登録なら登録）が済んでいること。
2. **条件固定**: `$O/pilot.json`（下のサンプル）を作る。`repeats: 1`、problems 2 件。CLI に C だけを除外する option は無い（下の「未接続」に追記: 5）。pilot で C を走らせたくなければ `secondary.subset` を 1 件にして C の 1 試行を許容する（承認範囲内。Jev smoke を兼ねる）。
3. 事前確認:
   ```sh
   $B validate --manifest $O/pilot.json           # hash を控える（以後これが条件の固定）
   $B dry-run  --manifest $O/pilot.json           # within_budget true を確認
   ```
4. 実行（token は環境変数の**名前**だけ渡す。値は argv に出さない）:
   ```sh
   export CELERIS_CAMPAIGN_TOKEN=...   # 人が自分の shell で設定（admin/API token）
   $B run --executor celeris --manifest $O/pilot.json --out $O/pilot-out \
      --api-url http://127.0.0.1:<port>/api/v1 --token-env CELERIS_CAMPAIGN_TOKEN \
      --task-template $O/task-template.json \
      --repo-src <repo の checkout> --materials-src <materials> --kb-src <kb snapshot>
   ```
   stdout の JSON（`executed / waiting / blocked / budget_cancelled / tokens_total / tokens_missing / max_overshoot_*`）を控える。`waiting>0` は完了待ち（再実行で再開、Part R）。
5. 採点と検査:
   ```sh
   $B check  --manifest $O/pilot.json --out $O/pilot-out
   $B grade  --case <grade-case.json> --artifact <trial の成果物 dir>   # 未接続 1・2 参照
   ```
6. **実測の読み取り**（次節の式に入れる）: `summary.json` の試行あたり tokens（A/B 別、`tokens_missing` を除外して数える）、wall、`max_overshoot_*`、外部 task の子 task 数・再試行数、Jev / CoS の共有オーバーヘッド。
7. 結果は進捗ファイル（WU pilot）に書く。**pilot の結果は較正にだけ使い、本測定の主張に混ぜない**（split・campaign_id・hash を分ける）。

## 本測定の件数・反復・上限の提案式（pilot 実測から）

承認済み予算: A/B 各 5 問・C 2 問・試行 12・tokens 1,000,000・7 日使用量 codex 21% / claude 24% を超えない・fable は 2026-10-19 23:00Z まで不使用。

記号（pilot 実測）: `a`,`b`,`c` = A,B,C の 1 試行あたり平均 tokens（欠測は除く。欠測率 `μ`）、`j` = 試行あたり共有オーバーヘッド（Jev・CoS・planner・reviewer の按分）、`ρ` = 内部再試行の平均倍率（子 task・再試行を含む tokens / 初回 tokens）、`n` = 問題数、`m` = C の subset 数、`r` = 独立反復数、`T` = tokens 上限（1,000,000）、`u_x` = 7 日使用量が 1 token あたり増やす割合（pilot で `Δ使用率 / tokens` を codex・claude 別に測る）、`U_x` = 許容の上限（codex 21%、claude 24%）、`U0_x` = 開始時の使用率。

1. 期待消費: `E(n, m, r) = r · ( n·a + n·b·ρ + m·(c·ρ + j) + n·j )`。
2. 安全係数 `s = 1.25`（欠測を保守課金する分: 欠測 tokens は per_case 上限を使い切ったとみなされる、`μ` が大きいほど早く止まる）。
3. 制約: `s·E ≤ T`（A の 12 試行分 = 試行 12 の枠は `r·(2n + m) ≤ 12`、すなわち承認範囲では `r=1, n=5, m=2` が上限。反復を増やすなら `n` を減らすのではなく**新しい決定**）と `U0_x + u_x · s·E ≤ U_x`（x = codex, claude）。
4. 解: 承認範囲内では `r = 1` を固定、`n ≤ 5`、`m ≤ 2` のうち 1・3 を満たす最大の `(n, m)`。満たせないなら `n` を減らす。承認範囲を超える `r > 1` や `n > 5` が統計上必要なら、それは `final-measure-budget` の**新しい決定**として人に出す（本手順書は自動で拡張しない）。
5. 上限の設定: `per_case.max_tokens = ⌈1.5 · max(a, b, c) の pilot 最大値⌉`、`per_case.max_wall_secs = ⌈1.5 · pilot 最大 wall⌉`、`per_arm.max_tokens = ⌈s · r · n · (arm の平均 · ρ)⌉`、`max_tokens = T`、`trial_timeout_secs = per_case.max_wall_secs`。`per_arm ≥ per_case` が validate の条件。
6. 実 B/C は完了後判定（未接続 4）なので、`max_overshoot_tokens`（pilot 実測）を `s` に足して見る: `s ← s + max_overshoot_tokens / (E/試行数)`。
7. fable: 本測定の model 選択に fable を入れない（2026-10-19 23:00Z まで）。manifest の `configs[].model` と task-template の role で確認する。

## Part M. 条件固定 → 本測定 → 再開 → 独立採点 → export

**開始条件**: (a) pilot 完了と式の結果が進捗に記録済み、(b) 件数・反復・上限が承認済み予算の範囲内、または範囲を超える場合は `final-measure-budget` が**人に承認済み**。API キーの登録は予算承認ではない。

1. 条件固定: 本測定用 manifest（`split: final`、新 `campaign_id`、model 実名・版、`routing` 固定、pilot の係数で決めた budget）を `$O/final.json` に書き、`$B validate` の sha256 を進捗に記録。以後 manifest・入力・snapshot は変更しない（変えるなら新 campaign_id）。dev の問題を final に入れない（`validate`/`check` が拒否する）。
2. `$B dry-run --manifest $O/final.json` で `within_budget true`。
3. 本測定: Part P-4 と同じ `run --executor celeris`、`--out $O/final-out`。
4. **再開**: 中断・待機・daemon 再起動の後は**同じコマンドをそのまま再実行**する（台帳が (manifest hash, case, arm, repetition)→task/run を持つので、投入済みは再投入されず、`resumed_skipped` に数えられる）。別 hash で同じ `--out` を再利用すると拒否される。
5. 検査: `$B check --manifest $O/final.json --out $O/final-out`（ok true、violations 空）。報告があれば `$B check-report --manifest … --out … --report <file>`。
6. 独立採点: 試行ごとに `$B grade --case <grade-case.json> --artifact <成果物 dir> [--work <dir>]`（exit 0 = final_pass、非 0 = 判定 JSON を stderr）。未接続 1・2 が解消するまで本測定の採点は完了と言わない。
7. export（campaign 側）: `$B export --manifest $O/final.json --out $O/final-out --dest $O/final-export`。
8. export（Celeris 側の消費・判断ログ。本番 DB の**写し**に対して）: Part H の H5。
9. 採用条件: check ok、grader の判定が全試行に付いている、`--verify-connected` が exit 0。失敗・timeout・cancelled・未完了は分母に残す（除外しない）。

## API キー未登録時の経路（受信箱通知 → 登録 → 自動再開）

1. B/C の work unit が Jev 鍵（secret id `jev_api_key`）を要る時点で未登録なら、その WU だけが `Blocked`（`waiting_for_auth`）になり、**受信箱に `credential_wait` の item が 1 件**出て Discord 通知が 1 件出る（CoS の run は起こさない。ADR 2026-10-15-credential-wait-skip-triage）。同じ鍵を待つ複数 task は 1 通知にまとまる。本文に値は出ない。
2. `campaign run` は `waiting>0` で戻る（待機中の試行は台帳に残る。他の arm・試行は止まらない）。
3. 人が GUI の `/accounts`（または H2 の API）で鍵を登録する。登録は secret の `PUT /api/v1/secrets/{id}` で、event `secret_stored` が追記される。
4. daemon が最小 1 呼び出しで認証を確認し、成功すれば**同じ WU を一度だけ自動で再キュー**（401/403 は待ちを維持して通知を更新、429/5xx は通常 backoff）。cancelled の task は再開しない。
5. 人は `campaign run` を同じ引数で再実行して追跡を再開する（Part M-4）。
登録 ≠ 測定予算の承認。登録で本測定を始めない。

## Part H. 人が本番 host で実行する手順（worker・agent は実行しない）

H1. **昇格**（本番切替）: GUI の「リリース」画面で対象 release の gate 結果を確認して昇格する。昇格前の確認（読み取りのみ）:
```sh
CT=$CARGO_TARGET_DIR/debug/celerisctl    # 本番では ~/.local/celeris/current/bin/celerisctl
$CT release preview --releases-dir ~/.local/celeris/releases <SHA12>   # 昇格の要約（current → 対象。読み取りのみ）
```
`systemctl --user` の再起動、`~/.local/celeris/releases` の差し替えは GUI/`scripts/selfdeploy/promote.sh` に任せ、手で触らない。
H2. **secret の確認と登録**（値は人が入力。ログ・チャットに貼らない）: 確認 `GET /api/v1/secrets`（`fingerprint` の有無だけが見える）。登録は GUI `/accounts`、または `PUT /api/v1/secrets/jev_api_key` に `{"value": "..."}`。
H3. **config**（本番 `~/.config/celeris/config.toml`）: `[routing] mode = decision_assisted` は全体設定のまま。B/C は試行（task）単位の `routing_override` で固定するので**全体 config を切り替えない**。変更が必要になった場合のみ人が編集し、変更前の写しを残す。
H4. **API token**: `--token-env` に渡す環境変数は人が自分の shell で設定する（値を argv・ファイルに残さない）。
H5. **export の接続確認**（本番 DB は読むだけ。写しを作る）:
```sh
$CT db backup <dest>.sqlite3 --src <本番 DB path>               # 本番 DB の写し（`db backup --help` で確認。--src で元を指定）
$CT --db <dest>.sqlite3 campaign export <campaign-id> --out $O/exp-db --verify-connected --manifest-hash <sha256>
echo $?                                                         # 0 なら接続 OK。非 0 は stderr の `not connected:` 行を見る
```
`summary.json` の `shared_overhead`（CoS 等）、`elapsed_ms` と `run_wall_sum_ms`、`total.cash_cost_usd`（取れなければ null）を目で確認し、進捗に転記する。
H6. **cluster job**: cancel された試行の job は自動では消えない。`$O/*/cluster-cancel.jsonl` の `scancel <id>`（slurm）/ `qdel <id>`（pbs）を人が実行する。

## サンプル設定

`$O/pilot.json`（`configs` の model はモデル実名・版に差し替え、`routing.evidence_table_id` は `$CT routing evidence` で最新版を確認して固定。`input.sha256` と `snapshot` は固定値を入れる。pilot は `split: dev`、本測定は `split: final`＋別 `campaign_id`）:
```json
{
  "schema": "celeris.campaign-manifest/1",
  "campaign_id": "mlsys-pilot-1",
  "version": "1",
  "split": "dev",
  "input": {"path": "input-manifest.json", "sha256": "<input manifest の sha256>"},
  "problems": ["<case-1>", "<case-2>"],
  "configs": [
    {"id": "A", "harness": "claude-code", "harness_version": "<ver>", "model": "<model>", "model_version": "<ver>", "policy_version": "ops-baseline"},
    {"id": "B", "harness": "celeris", "harness_version": "<release sha12>", "model": "<model>", "model_version": "<ver>", "policy_version": "fixed",
     "routing": {"lower_policy": "mlsys-min-1", "online_update": false}},
    {"id": "C", "harness": "celeris", "harness_version": "<release sha12>", "model": "<model>", "model_version": "<ver>", "policy_version": "jev-1",
     "routing": {"lower_policy": "mlsys-min-1", "evidence_table_id": "<固定する版>", "online_update": false}}
  ],
  "primary": ["A", "B"],
  "secondary": {"pair": ["B", "C"], "subset": ["<case-1>"]},
  "budget": {
    "max_tokens": 1000000, "max_wall_secs": 36000, "max_disk_bytes": 137438953472, "max_parallel": 1,
    "per_case": {"max_tokens": 100000, "max_wall_secs": 1800},
    "per_arm": {"max_tokens": 400000, "max_wall_secs": 20000}
  },
  "termination": {"trial_timeout_secs": 1800, "max_attempts_per_trial": 1, "stop_on_budget": true},
  "repeats": 1,
  "snapshot": {"repo_commit": "<sha>", "materials_sha256": "<sha256>", "kb_snapshot": "<id>"}
}
```
`$O/task-template.json`（`POST /api/v1/tasks` の雛形。title・objective・labels・workspace・max_wall_secs・routing_override は実行器が埋める。assignee と tier は書かない）:
```json
{"project_id": "<案件 id>", "repos": ["agent-platform"], "acceptance": [{"text": "campaign trial: 独立 grader が判定する", "check": {"type": "reviewer"}}]}
```

## 確認（コマンドの実在）
2026-10-16、この worktree の build で次を実行して確認した。
- 実際に走らせた（exit 0）: `celeris-campaign validate` / `dry-run` / `run --executor mock`（executed 12、再実行で resumed_skipped 12）/ `check` / `export`。
- `--executor celeris` を `--api-url` 無しで起動 → `error: --api-url is required for celeris`（引数の存在を確認。実 API は呼んでいない）。`grade` は `--case`/`--artifact` を要求することを確認。
- `celerisctl release preview` は `--releases-dir` と `<SHA12>` を要し、`db backup` は `--src` を持つことを `--help` で確認。
- `celerisctl --help` に `campaign` / `release` / `routing` / `db` / `api-request` が存在、`celerisctl campaign export --help` に `--out`・`--manifest-hash`・`--verify-connected`・`--db` が存在。
- `GET/PUT /api/v1/secrets` は `crates/task-api/src/handlers.rs` のルートとして存在（実呼び出しはしていない）。
