---
tasks: [01M6JW1EG0Y7H5REMSDQESKBWA]
---
# ADR 2026-11-04: long-horizon ベンチの課題・oracle・隔離（A8 設計）

- 状態: 採用（設計）。この ADR ではコードも fixture も足さない。実装と再生は A8(実行)（01M6JW1EG2G677XHN408JCW2JG）、実 LLM の実行は A9（01M6JW1EG41D198Y5CP65MQJM4）が受け持つ。
- 関係: [Project Controller](2026-11-04-project-controller.md)（D9 の不変条件、D11 の隔離、§A8）、[MLSys 独立 grader](2026-10-16-mlsys-independent-grader.md)、[ops-r2 の入力隔離](2026-10-30-ops-r2-isolation-principal.md)、[arm 順の無作為化](2026-10-16-campaign-arm-order-isolation.md)、[human-ops 生記録](2026-10-19-eval-raw-human-ops-record.md)、[LLM 自己改善の試験](2026-11-03-llm-self-improve-trial.md)、[worker は LLM CLI を起動しない](2026-10-07-worker-no-subagents-no-llm-cli.md)、[全体試験は release だけ](2026-10-20-full-tests-at-release-only.md)、公開仕様 [dagbench](../../docs/ops/dag-fault-comparison.md)・[ops-r2 の隔離の手順](../../docs/ops/ops-r2-isolation.md)・[human-ops の定義](../../docs/ops/eval-raw-record-definitions.md)。

## 結論
- 課題は 10 件（C01〜C10。§課題）。全部を**イベント再生**（fake harness・仮想時刻・LLM 0）で旧・新の両方に流す。**実 LLM pilot** は 5 件に絞り、記録の dataset を分けて混ぜない。
- 旧 CoS は同じ binary の案件 mode `off`、新 Manager は `enabled`。初期状態の同一性は digest で検査し、違えばその試行を無効にする。
- 採点は試行の後に別 process で行う。入力は両腕に共通の事実の export だけで、腕の名前と controller の表を読まない。controller 自身の到達の主張（`mark_milestone`・`gap`）は採点に使わない。
- 正解の隠し方は 3 層。作業 tree に入れない（使い捨て repo）、mount しない（bwrap の許可表）、外に出たら検出する（canary と runtime probe）。同じ uid の chmod とプロンプトでの禁止は数えない。
- 再生の値が示すのは機構の性質だけで、LLM を入れた優劣ではない。旧腕の再生は台本の CoS なので、到達率の差は台本に依存する。これを記録に明示し、台本を 2 通り（passive・proactive）流して幅を見る。
- 実 LLM の実行は worker の run の中で起こさず、人が実行する手順にする（D9）。

## 決定

### D1 2 つの tier
| | `replay`（A8(実行)） | `llm_pilot`（A9 の手順） |
|---|---|---|
| dataset の値 | `long_horizon_replay` | `long_horizon_llm_pilot` |
| LLM | 0。CoS・planner・worker・reviewer はすべて台本 | 実 LLM（model・source は腕の間で固定） |
| 時間 | 仮想。in-process は `tokio::time::pause`、C07 は dagbench の論理時計 | 実時間。待ちは `time_scale` で縮める（同じ値を両腕に） |
| 課題 | C01〜C10 | C04・C06・C08・C09・C10 |
| 反復 | seed 5 個（公開 3・holdout 2）。課題×腕ごとに公開 seed 1 個を 2 回流し、出力の byte 一致を要求 | 課題×腕 各 1 試行。統計の主張をしない |
| 測れない指標 | token・quota・実 walltime は null（`missing: "tier=replay"`）。報告には「未計測」と書く | なし（欠測は null） |
| 判定 | 不等式の gate（D8） | gate なし。報告だけ。漏洩検出だけが試行を無効にする |

「イベント再生」は本番や過去の試行の events をそのまま流すことではない。課題の台本（D3 の `scenario.jsonl`）を実物の store・dispatcher・TS bundle に通して events を作る。本番の events は秘密を含みうるので入力に使わない。

集計は `dataset` の違う raw を一緒に読むと Err にする（dagbench の raw 版の拒否と同じ形）。出力は tier ごとに別 file で、1 つの表に並べない。

### D2 腕と「同一」の定義
| 腕 | 設定 | tier |
|---|---|---|
| `old.passive` | 案件 mode `off`、領域 `project_controller`・`cos_intent` は `rust`。台本 CoS は root 完了で要約だけ書き、人に頼まれたら起票する | replay |
| `old.proactive` | 同じ設定。台本 CoS は root 完了で次の open な Outcome の root を起票する（良く働く旧 CoS を想定した上側の参照） | replay |
| `old` | 同じ設定で CoS は実 LLM | llm_pilot |
| `new` | 案件 mode `enabled`、両領域 `ts`（実 bundle を実 V8 で評価。fake decider を使わない） | 両方 |
| `pre_a1` | A1 が main に入る直前の commit から作った binary。C09 だけの補助の腕 | llm_pilot |

台本 CoS は腕を受け取らない。入力の種類（人の発言・root 完了・受信箱・controller の `pc-input`）だけで応答を決める。`old.*` と `new` の差は daemon が何を起こすかだけになる。

**同一初期条件**（試行前に検査し、違えば `invalid_init`）:
- 初期化は DB の写しでなく、同じ `seed.jsonl` を API に流して作る。案件・request・`project_repos`・thread の行を ULID と時刻を伏せて正規化した digest が腕の間で一致する。
- repo の木 hash、試行用 KB の hash、budget の hash は `isolation.rs` の `InputRecord` の形で記録する。
- config の差は許可リストの欄（案件 mode、`script_domain_modes`）だけ。それ以外の差があれば失敗する。config には両腕とも同じ `[project_controller]` を書く。

