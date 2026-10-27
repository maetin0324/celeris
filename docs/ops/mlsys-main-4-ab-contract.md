---
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
---
# mlsys-main-4 主比較の契約: A0（単体 Claude Code）と B0（Celeris・同一固定モデル・Jev 無効）

この文書は main-4（`campaign_id = mlsys-main-4-v2`、revision `judged-scope-v2`）の主比較で、何を同じにし、何を違いとして測り、token と費用をどう数えるかを測定前に固定する。
照合した現物は次のとおり（読むだけ。書き換えていない）:

- campaign 定義: K4 成果物 root `/local/celeris/data/workspaces/01M5KVHA4222TK6VEKW599SNMA/artifacts/mlsys-main-4/revisions/judged-scope-v2/pinned/main/` の `final.json`・`pins.json`・`task-template.json`・`final-input-manifest.json`・`lock.json`、同 root の `freeze-status.json`
- 起動経路: `scripts/campaign/measure-launcher.sh`、手順 [measure-run-final.md](measure-run-final.md)、セット [experiment-sets.md](experiment-sets.md)
- 実行器: `crates/campaign/src/harness_exec.rs`（A）、`crates/campaign/src/celeris_exec.rs`（B）、`crates/task-worker/src/claude_code.rs`（B の run）
- 成功契約と集計: `agent-docs/progress/2026-10-23-success-contract-human-ops.md`、[eval-raw-record-definitions.md](eval-raw-record-definitions.md) §7、`crates/campaign/src/{success,export,final_accounting}.rs`

## 評価範囲（2026-10-27 明記）

A0/B0 が測るのは **task 実行系**であり、通常運用の Celeris 全体ではない。A0 は単体 harness、B0 は
`crates/campaign/src/celeris_exec.rs` の `submit` が task API を直接呼ぶ構成で、通常の案件 CoS・統括 CoS による
依頼の理解・起票・受信箱対応・報告や、外部 operator による運用補助を試行の経路に組み込まない。
§7 の `cos_not_called_basis` はこの経路と消費行の確認に基づく。通常運用でも CoS の費用が 0 という意味ではない。
したがって、この比較だけから通常運用全体の成功率、人の負担、自律性、総費用の優劣は主張しない。

この追記は評価範囲の説明であり、既存の pinned bundle・pilot の条件、ID、予算、成功契約、§7 の集計範囲を変えない。
後続の freeze/pilot の現物は [pilot 実行票](mlsys-main-4-pilot-runbook.md)と
[readiness 記録](../../agent-docs/progress/2026-10-25-main4-pilot-readiness.md)を参照する。
実際に外部 operator が回答・修復・再投入等で結果へ影響した場合は、その支援を別記し、
「共有の開発作業」と一律に除外して支援なしの結果に見せない。既存の試行記録・成功判定は書き換えない。

CoS を含む通常運用と、境界外の「外部 AI による支援」を報告する新 revision は
[main-4-ops-r1](mlsys-main-4-ops-r1.md)で別に定義する。既存 A0/B0 の結果とは混合しない。

## 1. 後で入る欄（人の決定）

| 欄 | 決定 | 値 | 記入する段 |
|---|---|---|---|
| 固定モデル名 `MODEL` | d-model（回答: sonnet） | `<final-freeze で記入>`（現 `final.json` の A・B・全役割は `claude-sonnet-5-5`。最終値はこの欄に写し、A・B・role_models・`routing.models` の全部で同じにする） | final-freeze |
| Celeris 版 `RELEASE`（B の `harness_version`） | d-release（回答: post-merge） | `<final-freeze で記入>`（現 `final.json` の B `harness_version` は旧版の仮値。本 task の変更を main に取り込んだ後の昇格済み release の sha12 を入れ、validate/lock を作り直す） | final-freeze |
| A の Claude Code 版 `A_VERSION` と実体 `A_PROGRAM`・`A_PROGRAM_SHA` | 運用者（pin-audit で現物照合） | `<final-freeze で記入>`（現 `final.json` A `harness_version` は `2.1.303`） | final-freeze |
| 測定 ID `MEASUREMENT_ID` | final-freeze | `<final-freeze で記入>` | final-freeze |