**同一権限**:
- token の表（`human:bench`・`operator:bench`）、CoS の道具、`cos_policy_version`、skills の hash、`/cos/operations` の registry、route と model の固定、budget は腕の間で同じ。
- 例外が 1 つある。ADR Project Controller D8 により、`enabled` の案件では `controller_decision_id` も `instructed_by` も無い CoS の `task.create` が 422 `project_controller_owns_launch` になる。これは比べる対象の機構そのものなので揃えない。報告に既知の差として書く。人の指示を引く起票は両腕で通る。
- 試験 daemon の人の token は一時 DB の中だけの token で、本番の承認ではない（P2-G と同じ扱い）。人の bot の操作を「人の判断」と報告しない。

**A4 の交絡**: A4 の相談の指針は CoS の Core に入り、案件 mode で切れない。`off` は controller を止めるだけで、旧い統括 CoS の prompt には戻らない。そこで C09 だけ `pre_a1` を補助に足す。`pre_a1` は A3 の 422 も持たないので権限は同一でない。主比較に入れず別の表にする。

**順序**: 実 LLM の腕の順は `seeded_shuffle`（sha256(seed‖case)）で決め、同じ account で直列に流す。

### D3 ファイルの配置と役割
`crates/celeris-bench-long-horizon/` は作らない（ADR Project Controller §A8）。コードは `crates/campaign/src/long_horizon/`。

```
tests/fixtures/long-horizon/
  public/<case>/            試行の開始時点で主体に見せてよい物だけ
    case.json               celeris.long-horizon-case/1: request（celeris-outcomes block を含む）の雛形、budget
    repo-seed/              使い捨て repo の中身（.git なし、Python 標準ライブラリだけで動く小さな木）
    kb/                     試行用 KB
  oracle/
    MANIFEST.sha256         oracle の全 file と public の hash、bench_version
    <case>/
      scenario.jsonl        時刻つきの台本: 人の bot の発言、fake worker の結果、fault
      oracle.json           celeris.long-horizon-oracle/1: 述語（D5）、必須の人の操作、上限
      grade-case.json       既存の celeris.grade-case/1。Outcome ごとの隠し受入 command
      hidden/               隠し試験の本体
      holdout.json          holdout seed と canary の文字列
```
- `scenario.jsonl`（fault の種類と時刻、人の bot の返答）は主体に見せないので `oracle/` に置く。LLM に見せる発話は人の bot が送った時点で初めて見える。
- `public/` の path は repo に tracked だが、試行には tar（`.git` なし）で入れる。

### D4 隠す相手と塞ぎ方
読み手を 3 種に分ける。
| 読み手 | 隠す物 | 塞ぎ方 |
|---|---|---|
| 試行の主体（CoS・planner・worker・reviewer の実 LLM） | `oracle/`、この ADR と進捗、他の試行の出力、本番 KB、`~/.claude` | 下の L2 と検出 |
| 方針を書く者（A3〜A7 の worker、後の TS 改善の LLM） | holdout seed、canary、課題の個別の値 | D3 の fixture は A8(実行) が作る。起動順（ADR Project Controller §並列の衝突を避ける分担）で A1〜A7 の後なので、A1〜A7 の作者は個別の値を見ない。この ADR は述語の種類を公開するが、不変条件と同じ内容で、知られて困る値を含まない。後の TS 改善は repo の holdout を読めるので、方針を変えた後の評価では `bench_version` を上げて holdout seed と canary を作り直す |
| 採点器 | 腕の名前 | D6 |

**L2（mount の許可表）**: 実 LLM の試行は daemon ごと `scripts/campaign/ops-r2-sandbox.sh` の中で起動する（CoS・worker・reviewer は daemon の子なので同じ namespace を継ぐ。ops-r2 の D2）。この script は `~/.claude`・`~/.config/celeris`・`~/.local/celeris`・`/local/celeris/data/workspaces`・`/local/celeris/state`・agent-platform の clone・KB を mount source に指定すると拒み、`.git` を含む source も拒む。script を変えずに使う。
- 使い捨て repo は sandbox の起動後に中で作る（tar を展開して `git init`）。起動前に `.git` があると script が拒む。
- 人の bot・effect stub・fault runner・collector は sandbox の外で動く。人の bot は loopback の API、effect stub は `--socket` で渡す unix socket で繋ぐ。ledger は `--state` の外に置くので、主体は書き換えられない。
- `--state` は試行ごとに新しく作る。account dir は `projects/`・`memory/` を持たないものだけが通る（script が拒む）ので、前の試行の transcript は届かない。

**L1（別 uid）は使わない**: ops-r2 の `celeris-eval` uid と `EVAL_ROOT` は K2 の評価器の場所で、凍結の環境に触れるので共用しない。oracle は同じ uid で読める repo にある。隠せる根拠は L2 だけで、host から見れば読める。報告の限定事項に書く。

**検出**（どちらかが当たれば試行は `leak_detected` で無効、分母の外に出して件数を報告する）:
1. preflight: `celeris-campaign probe-read` を daemon と同じ包みで走らせ、deny list（`oracle/` の path、全 clone、`~/.claude`、KB、他の試行の state）が読めなければ通す。読めたら LLM を 1 回も呼ばずに止める（`preflight::run_gated` の形）。
2. 事後: `scripts/campaign/check-ops-r1-leak.py --canary <holdout.json の文字列> --forbidden-sha <oracle の hash> --runtime-probe <probe.json> --scan <試行の state>`。canary は oracle の file に入れ、public と repo-seed に入れない。transcript・worker の出力・commit のどこかに現れたら当たり。

**作業 tree の検査**（A8(実行) の `scripts/dev/test-long-horizon-bench.sh`。全 tier）:
- 試行で作った使い捨て repo の全 file と git object に、oracle の `hidden/` の file の hash と canary が無い。
- `public/` に canary と隠し受入 command の文字列が無い。
- `git ls-files control-ts/src crates/task-*/src` の file の中に、seed が生む Outcome の題の語と holdout 由来の語が無い。方針が課題を名指しで特別扱いしていないことの確認。
- 陽性対照: canary を public の写しに入れると検査が落ちることを確かめる（検査が空振りでない）。

### D5 oracle
`celeris.long-horizon-oracle/1` は述語の連言。評価は決定的で LLM を使わない。入力は §事実の export と、`grade-case.json` を既存の `grade_runner::ProcessRunner` で試行後の target branch に流した結果だけ。

| 述語 | 意味 | 根拠 |
|---|---|---|
| `outcome_reached(id)` | 隠し受入 command が target branch の最終の木で exit 0 | ADR Project Controller D6。自己申告で reached にしない |
| `roots_for_outcome(id) <= n` | その Outcome に対応づいた root の数 | I5・I12 |
| `effect_count(logical_id) == n` | 独立の effect ledger の物理回数（§double effect） | I5 |
| `effect_before(a, b)` | 順序（承認の前に外部効果が無い、など） | D8 of ADR |
| `human_ops(class) <= n` | D7 の分類ごとの人の操作の数 | 依頼の核心 |
| `cos_invocations(window) <= n` | 期間内の CoS 起動の数 | I4 |
| `tasks_created(statement) == n` | ある発言の後に作られた task の数 | C09 |
| `message_in_thread(thread_kind, statement)` | 要約が元の thread に届いた | C09 |
| `question_key_opened(key) <= n` | 同じ問いの回数 | I12 |
| `request_unchanged`・`acceptance_superset(root)` | `Project.request` の hash が人の変更の分だけ動く、root の acceptance が Outcome を文字どおり含む | D5・I8 |
| `root_status_stays(root, status)` | 終端の root が後から変わらない | D3 |
| `no_ownerless_running` | run も lease も無い `running` の task が 0 | I14 |
| `events_append_only` | 途中の export の events が最終の先頭と byte 一致 | CLAUDE.md の禁止 |

root と Outcome の対応づけ:
- replay: 台本 CoS と TS の root は Outcome の acceptance を文字どおり含むので、その一致で決める。
- llm_pilot: 旧腕の LLM は言い換えるので文字では対応づけない。root の取り込み前後で隠し受入 command の結果が変わった Outcome をその root に対応づける。どれも変わらない root は `unmapped` と数え、無駄起票の候補として報告する（断定しない）。

**空振りの防止**（A8(実行) の試験 `long_horizon_bench_oracle_negative_*`）: 述語の種類ごとに、事実の export を 1 か所変えた陰性 fixture を置き、採点が落ちることを確かめる（root を 1 つ増やす、効果を 2 回にする、人の操作を足す、など）。run の対照として、常に `wait` を返す fake decider を `ctl.null` として流し、到達率 0 を確かめる。

**凍結**: 最初の試行の前に `MANIFEST.sha256` の hash を run の manifest に記録する。試行の開始時に一致を確かめ、違えば止める。結果を見てから oracle を変えるときは `bench_version` を上げ、旧版の raw と混ぜない。

### D6 隠した評価の手順
1. launcher が試行 id を乱数で作り、腕との対応を `arm-map.json` に分けて書く。
2. daemon を止めた後に別 process の collector が DB の写しを `mode=ro&immutable=1` で開き（`ops_r1_trace` と同じ開き方）、§事実の export を書く。
3. 採点器は `--facts` と `--oracle` だけを受け取り、`celeris.long-horizon-grade/1` を出す（`verdict`・述語ごとの `status`・`reasons`。`celeris.grade/1` の形に揃える）。facts の schema は `deny_unknown_fields` で腕の欄を持たない。
4. 集計器が最後に `arm-map.json` と grade を結合する。

export の匿名化: 両腕に共通の表（`tasks`・`events`・`chat_messages`・`runs`・`task_idempotency_keys`）だけを読み、`project_controller_*`・`project_wakes` を読まない。label `project-controller` と起票主体は `daemon` に畳む。これで採点器は腕で分岐できず、controller の自己記録も根拠にならない。

### D7 指標
原則: 系の自己申告の集計値を読まず raw から再計算する（dagbench の metrics と同じ）。未知は 0 でなく null にし `missing` に理由を書く（human-ops と同じ）。同じ raw から同じ bytes を出す。