モデル・版は A・B で 1 つだけにする。どちらかの腕だけ別のモデル・別の Claude Code 版になった試行は主比較に入れない。

## 2. A0 と B0 の定義

- **A0（単体 Claude Code）**: `final.json` の config `A`。`claude -p <prompt> --model <MODEL> --output-format stream-json --verbose --permission-mode bypassPermissions` を 1 process で走らせる（`harness_exec.rs` の `ClaudeCodeBuilder`）。Celeris の routing・turn 制限・tool 制限・reviewer は通さない。外から足すのは bubblewrap による FS 隔離、実行中の token 上限、試行 timeout の 3 つだけ（ADR 2026-10-17-mlsys-pilot2-fixes）。
- **B0（Celeris・同一固定モデル・Jev 無効）**: config `B`。campaign の実行器が測定専用 daemon に `POST /tasks` で task を 1 件作り、Celeris がその task の木（planner・worker・reviewer の run）を回す。run の harness は `claude-code` だけ、モデルは `routing.models = [<MODEL>]` だけ、`policy_version = fixed`・`lower_policy = mlsys-min-1`・`online_update = false`・根拠表なし。
- 実験セット `main`（`arms = [A, B]`、`compare = [[A, B]]`）だけを選ぶ（`selected_sets = ["main"]`）。human-op 記録と報告では A を `A0`、B を `B0` と呼ぶ（Jev 副比較の B1/C1 と区別する）。

### 2.1 Jev 無効

- B0 の routing は `fixed` で、Jev の分類器を呼ばない。`jev_api_key` を確かめず、要求もしない（[measure-run-final.md](measure-run-final.md) H2・§3a）。C（`jev-1`）は主比較に入れない。
- 集計上の扱い: final-accounting の cell ごとの Jev 状態は、A が `not_called`（根拠 `arm_without_celeris_routing`）、B が `not_called`（根拠 `fixed_routing`）。B に Jev の消費行が 1 件でも現れたら、その試行は Jev 無効の条件を破っているので主比較から外し、未整合として記録する（0 に読み替えない）。

## 3. 条件の差の表

「同じ」は契約で揃えるもの、「違う」は比較したい差（Celeris の仕組み）か、既知の差として報告に書くもの。