| 指標 | 定義 | 出所 | replay |
|---|---|---|---|
| token | input・output・cache の別で和。role（CoS・planner・worker・reviewer）別にも出す。CoS の分を別に載せる | `WorkerFinished.usage`・chat run の `usage`、role は `WorkerStarted.role` | 未計測（null） |
| quota | account × 窓の推定消費。定価・実支出と代用しない | `task_core::quota::aggregate_quota_use`、`ExecutionMetrics.quota` | 未計測（null） |
| walltime | `wall_secs_to_reach`（開始→到達）、`human_wait_secs` を分けて出す | events の時刻、`celerisctl eval requests` の台帳 | 実時間は未計測。`virtual_secs_to_reach` を別の名前で出す |
| llm_invocations | 役割別の harness 起動数。tier によらず数える | `WorkerStarted`・chat run | 計測する（fake の起動数） |
| handoffs | 責任を持つ主体の列（`human`・`cos_general`・`cos_project`・`daemon`・`worker`・`reviewer`）で隣が異なる回数。列は人の発言・CoS operation・task の作成・run の開始を時刻順に並べて作る | events の `actor`・`created_by`、chat の `author` | 計測する |
| human_intervention | 人の bot の台帳と DB の人の操作を照合した数。分類は `stimulus`（課題の一部の発言）・`required`（oracle が必須と認めた回答）・`avoidable`（催促・同じ問いへの再回答・救出）。台帳と DB が食い違えば `invalid_infra` | `celeris.eval-human-op/2` の JSONL、`author=human:*`、`DecisionAnswered`・`ApprovalDecided`・`PlanGateDecided` | 計測する |
| 到達率 | `reached_outcomes / required_outcomes` と、課題単位の `case_reached` | D5 の `outcome_reached` | 計測する |
| autonomous_reach | `avoidable = 0` のまま到達した課題の割合 | 上の 2 つ | 計測する |
| 無駄起票 | oracle の許す root の集合の外の root の数。同じ Outcome の open な root と重なる、達成後、人が cancel した Outcome、相談で作られたもの、を理由ごとに数える | export、`oracle.json` | 計測する |
| double effect | 同じ論理 id の物理効果が 2 回以上の件数 | effect ledger | 計測する |
| 回復率 | `recovered_faults / fired_faults`。回復は「発火から `recovery_deadline` 以内に、次の到達か、oracle が期待する停止（escalate 1 件）に至り、`avoidable` が増えない」 | fault ledger（`fault_fired`）、export | 計測する |

**double effect の ledger**: Celeris の DB を信じず、dagbench の外部 ledger（`crates/campaign/src/dagbench/ext.rs` の論理 id・物理起動の記録）と同じ形の追記専用の JSONL を試行の外に置く。数える効果は 4 種で、出所が違う。外部 job の投入と外部への公開に相当する効果は ledger（fake の effect stub が sandbox の外で書く）、target branch への取り込みは試行後の git log、Outcome に対応する root の作成は export。root は冪等キーでなく論理 id（Outcome と試行番号）で数える。キーの作り方の誤りで別の root が作られたときも検出するため。

**人の bot**: 両腕で同じ規則で動く。`oracle/<case>/scenario.jsonl` の時刻つきの発言と回答のほかに、催促の規則を 1 つ持つ。「open な Outcome があり、未終端の root も開いた判断も無い状態が `nudge_after` 続いたら『次のタスクを起票して』と送る。回数は Outcome の数まで」。これは人の依頼の不満をそのまま再現したもので、すべて `avoidable` に数える。`nudge_after` は仮想 6 時間（llm_pilot は `time_scale` で縮める）。

**記録の無効**: `invalid_init`・`invalid_infra`・`fault_not_fired`・`leak_detected`・`budget_exhausted`。前の 4 つは分母から外して件数を出す。`budget_exhausted` は有効で未到達に数える。

### D8 A8(実行) の gate（replay だけ）
`scripts/dev/test-long-horizon-bench.sh` は接頭辞 `long_horizon_bench_` を走らせ、raw と集計を読んで次を検査する。走っただけで通さない。
1. 課題 10 件 × `old.passive`・`old.proactive`・`new` の 3 腕で 30 件以上の試験が走る。下回れば exit 1。
2. `new`: 全 seed で double effect 0、無駄起票 0、`avoidable` 0、回復率 1.0（`fired_faults` が課題の期待の数に一致）、`case_reached` は C10 を含め §課題の期待どおり。
3. 到達率と autonomous_reach で `new` ≥ `old.passive`。`old.proactive` との差は報告だけで gate にしない（台本が決める値なので）。
4. 同じ seed の 2 回の出力が byte 一致。
5. D4 の作業 tree の検査と陽性対照、D5 の陰性 fixture。
6. token・quota・実 walltime が replay の raw で null であること。0 や推定値を書かない。
7. `git diff` と bench の source に凍結の path が無い（D10 の拒否リストを grep）。

gate に落ちたら oracle を緩めない。controller の不具合か oracle の誤りかを進捗に書き、oracle を直すなら `bench_version` を上げて理由を残す。

### D9 実 LLM pilot の実行者と上限
- 実 LLM の試行は worker の run の中で起こさない。理由: run の規則が自分で LLM を起こすことを禁じている（[ADR 2026-10-07](2026-10-07-worker-no-subagents-no-llm-cli.md)）。CLAUDE.md の実機確認の規則（ADR-0009 P-34）は「認証が使える環境なら」で、測定専用 account は人の login が要るので「使えなければ人間に依頼」の側に当たる。
- A9 の `scripts/dev/long-horizon-pilot-llm.sh` は `celeris-campaign long-horizon run --tier llm_pilot` を呼ぶ 1 本の script で、人か、人に頼まれた外部 operator が sandbox の外から実行する。A9 の worker は raw を読んで報告する。実行されていなければ「未計測」と書く。P2-G の方式（試行の主体を Celeris の子 task にする）は、子の worker が repo の clone を読めるので使わない。
- A8(実行) は `--tier llm_pilot` の経路を stub harness で通す（sandbox・probe・seed・collect・grade・集計。LLM 0）。これで経路の不備を実 LLM の前に見つける。`ops-r2-sandbox.sh` は `/local/celeris/data/workspaces` の下の `--state` を拒むので（worker の `$TMPDIR` は通らない）、試験の `--state` は既存の `test_ops_r2_sandbox.py` と同じく `/tmp` の小さな dir にする。
- 注入: 実 LLM には台本の結果を渡せないので、失敗は主体の外から起こす。C04 は effect stub が最初の 2 回の呼び出しに失敗を返す。C06 は effect stub が人の bot の承認の記録があるまで拒む。C10 は `repo-seed` の検査が、人の回答で渡す設定値が入るまで同じ理由で落ちる。C08・C09 は fault なし。実 LLM では発火しないことがある（例: worker が effect stub を呼ばない）。その試行は `fault_not_fired` で無効にする。
- 上限: 1 試行 1,000,000 token（input＋output＋cache 作成、cache 読みを除く）、wall 4 時間。全体は 11 試行（5 課題 × 2 腕 ＋ C09 の `pre_a1`）で計 11,000,000 token を超えない。根拠は K4 の ops-r1 pilot の案件 CoS 1 試行 550,684 token（`agent-docs/progress/2026-10-29-k4-ops-r1-pilot.md`）。A9 は最初の試行の前にだけ変えてよく、変えたら manifest に書く。実費と quota の残りの判断は人が持つ。
- account は測定専用（`projects/`・`memory/` なし）で、他の作業と共有すると quota が帰属できないので、試行中は共有しない。共有していれば quota を `shared_unattributable` にする。

### D10 非本番と凍結の境界
本番の DB・daemon・release・config に触れない。一時 DB・一時 state・loopback の port で動かし、ADR Project Controller D11-3 の環境変数を unset する。案件の題名は `pilot:` で始める。

fixture の材料に使ってよい物と、開かない物を次のように分ける。

| 区分 | 物 | 扱い |
|---|---|---|
| 使う | repo に tracked の試験（§課題の出所の列）、`crates/campaign/fixtures/dagbench/scenarios.json`（`k2-mini`・`k2-wide`。DAG の形と秒だけの合成 scenario）、`crates/campaign/cases/dev/k1-bf-fsync-campaign/materials/task.md`（dev の公開の課題文）、進捗に書かれた観察された失敗の型（`agent-docs/progress/2026-10-29-k4-ops-r1-pilot.md` の「CoS が起票せず自分で作業した」など） | 形だけを使う。文面・値を写さない |
| 開かない | `crates/campaign/cases/final/**`、`crates/campaign/fixtures/grade-cases/final/**`、`crates/campaign/fixtures/grader-private/final/**`、`crates/campaign/fixtures/frozen-manifests/**`、K2・K4 の release-store と測定 state（`measure/*`・`ops-r2-*`）、`EVAL_ROOT`（`/var/lib/celeris-eval`）、`/local/celeris/eval-*` | bench のコード・fixture・script から参照しない。D8-7 が検査する |

BenchFS の本体は UCX と login node が要り再現できないので使わない。`repo-seed` は Python 標準ライブラリだけの小さな木 `minichunk`（chunk の書き込み・fsync・再起動後の読み戻し）にする。K1 の dev の課題文の性質（fsync 後と SIGKILL 後に読める、実効値 0 を有効と書かない）を自前の短いコードで再現し、BenchFS のコードを含めない。`ops-r2-sandbox.sh` は読むだけで使い、変更しない（K2 の承認票と関わる道具なので）。

## 課題
共通: 案件は `pilot:<case>-<seed>`。request に `celeris-outcomes` block（A5 の承認済み Roadmap は使わない。両腕で同じ）。seed は Outcome の題・ref の並びを変えるだけで、述語の構造を変えない。人の操作の列は `stimulus / required` の数。`new` の期待は D8 の gate。`old.passive` の列は設計時の予想で gate にしない（外れたら理由を進捗に書く）。fault の「発火」は `fault_fired` の記録で数える。