| 項目 | A0 | B0 | 扱い |
|---|---|---|---|
| 課題（初期入力） | input manifest の `prompt`（`materials/<case>/task.md` と `case.json` の参照）をそのまま `-p` に渡す | 同じ `prompt` を task の `objective` にし、末尾に `campaign-idempotency-key` と `cluster-job-walltime-secs` の 2 行を足す | 同じ課題。B の 2 行は実行器の識別と walltime 通知で、課題の情報ではない |
| 公開資産 | trial workspace に `MATERIALS_SRC`・`KB_SRC`・（K2 だけ）`REPO_SRC`・`INPUTS_SRC` の写し。hash は `snapshot.materials_sha256`・`kb_snapshot`・lock で固定 | 同じ trial workspace を task の作業場所にする（`repos: []`・`workspace_mode: shared`） | 同じ。grader-private はどちらにも見せない |
| Celeris の前置き | 無い | run の prompt に Celeris の profile・知識・記憶などの前置きが載る | 違う（B の仕組みの一部）。中身の固定は未整合 U5 |
| モデル | `<MODEL>` | 全役割（planner・worker・reviewer・CoS）が `<MODEL>` | 同じ |
| harness 版 | `A_VERSION` の Claude Code | run は同じ Claude Code（`[adapters.claude_code] command`）、外側は `RELEASE` の Celeris | Claude Code の版を揃える。違いは未整合 U4 |
| 実行環境・toolchain | bubblewrap 内。PATH・toolchain は `CAP_ENV` | worker sandbox。同じ `CAP_ENV`（`ro_mounts`・`PATH`・`PKG_CONFIG_PATH`） | 同じ。preflight の cap-probe と env-lock で照合する |
| 認証 | `A_ACCOUNT` の `CLAUDE_CONFIG_DIR`（試行ごとの state dir、`.credentials.json` だけ read-only） | 測定 daemon の `[accounts] claude_dir` の pool | アカウントの種類（subscription か API か）を揃える。quota の扱いは §6.4 |
| tool | Claude Code の既定の tool 全部（`bypassPermissions`）。Agent/Task も使える | `--disallowedTools Agent,Task,Workflow`（subagent 禁止）、background task 無効 | 違う。B は subagent を使えない。A の subagent 消費の数え方は未整合 U3 |
| turn 上限 | 付けない | 各 run に `--max-turns 100`（`task-template.json` の `max_turns`） | 違う（既知の差として報告に書く） |
| 役割 | 1 process が全部やる | planner → worker → reviewer。完了は reviewer が決める | 違う（比較したい差） |
| 試行の token 上限 | `min(per_case, per_arm の残り)` を stream-json の累積で監視し、超えたら process group を kill | 同じ値を task の `max_tokens` として渡す。Celeris が run の途中値（assistant 1 応答ごと）を木全体で足し、上限に達したら run を止め、continuation・retry・reviewer を起こさない（ADR 2026-10-20-mlsys-main2-fixes D1） | 上限値と数え方（`token_basis = all`）は同じ。超過の幅は §5.2 |
| 試行の壁時計 | `trial_timeout_secs`（1800 秒）で kill → `Timeout` | 同じ秒数を task の `max_wall_secs` にし、超えたら実行器が task と子孫を cancel API で閉じる | 同じ秒数 |
| 外部の cluster job | 試行内で投げた job は自動で消さない | 同じ。`cluster-cancel.jsonl` に `scancel`/`qdel` の人手手順を残す | 同じ（人が消す。H6） |
| 予算 | `final.json` `budget`: 全体 3,000,000 tokens、per_arm 1,500,000、per_case 1,500,000・1800 秒、`token_basis = all`、`max_parallel = 1` | 同じ | 同じ |
| 共有台帳 | `shared_ledger = /local/celeris/data/measure/main-4/global-ledger`、`approved_trials = 2` | 同じ | 同じ |
| 成功の判定 | executor `passed` かつ独立 grader の `final_pass`（成功契約） | 同じ。Celeris の reviewer の合格は成功に数えない | 同じ |

## 4. retry と timeout の意味

- **試行の再試行**: `termination.max_attempts_per_trial = 1`。campaign の実行器は 1 cell につき 1 試行だけ投入する。成功契約では cell を `trial_no` 最小の記録で判定し、後の `trial_no`（再開・人の再投入）は `executor_trials` に数えるだけで成功に昇格させない。
- **B の内部 retry**: Celeris が task の木の中で run をやり直す（`parent_run_id` を持つ run、decision log の `is_retry`）のは B の仕組みの一部で、同じ試行の中に数える。その run の token と費用は試行の消費に入る。final-accounting の `by_retry` は切り口で、`by_role` と足さない（`additive = false`）。human-op report の `auto_retries` に別掲する。
- **A の retry**: A は 1 process で、外から retry しない。Claude Code 内部の API 再送は観測できないので、A の `auto_retries` は欠測（0 ではない）。
- **timeout**: どちらも `trial_timeout_secs` を越えた時点で止め、outcome は `Timeout`（success class `executor_timeout`）。timeout した試行は分母に残し、失敗として数える。timeout までの消費は試行の消費に入れる（捨てない）。
- **予算で止まった試行**: token 上限で kill・cancel した試行は `executor_cancelled`（理由 `case_budget` 等）で分母に残す。`stop_on_budget = true` で campaign 全体の予算を使い切ったら、残りの cell は未実行（`missing`）のまま分母に残す。
- **再開**: daemon や実行器の再起動後は冪等キー（`<state_dir>/<key>/meta.json`、B は idempotency label）で同じ試行を追う。二重起動で増えた B の task は実行器が新しい方を cancel する。どちらも新しい試行にしない。

## 5. token の内訳を A/B で揃える方法

### 5.1 4 つの欄

どちらの腕も Anthropic API と同じ 4 欄で数え、欄の対応を固定する。

| 欄（集計名） | A0 の取り方 | B0 の取り方 |
|---|---|---|
| input（`input_tokens`） | 最後の `result` 行の `usage.input_tokens` | 木の全 run の `usage.input_tokens` の和 |
| cache read（`cache_read_tokens`） | `usage.cache_read_input_tokens` | 各 run の `cache_read_tokens`（Claude Code の `cache_read_input_tokens`） |
| cache write（`cache_creation_tokens`） | `usage.cache_creation_input_tokens` | 各 run の `cache_creation_tokens` |
| output（`output_tokens`） | `usage.output_tokens` | 各 run の `output_tokens` |

- 比較・予算に使う合計は `token_basis = all`（input + output + cache read + cache write）。内訳の 4 欄も別に出し、合計だけで比べない。cache read は単価が違うので、費用の比較は §6 の定価換算で行う。
- 実行中の上限監視（A の `live_usage`）は stream-json の `assistant` 行を `message.id` で重複除去した累積。最終値は `result` 行。両者が食い違っても最終値を記録に使う。
- 欄が欠けたら欠測にする: input か output が無い試行は tokens 全体が欠測、cache の欄だけ無ければその内訳だけ欠測。0 で埋めない。B は 1 run でも input/output が欠ければ試行の tokens を欠測にする。
- B の照合: 試行の外側の値（`trials.jsonl`）と decision log の run の和を final-accounting が突き合わせる（`match` / `mismatch` / `incomplete`）。足さない。`mismatch` と `incomplete` は gap に残し、その cell の tokens を比較に使わない。A は decision log が無いので `no_breakdown` になる（外側の値だけ）。

### 5.2 予算超過分

上限を越えて kill・cancel した試行の消費は、観測できた分をそのまま記録する。上限値で切り詰めない。どちらも判定は応答の後なので、超過は最後の 1 応答ぶんまで残る。B は止めた run の終端 usage（無ければ途中の累積値）を記録し、どちらも無ければその run は欠測にする。

## 6. 費用の 4 区分（別欄にし、互いに代用しない）

| 区分 | 意味 | A0 の出所 | B0 の出所 | 出る場所 |
|---|---|---|---|---|
| 定価換算（list） | 公開単価 × token。推定 | 未実装（未整合 U1） | 各 run の usage に静的単価表（`task_core::pricing`）を掛けた `list_cost_usd` | final-accounting `total.list_cost_micro_usd` |
| 実支出（cash） | 実際の請求 | 取れない（欠測） | 取れない（`cash_cost_usd` は常に欠測） | final-accounting `total.cash_cost_micro_usd` |
| harness 報告額 | 実行器が試行の記録に書いた金額 | Claude Code の `result.total_cost_usd` | 木の run の `cost_usd` の和（= Celeris の単価表の推定。Claude Code の `total_cost_usd` ではない） | `trials.jsonl` の `cost_micro_usd`、export の `cost_micro_usd_sum`、final-accounting `executor_reported_cost_micro_usd` |
| quota | subscription の使用率（7 日・5 時間など） | `celeris-campaign usage` の報告、開始値 U0 と終了値 | 同じ（同じ account pool なら共有） | final-accounting `quota`（消費行に載らない別項目） |

規則:

1. 4 区分は足さない。ある区分が欠けても別の区分で埋めない（cash が無いから list を cash とする、などをしない）。
2. harness 報告額は A と B で出所が違う（§6 表）。A0/B0 の金額の比較に使ってよいのは、同じ単価表で両腕を計算した定価換算だけ。harness 報告額の差を A/B の費用差として書かない（未整合 U1）。
3. quota は試行ごとに割り振らない。開始値と終了値の差は主測定の期間全体の値として別に書き、他の利用（本番の Celeris・人の会話）が混ざることを明記する。
4. 単価表は版を記録する（`task_core::pricing` の `PRICE_TABLE` は 2026-09 時点の概算で、cache write の単価は 1 種類）。単価表を変えたら、両腕とも同じ表で計算し直す。