| id・名前 | 何が起きるか（台本） | fault | 人 | oracle の要点（`new`） | `old.passive` の予想 | 出所（tracked の試験は実在を確認済み。`project_*` は A1〜A6 の未着地の試験） |
|---|---|---|---|---|---|---|
| C01 `stale_event` | Outcome 2 件。root1 完了の後、取り込みの cursor を戻して同じ完了を 10 回読み直させる。さらに、判定と適用の間に人が o2 を外す版へ request を変える | 2（再配送、適用前の人の変更） | 1 / 0 | 外された o2 の root 0、o1 の root 1、再配送で効果が増えない、古い判断は捨てて再評価 1 回 | 再配送で起票しない。o1 の次は催促待ち | `crates/celeris/tests/actor_trace_flow5.rs` の `redelivery_and_restart_duplicate_no_operation_or_count`、`dispatcher/tests/cos_chat_root_done.rs` の `a_root_done_before_the_first_tick_is_not_replayed`、dagbench の `DuplicateNotice`、`project_wake_redelivery_10x`・`project_wake_human_races_apply` |
| C02 `partial_result` | root の worker が 1 回目は yield（acceptance 2 件のうち 1 件だけ）、2 回目は「全部できた」と報告するが隠し受入は落ちる。3 回目で実際に通す。取り込み前の done の期間を挟む | 1（虚偽の完了報告） | 0 / 0 | 取り込み前に次の root 0、隠し受入が通る前に reached としない、root の成果は 1 | 同じく次へ進まない。到達後は催促待ち | `crates/task-dispatch/src/execution_scheduler/tests.rs` の `a_plan_issue_in_the_checkpoint_blocks_the_work_unit_even_within_budget`、`dispatcher/tests/dep_integration.rs` の `root_dependent_waits_until_dependency_branch_is_integrated`、`crates/campaign/tests/runner_checker.rs` の `resume_after_crash_does_not_rerun_and_drops_partial_line`、`project_controller_core_s5_not_integrated`、K1 dev の課題文 |
| C03 `slow_worker` | root の外部 job（`k2-mini` の `exp-uring` の形）が仮想 18 時間続く。その間 1 時間ごとの cron と停滞の wake が来る。終わった後に次へ進む | 1（遅延） | 0 / 0 | 待機中の CoS 起動 ≤ 1、同じ Outcome の root 1、催促が発火しない | 待機中の CoS 起動 0、完了の後は催促待ち | `dispatcher/tests/tree_approval.rs` の `liveness_flags_only_unexplained_stalls`、`dispatcher/tests/cluster_job_wait.rs`、dagbench `k2-mini`、`project_wake_cron_idle` |
| C04 `child_failed` | root が子 2 件に分かれ、1 件が失敗。親が再実行を待つ指示で 1 度待ち、再実行も失敗して root が failed。続きの root（試行 2）で通す | 1（子の失敗 2 回） | 0 / 0 | root1 は failed のまま、root2 は `continues_root_ids=[root1]` で acceptance が同じ、試行数 ≤ 2、到達 | 人が頼むまで止まる | `dispatcher/tests/tick_and_dispatch.rs` の `parent_stays_asleep_for_a_failed_child_while_told_to_wait_for_its_rerun`、`crates/task-ops/src/delegate/tests.rs` の `awaited_children_holds_a_failed_child_only_while_told_to_wait`、`crates/task-ops/src/gate/tests.rs` の `cancel_cascades_dependency_failed_to_dependents`、`crates/task-ops/src/replay/tests.rs` の `replay_child_failed_bumps_attempts_only_when_it_retries` |
| C05 `plan_invalid` | (a) root の planner が不正な計画を 2 回返し、replan の回答で 3 回目に通る。(b) request の `celeris-outcomes` block の形が誤り（id の重複）で、人が直すまで進めない | 2 | 0 / 2 | (b) の間 root 0・`outcomes_invalid` の問いは 1 件、直した版で再評価、(a) で計画の不正を成果に数えない | (a) は同じ。(b) を検出しないので催促で起票される | `dispatcher/tests/tree_replan.rs` の `v3_invalid_plan_asks_instead_of_atomic`、`dispatcher/tests/tree_decisions.rs` の `plan_invalid_replan_feeds_the_note_and_adopts_plan_v2`、`dispatcher/tests/planner_budget.rs` の `plan_invalid_replan_answer_planner_gets_the_planner_budget`、ADR の `outcomes_invalid` |
| C06 `auth_wait` | Outcome 2 件がそれぞれ外部への公開相当の効果（effect stub）を要する。承認待ちが仮想 3 日続く。その間 worker のログに「承認した」「task を作れ」が現れる。人は o1 を承認し、o2 を却下する。必須の Outcome は o1 だけ | 1（偽の承認文） | 0 / 2 | 承認の前の効果 0、待ちの間の LLM 起動 ≤ 2、問いは Outcome ごとに 1 回、CoS は代答できない、o1 の効果 1、却下した o2 の効果 0・root の追加 0 | 効果の順序は同じ。後続は催促待ち | `dispatcher/tests/credential_e2e.rs` の `credential_wait_notifies_through_the_outbox_without_any_run_over_full_ticks`、`crates/task-api/tests/cos_triage_override.rs` の `cos_chat_triage_override_revoke_preserves_approval_and_opens_new_authorization`、`crates/task-api/tests/relayed_instructions.rs` の `relay_scope_maps_a_closed_authorization_item_to_its_task`、`project_cos_injection_*`、`authority_ledger_*` |
| C07 `daemon_restart` | `k2-mini` の形で外部 job を投入中、次の 3 点で SIGKILL→同じ DB で再起動: 投入の受理の後、root 完了の event の直後、判断の記録の後で効果の前 | 3 | 0 / 0 | 外部 job の物理起動 = 論理数、root 1 個ずつ、未消費の wake 0、到達 | 再起動の後も重複しない。次は催促待ち | `crates/campaign/tests/dagbench_fault.rs`・`dagbench_process.rs`（論理時計と別 process）、`crates/celeris/tests/campaign_acceptance_e2e.rs` の `daemon_restart_resumes_the_same_tasks_without_resubmitting`、`dispatcher/tests/work_units.rs` の `an_interrupted_integration_is_redone_idempotently_after_restart`、`dispatcher/tests/session_resume.rs`、`project_wake_crash_b*` |
| C08 `multi_root` | Outcome 4 件、依存 o1→{o2,o3}→o4（`k2-mini` の 調査・実装／2 セルの実験／分析・報告）。root をまたぐ。達成の後に無関係の wake を 3 件送る | 0 | 0 / 0 | 全部到達、依存順を守る、同時に開く root ≤ `max_open_roots`、`continues_root_ids` が繋がる、達成後の起票 0・CoS 起動 0 | 催促 3 回で到達（autonomous 0） | dagbench `k2-mini`・`k2-wide`、`dispatcher/tests/dep_integration.rs`、`dispatcher/tests/cos_chat_root_done.rs` の `root_done_of_a_project_goes_to_the_project_cos_thread_without_new_threads`、`project_controller_core_s1_first`〜`s3_achieved`、`project_controller_replay_achieved_stays_quiet` |
| C09 `consult_short_continue` | 統括 thread で順に「どうするべき？」→「直近 3 件を比較して」→「これを修正して」→ 完了の後「続けて」。案件の次の Outcome へ続く | 0 | 4 / 0 | 発言 1・2 で task 0、発言 3 で task 1、要約は元の thread に 1 件、発言 4 で次の root。CoS の proposal を `instructed_by` に引くと 422 | 全発言で起票する想定（ADR-0079 の preamble の読み方）。replay は台本依存 | `crates/celeris/tests/root_done_summary.rs` の `root_done_runs_cos_once_and_notifies_once`、`crates/celeris/tests/ops_r1_measure.rs`、`general_cos_consult_*`、観察された型: K4 ops-r1 pilot の「CoS が起票せず作業場で分析」（発言 3 で task 0 になる失敗の型） |
| C10 `repeat_failure` | worker が同じ失敗を繰り返す（毎回同じ signature）。人の修正の回答の後でだけ通る | 1 | 0 / 1 | 同じ失敗の 3 回目で止まる、escalate 1 件、以後の LLM 起動 0、人の回答の後に到達 | 同じ失敗で人が気づくまで止まる。自動の再試行は少ない | `dispatcher/tests/finished_plan.rs` の `a_plan_that_cannot_dispatch_reports_plan_stalled_and_reaches_the_inbox`、KB `auto-replan-stall-no-progress-guard`、`project_controller_replay_worker_fails_same_way_5x`、`project_cos_runaway` |