## 7. CoS と共有消費の範囲

- 主比較の tokens・費用は試行の run だけ（`export::COST_SCOPE`）。B は試行の task の木の run（planner・worker・reviewer）、A は試行の process。
- CoS の call（人との会話、計画の承認の取り次ぎなど）は試行に含めず、腕にも配分しない。final-accounting の `cos.state` は `observed`・`not_called`（根拠つき）・`missing`/`not_retrieved` のどれかで、根拠が無ければ欠測（0 にしない）。B の task を API で直接作る主比較では CoS は試行の経路に無いので、`cos_not_called_basis` を渡すときはその根拠（「試行の task は実行器が API で作り、CoS を通らない」と、その期間の CoS の消費行が 0 件であること）を書く。
- 共有の消費（測定環境の準備、env-lock の作り直し、cap-probe、preflight、grader の run、人の共有作業）は試行に入れない。human-op は `--scope shared` で 1 度だけ記録し、report の `shared_overhead` に別掲する。final-accounting の `shared_unattributable` も腕に配分しない。
- 独立 grader の消費は campaign の評価側の費用で、A/B どちらの腕にも入れない。

## 8. 金額・token の欠測の扱い

- 欠測は欠測のまま数える。試行の数（`trials_without_record`、`trial_tokens_missing`）と欄ごとの欠測数を出し、和は観測できた分だけの `observed_sum` に出す。`total` は全部観測できたときだけ値を持つ。
- 観測 0 件の和は null（0 ではない）。「呼ばなかった」ことの 0 は根拠（`basis`）があるときだけ `not_called` にする。
- A の定価換算と両腕の実支出は、今の実装では全試行が欠測になる（未整合 U1・U2）。報告では「欠測」と書き、推定で埋めない。
- 未実行・中断の cell は分母に残し、消費は欠測（未実行なら `not_executed`）にする。

## 9. 比較可能でない値を総費用削減の根拠にしない

次のどれかに当たる値は、「B0 は A0 より費用（または token）が少ない」という主張の根拠にしない。報告にはそのまま載せ、比較可能でない理由を添える。

- final-accounting の `reduction_confirmable.<metric>` が false（欠測・gap がある）。
- 両腕で出所が違う金額（A の `total_cost_usd` と B の単価表推定。§6 の harness 報告額）。
- 片方の腕だけで観測できた区分（A の定価換算が欠測で B だけある、など）。
- B の外側と decision log の照合が `mismatch`・`incomplete` の cell。
- 違うモデル・違う Claude Code 版・違う単価表で数えた試行どうし。
- quota の変化（他の利用が混ざり、試行に割り振れない）。
- 成功しなかった試行を除いた和（分母は計画 cell。失敗・timeout・予算停止の試行の消費も含める）。
- 1 cell 1 試行の pilot の値（傾向の確認であって削減の根拠ではない）。

費用の比較を書くときは、同じ単価表で両腕を計算した定価換算と token の 4 欄を、計画 cell 全部の分母で並べる。

## 10. 未整合（既存実装で揃わない点。この文書では実装しない）