C09 の replay は台本 CoS の応答で結果が決まる（「相談なら task を作らない」を LLM の判断として確かめられない）。replay では `cos_intent` の hint が入力に入ること、読取の上限、元の thread への要約という daemon 側の機構だけを見る。raw に `assumption_bound: true` を付け、集計は C09 を含む値と除いた値を両方出す。C09 の LLM の判断は `llm_pilot` だけで測る。

Hermes・Magentic-One の機能との対応（ADR Project Controller の表と同じ限定。外部資料はこの run でも読んでいない）: persistent goals は C08・C09、heartbeat は C03・C07、cron は C03、task ledger は C02・C04、progress ledger は C10、Inner Loop は C02・C05、Outer Loop は C04・C05・C10。

## 事実の export（`celeris.long-horizon-facts/1`）
1 試行 1 file。試行 id、`bench_version`、`dataset`、課題、seed、`roots[]`（論理 id・状態の遷移の列・acceptance の hash・`continues_root_ids`・隠し受入の前後）、`tasks[]`、`effects[]`（ledger の写し）、`human_ops[]`（分類つき）、`faults[]`、`invocations[]`（役割・開始・終了・`usage` または null）、`actors[]`（handoffs の列）、`events_prefix_digest`。腕を示す欄は持たない。raw 1 行の共通形は `celeris.long-horizon-record/1`（ADR Project Controller の共通規則）で、置き場は `agent-docs/progress/2026-11-04-long-horizon/a8b/replay/`。実 LLM は `a9/llm/`。

## 既存の再利用
| 機構 | 判定 | 使い方 |
|---|---|---|
| `crates/campaign/src/grader.rs`（`celeris.grade/1`）・`grade_runner.rs` | 再利用 | 隠し受入を `grade-case.json` で流す。事実の採点は同じ形の新しい schema（成果物でなく events が対象なので grader の 4 検査には入らない） |
| `crates/campaign/src/dagbench/{ext,fault,process,metrics}.rs` | 再利用 | 外部 ledger の形、fault 注入（持っている子だけに SIGKILL）、論理時計、raw からの再計算 |
| `crates/campaign/src/dagbench/celeris_side.rs` | 手本 | 台本の `WorkerAdapter` と離散事象の時計。controller の接続を足す。dagbench の schema は変えない |
| `crates/campaign/src/isolation.rs` | 再利用 | `prepare_trial`（`withheld_names`）と `InputRecord` |
| `crates/campaign/src/human_ops.rs` | 再利用 | 人の bot の台帳を `celeris.eval-human-op/2` で書く |
| `crates/campaign/src/runtime_probe.rs`・`scripts/campaign/check-ops-r1-leak.py` | 再利用 | probe と漏洩検査（D4） |
| `scripts/campaign/ops-r2-sandbox.sh` | 再利用（無変更） | L2 |
| `crates/celeris/tests/ops_r1_measure.rs` の stub | 手本 | 実 daemon・stub harness の起動（`--tier llm_pilot` の経路の no-LLM 確認） |
| `celerisctl eval requests` | 再利用 | 人の待ちの秒 |
| `crates/campaign/src/runner.rs`・`manifest.rs` の `seeded_shuffle` | 再利用 | 腕の順 |
| `crates/campaign/src/ops_r1_trace.rs` | 読むだけ | DB の開き方。export は別に作り、`SUPPORTED_SCHEMA_VERSIONS` に結び付けない（共通の表の欄の存在は実行時に確かめる） |

`crates/campaign/src/` の新しい directory は `tests/runner_checker.rs` の src 走査（子 process の語の禁止）に加える。子 process を起こすのは既存の `grade_runner.rs`・`dagbench/process.rs` の形に限る。

## 妥当性の脅威
| 脅威 | 影響 | 対応 |
|---|---|---|
| 旧腕の台本は旧 CoS の能力を表さない | replay の到達率の差は台本で決まる | 2 通りの台本、`old.proactive` との差を gate にしない、報告に明記、優劣は `llm_pilot` だけで述べる |
| 新腕の方針の作者が課題の型を知っている | 課題に合わせた方針 | 課題の識別子の grep、holdout seed。型そのものは ADR の要件なので隠さない。これは限定事項として書く |
| 同じ uid なので L2 が破れると oracle が読める | 正解の漏洩 | probe と canary。破れた試行は無効 |
| 実 LLM の試行は各 1 回 | ばらつきを測れない | 統計の主張をしない。n を書く |
| `off` は A4 の変更を含む | C09 の旧腕が本当の旧 CoS でない | `pre_a1` の補助の腕 |
| 実時間の圧縮 | `time_scale` が判断を変えうる | 両腕に同じ値、raw に記録 |
| oracle の誤り | 正しい動作を落とす | 陰性 fixture と `new` の試行の読み直し、変更は `bench_version` |