- **U1 A の定価換算が無い**: A の試行は decision log を持たず（`no_breakdown`）、final-accounting は A の list を計算しない。A の試行記録の金額は Claude Code の `total_cost_usd`（harness 報告額）だけで、B の記録の金額（単価表の推定）と出所が違うのに、export の同じ列 `cost_micro_usd_sum` に入る。A の 4 欄 token に B と同じ単価表を掛けた定価換算を出す処理が要る。
- **U2 実支出が両腕で取れない**: B の `cash_cost_usd` は常に欠測、A は欄が無い。subscription の account では試行単位の請求は存在しない。cash は「取得不能」として固定し、quota と併記する方針を final-freeze で確定する必要がある。
- **U3 subagent の消費の数え方**: B の run は subagent を禁止し、Celeris は main thread の usage だけを数える。A は Agent/Task を使え、A の token は `result.usage` から取る。`result.usage` が subagent の消費を含むかを確かめておらず、A が subagent を使った試行で token・費用を過少に数えるおそれがある。stream-json の subagent 行の有無を試行ごとに記録する必要がある。
- **U4 Claude Code の版が別々に固定される**: A は `A_PROGRAM`・`A_VERSION`、B の run は測定 daemon の `[adapters.claude_code] command` で、同じ実体・同じ版であることを照合する検査が無い（`final.json` の B `harness_version` は Celeris の release で、run の Claude Code 版ではない）。
  新しい契約 revision `main-4-ops-r1` の manifest（`contract`）では、試行の開始時に daemon の release と両腕が起動する Claude Code の sha256・版を `observed_versions` に記録し、宣言と違えば試行を拒否する（ADR 2026-10-27-trial-observed-versions）。この文書の revision `judged-scope-v2` と固定済み bundle の条件は変えない。
- **U5 B の前置きの固定**: B の run の prompt に載る Celeris の前置き（profile・知識・記憶・必須規則）は、試行の入力 hash（`initial_info`・materials・kb snapshot）に含まれない。測定 daemon の DB（org・memory）が試行ごとに同じであることを示す記録が無い。
- **U6 `initial_info` と材料 hash の食い違い**: `final.json` の `experiment_sets.main.fixed.initial_info` は `dab3ca05d01adf1da268c8ec35e24f33249ca3cc02083e0a2e6b487d0c5a800d`（mlsys-main-1 の材料 hash）で、`snapshot.materials_sha256` と `lock.json` の `materials_sha256` は `ae9007f5f32c1ea4230001ca30c7e47fc6ea1cf51e142ba2e921dbb613af32d4`。checker の `reconcile_set_evidence` はこの差を違反にする。final-freeze で `initial_info` を材料 hash に合わせ、campaign hash を作り直す必要がある。
- **U7 per_arm と per_case が同じ値**: per_arm 1,500,000 = per_case 1,500,000 で、1 腕に K2・K4 の 2 試行がある。先の試行が上限近くまで使うと、後の試行の上限（`min(per_case, per_arm の残り)`）が小さくなり、腕ごと・順番ごとに条件が変わる。pilot（各腕 1 試行）では起きないが、K2 を足す段で確かめる。
- **U8 予算停止の記録の違い**: A は実行器が process group を kill して `executor_cancelled`、B は Celeris が task を `failed`（`token_budget_exhausted`）にし、実行器の case_budget 判定と二重に止める。同じ「予算で止まった」試行が両腕で同じ success class・同じ理由で記録されることを確かめた試験が無い。A の監視は stream-json の累積（`message.id` で重複除去）、B は Celeris の run 途中値で、数え方の一致も未検証。
- **U9 A の自動 retry が観測できない**: A の `auto_retries`・待ち（`event_waits`）は欠測のまま（成功契約の未解決にも記載）。B の内部 retry だけが数に出るので、retry の多さを A/B の差として書かない。
- **U10 quota の割り振り**: `celeris-campaign usage` の値は account 全体の使用率で、主測定の期間に本番 Celeris や人の会話が同じ account を使うと混ざる。測定専用 account にするか、混ざることを報告に書くかを final-freeze で決める必要がある。
- **U11 単価表の版**: `task_core::pricing` の単価表は 2026-09 の概算で、版の識別子を記録に残していない。cache write の単価は 1 種類で、5 分・1 時間の cache を区別しない。
  版の識別子は `task_core::pricing::PRICE_TABLE_VERSION`（`GET /health` の `pricing_version`）として足した。`main-4-ops-r1` の B/C は `pricing_version` を宣言し、試行の開始時に照合する（同 ADR D3）。cache write の区別は未整合のまま。