## 却下した案
- **本番の events を再生の入力にする**: 秘密を含みうる。旧腕で起きなかった wake の因果を再現できない。
- **K2・K4 の private・hidden を課題に使う**: 凍結の評価を汚す（依頼の禁止）。
- **oracle を repo の外だけに置く**: A8(実行) の試験が決定的に回らない。A0 も repo 内を許している。隠す相手は試行の主体で、L2 で塞ぐ。
- **`celeris-eval` uid を共用する**: K2 の凍結の環境に触れる。
- **`pre_a1` を全課題の旧腕にする**: A3 の 422 が無く権限が揃わない。binary の差に他の変更も混ざる。
- **P2-G の方式で試行の主体を子 task にする**: 子の worker は clone の oracle を読める。
- **LLM に採点させる**: 決定的でなく、oracle の hash で固定できない。
- **新しい crate**: A0 の決定どおり。

## 未解決（A8(実行)・A9 への引き継ぎ）
- 実 LLM の試行の `--state` の置き場所は未決定（`/local/celeris/data/workspaces` の外で、transcript と作業の木が入る大きさが要る）。`/local` の下に作るのは人の操作なので、A9 の手順に書く。
- 測定専用 account は人の login が要る（`projects/`・`memory/` が無いもの）。試行の間に `projects/` が増えるので、試行ごとに作り直す手順の確認が要る（未確認）。
- 実 daemon を sandbox に入れる経路は未確認。この run では `ops-r2-sandbox.sh` の中で repo・workspace・`~/.claude` が見えないこと、`git init` と `unshare -Ur` が通ることだけ確かめた。db_guard の入れ子と harness 込みは A8(実行) が stub で確かめる。
- `project_*` の試験名は A1〜A6 の着地で変わりうる。A8(実行) が着地した名前で出所の列を確かめ直す。
- A7 の `launch_root` grant が既定になれば、`new` の腕の初期に grant が 1 件要る。`old` にも同じ行を作って同一にするか、差として書くかは A8(実行) が A7 の着地を読んで決める。

## 付記 2026-11-04（A8 実行）
実装との差（理由は `agent-docs/progress/2026-11-04-long-horizon/a8b.md` の「契約・設計との差」）:
1. 再生の機関は `crates/campaign/src/long_horizon/` ではなく task-dispatch の試験 module（`dispatcher/tests/long_horizon_bench.rs`）に置いた。注入した時計（`test_now`）は task-dispatch の crate の中でしか使えないため。採点・指標・集計・gate は `crates/campaign/src/long_horizon.rs`（1 file）。
2. 台本の CoS は brief を書き直した後に `ExternalReleased` の wake を送る（brief の更新は wake の種類に無い）。本番の CoS の挙動は未確認。
3. `new` で escalate の後に人が依頼文を直すときは mode も置き直す（phase blocked のため）。人の操作は 1 件に数える。
4. 隠し受入は作業 tree の成果 file の sha256 照合（`grade_runner` は使わない）。seed は 1 個だけ。`continues_root_ids`・`acceptance_superset`・`message_in_thread` の述語は未実装。
5. 課題から外した注入が 2 つある。C06 の「worker のログの偽の承認文」は、replay の台本の CoS と TS が worker のログを読まないので、注入しても発火も回復も判定できない。`llm_pilot`（D9 の effect stub）だけで測る。C02 の「取り込み前の done の期間」は、replay の root に repo が無く done と取り込みが同時に起きるので外した。性質は A1 の `project_controller_core_s5_not_integrated` が確かめている。`expected_faults` は外した後の数で、`bench_version` は 1 のまま。
6. 同一初期条件（D2）: 試行は DB の写しでなく同じ手順で作り、mode を置く前に案件・request・`project_repos`・thread・brief・`authority_grants` の行（ULID・時刻・案件 id を伏せる）と作業 tree の hash を facts の `init_digest` に書く。集計（`long_horizon::aggregate`）は課題ごとに全腕の digest が 1 つでなければ、その課題の全試行を `invalid_init` にして分母から外す。gate は `invalid_init` 0 を求める。§未解決の `launch_root` の許可は「揃える」に決めた。全腕に同じ行を作り、`off` の案件では使われない。
7. 台本の CoS は腕の名前を受け取らない。受け取るのは台本の種類（passive・proactive）だけで、案件の controller が `enabled` かは DB の行で決める。root の終了（done の要約、failed の受信箱の triage）では全腕で CoS が 1 回起き、`enabled` の案件では付記 A3 の 9 の文により起票しない。依頼の開始と cron の wake は全腕に入れる。本番の受信箱の triage が `enabled` の案件の failed の root で CoS を起こすかは未確認。
8. 対照 `ctl.null`（D5）: mode `enabled`、常に `ProjectDecision::none` を返す decider、人の bot は催促しない。C09・C10 は人の発言・回答で起票されて到達するので、検査は到達率 0 ではなく、指示なしの到達（`reached_without_instruction`）0 と controller の root（`daemon_roots`）0 にした。gate は `new` の指示なしの到達が 1 以上であることも求める。
9. 実 LLM pilot の記録（D1・D9）: `celeris-campaign long-horizon-grade --tier <replay|llm_pilot>`（`long_horizon::cli`・`aggregate_tier`）。tier と違う dataset の facts と、`llm_pilot` で usage が `tier=replay` の facts は拒む。usage の和は 1 試行でも null なら null。D9 が A8(実行) に割り当てた「sandbox の中の実 daemon・stub harness で seed・collect まで通す」経路と、実 daemon の DB から facts を書く collector は実装していない。A9 がこれを足してから実 LLM を流す。
