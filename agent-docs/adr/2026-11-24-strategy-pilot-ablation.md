---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
---
# ADR 2026-11-24: 戦略層 v1 子 8 の対照 pilot — 3 方式を既存の入口に写し、アーキv4 の replay 3 時点と研究ケース 3 時点で、同じモデル・同じ上限・盲検で比べる

- 日付: 2026-11-24
- 状態: Accepted
- 関係: 戦略層 v1 子 8（task 01M7XQA74FP70G3T3KSTP02S0W、親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE）。
  子 0 の replay（`agent-docs/progress/2026-11-21-strategy-v1-0/replay/`）・研究ケース（同 `research/`）・[rubric](../progress/2026-11-21-strategy-v1-0/rubric.md)、
  子 2 の [ADR 2026-11-21-strategy-evidence-contract](2026-11-21-strategy-evidence-contract.md)、子 5 の [ADR 2026-11-22-strategy-review-critic](2026-11-22-strategy-review-critic.md)、
  子 6 の [ADR 2026-11-22-parent-checkpoint-evaluator](2026-11-22-parent-checkpoint-evaluator.md)（D3・D5.2・D8・付記）と進捗 `agent-docs/progress/2026-11-22-strategy-v1-6.md`、
  A9 の非本番 pilot の承認の門（`scripts/dev/long-horizon-pilot/pilot_gate.py` の `check_approval`）。

## 背景

- 子 6 の第二ゲートは、人（rmaeda）が 2026-11-23 に案 1「条件付き合格」を選んだ（件 01M844NDJ5E5）。条件は 3 点:
  (1) 途中の親 Evaluator の質を、子 8 の pilot で C-mid・D-done・F-done の盲検の評価と人の rubric で測る。
  (2) それまで本番の controller の checkpoint の評価は mode `shadow` のままにする。
  (3) 2 時点の answer key に合わせて評価器の prompt（`task_worker::change_rules::review::parent_checkpoint_section`）を直さない。
- 子 6 の未確認: 独立の評価器の評価から pivot が起きていない。評価器は file を読まず prompt だけで判定した。C-mid・F-done の盲検の評価が無い。
  評価器の入力に親の Outcome と D6 の到達が無い（`crates/task-dispatch/src/review/parent_evaluation.rs` の `request` が `goal.outcomes` と子の `criteria` を空で渡し、`facts` が `goal_reached` を `false` に固定する）。
- 人の答え（2026-11-24。operator が AskUserQuestion で計画 v2 の段を示して聞き、人が「承認する」を選んだ）:
  - 評価器の入力の欠けは、同じ段 prep の WU eval-input・eval-seed が測定の前に直す。
  - 実 LLM の pilot は **2 反復・最大 48 run・25 USD** で、operator が worker の外で流す。
  - 研究ケースは**内部の評価だけ**に使う（ChatGPT・Muse に送らない）。
  - 合格線は **rubric.md のまま**。
  - 評価器の prompt の文面（change_rules）は変えない。本番の controller は shadow のまま。HPC・canary・外部公開・本番昇格はしない。
- 子 8 の依頼: 子 0 の中間 snapshot だけを使い、未来の結果を隠して再生する。旧の固定 Task tree・途中の親 Evaluator・Evidence + 複数戦略の 3 方式を同じモデル・同じ予算で比べる。
  人が rubric で独立に採点し、採点の材料を root の artifacts の `pilot-scoring.md` に揃える。合格を実証できなければ rollout しない。

## 決定

### D1: 3 方式を既存の入口に写す（新しい production の口を足さない）

3 方式とも、子 6 の driver（`scripts/dev/parent-eval-blind.sh` → `scripts/dev/parent-eval-blind/driver.py`）が起こす一時 home の daemon（一時 DB・loopback。
本番の DB・daemon・config に触れない）に、時点の slice から使い捨ての案件・strategy root・子を seed して流す。方式の違いは seed と controller の mode だけで作る。

| 方式 | 中身 | 既存の入口（file・関数） | 1 時点の LLM run | 採点に渡す記録 |
|---|---|---|---|---|
| (a) 旧の固定 Task tree | 子の局所の final review だけ。親の途中評価は無い | 子の final review: `crates/task-dispatch/src/review.rs` の `review_task`（`ReviewRequest.parent_checkpoint` は常に `None`。同 file の request の組み立て）。親の評価を起こさない: 案件の controller を mode `shadow` にし、`evaluate` の判断は `effect_state = suppressed` で reviewer run を起こさない（`docs/ops/project-controller.md`、試験 `parent_checkpoint_regression_shadow_no_reviewer_run`） | 0 | 時点の slice の子の報告と局所の review の判定（`reviewer_passed`）、`evaluate` の判断の行（`suppressed`）、固定の計画の次の手（計画どおり次の子へ進む） |
| (b) 途中の親 Evaluator | (a) に、checkpoint ごとの独立の評価を足す（子 6） | controller の mode `enabled` → `evaluate` → `crates/task-dispatch/src/health_review/design_gate.rs` の `start_side_review`（`SideReviewKind::ParentEvaluation`）→ `crates/task-dispatch/src/review/parent_evaluation.rs` の `launch`・`request`・`settle`・`facts` → `task_core::parent_checkpoint::check_evaluation`・`allowed_actions`（`crates/task-core/src/parent_checkpoint.rs`）。prompt は `change_rules::review::parent_checkpoint_section`。子の報告は `task_core::parent_checkpoint::child_reports` | 1（評価の reviewer run） | `parent-evaluation/<判断 id>.json`（最終の記録）・`reads.json`・`run.json`（driver の出力） |
| (c) Evidence + 複数戦略 | (b) に、子 2 の strategy の版（候補 2〜3・識別実験・反証条件・保留する前提）と子 5 の Critic を足す | 版: `celeris.strategy/1`（`task_core::strategy` の `validate_document`、成果物 `strategy/<id>/v<n>.json`、承認は決定 `strategy-approve-v<n>`）。Critic: 版を書いた task の final review（`review_task`）で `ReviewRequest.strategy_critic` を `Some` にする（`task_core::strategy_critic::critic_subjects`、`change_rules::review::critic_section`）。起票の gate: CoS の `task.create` の `strategy_experiment_phase`・`strategy_revisit_rejected`・`strategy_failure_unread`。照会: `task_core::strategy_reads::strategy_task_creates` | 1（評価）＋ 案件・反復ごとに版 v1 の 2（版を書く run・Critic）＋ 最初の pivot・識別実験の後に v2 の 2（D3.3） | (b) の記録に、版 v1（・v2）の候補・識別実験・反証条件、Critic の判定、v2 の起票の gate の結果を足す |

#### D1.1 方式 (a)

- strategy root は seed するが、controller を mode `shadow` にする。checkpoint は導かれ、`evaluate` の判断は `suppressed` で記録され、reviewer run は起きない。
  これが「親の途中評価なし」の既存の入口である（子 6 D9 の回帰と同じ経路）。
- 子の局所の final review は、replay・研究ケースとも**史実の記録**（slice の events の review の判定と報告）を使い、この pilot では LLM で流し直さない。
  理由: `review_task` は候補の commit と差分を入力に取るが、子 0 の replay の slice は events と進捗の文だけで、子の差分を持たない。差分の無い final review を流すと、
  別の入力の review になる。したがって方式 (a) の局所の review を同じモデルで流し直した結果は**未確認**として報告する（D1.5）。
- 方式 (a) の LLM run は 0 で、費用の指標は史実の値（replay は子 0 の rubric の参照: 未達の検知の後に worker の run 137・264.25 USD、list price）で数える。

#### D1.2 方式 (b)

- 子 6 の driver の流れのまま: 承認済み Roadmap（版 1）を一時 DB に置き、controller を mode `enabled` にし、wake で `evaluate` を起こす。
- strategy の参照は子 6 の seed と同じ名目の `StrategyRef`（文書の本体は作業 tree に置かない）。評価器は戦略の候補を知らずに評価する。
- 評価器の入力は、D8 の修正（eval-input・eval-seed）の後のものを使う。

#### D1.3 方式 (c)

- 版 v1 は、案件・反復ごとに 1 回、**最初の時点より前の情報だけ**から書く（replay は goal と A-mid の slice、研究ケースは goal と 11.15 節）。書くのは一時 daemon の中の既存の worker の run
  （CoS の skill `config/skills/cos-operator/strategy-review.md` の手順で `celeris.strategy/1` を成果物に書く task）で、その task の final review が Critic を兼ねる。
  operator や実装者は版を手で書かない（未来を知る者が書くと漏れになる）。
- 版 v1 の承認（`strategy-approve-v1`）は一時 DB の seed として driver が `human:` の actor で答える。これは人の承認ではなく、使い捨ての DB の中の段取りである。
  結果の報告では「一時 DB の承認は人の承認ではない」と書く。本番の DB・決定には触れない。
- 各時点の評価は (b) と同じ入口で、`StrategyRef` が v1 の文書を指し、作業 tree に v1 の文書（と Critic の判定）を置く。
- 評価が `pivot` か `discriminating_experiment` を選び、Rust の写しが `replan`（`checkpoint_pivot`・`checkpoint_experiment`）になった最初の時点だけ、
  v2 を書く run と v2 の Critic を流し、v2 を参照する `task.create` を CoS の API で 1 回試して gate の結果（通る・`strategy_*` の拒否）を記録する。
  2 回目以降の pivot は流さない（replay の後の時点は史実の続きで、pivot した方式の続きではないため）。

#### D1.4 足さないもの

新しい production の口・Manager・trait・状態機械・DB 表・migration・events の種類・Agent サービス・config の欄は足さない。
driver・比較・採点材料の script（`scripts/dev/` の下）と task-api の試験の seed（`crates/task-api/tests/` の下）だけを、後の段の WU（arm-driver・research-driver・pilot-compare・pilot-materials）が変える。

#### D1.5 既存の入口で走らせられない部分（未確認に書く）

- 方式 (a) の子の局所の final review を同じモデルで流し直すこと（D1.1）。
- 方式 (c) の版を書く run・Critic・v2 の起票を一時 daemon で流せない場合（arm-driver が確かめる）。そのときは手で書いた版で代えず、(c) の該当部分を未確認として報告する。
- 研究ケースの時点で checkpoint を 1 回だけ発火させる `children_per_checkpoint`（設定の範囲 1〜8）を選べない場合（D2.2）。

### D2: 時点

#### D2.1 replay（アーキv4）: C-mid・D-done・F-done

- 子 6 D8.1 の N = 2 の発火の時点で、終端の子はそれぞれ 2・4・6 件、trigger は `children:2`・`children:4`・`children:6`。人の条件 1 の 3 時点である。
- slice は子 0 の `replay.py build` の出力を写した fixture（`crates/task-api/tests/fixtures/parent_checkpoint_shadow/<時点>/`。各 file の sha256 は `replay/manifest.sha256` と一致）。
- E-mid は流し直さない。子 6 の 4 run（`crates/task-api/tests/fixtures/parent_checkpoint_live/blind/` と `compare.json`、operator の 2 回目）を参照として表に載せる。
  4 run は D8 の修正の前の入力で流れたので、pilot の結果と合算しない。

#### D2.2 研究ケース（BenchFS ISC27 task C）: 11.16・11.17・11.20

- `research_case.py` の `POINTS` から 3 点: **11.16**（介入の前。同じ優先度の共有で止まる）、**11.17**（早すぎた結論「止まらなかった」）、**11.20**（反証。yield でも止まった）。
  子 0 の研究ケースの「時点ごとの採点の参照」の 3 行と同じ時点である。scope の cases の名は `research-11.16`・`research-11.17`・`research-11.20`。
- slice は `research_case.py` の `slices`（11.15 節の始まりから 11.k 節の終わりまで）。本文は `c-results.md`（sha256 を `research_case.py` の `SHA` で固定）を実行時に読み、作業 tree は `$TMPDIR` の下に作る。
- seed: 研究の root に、11.15 節からの各節を終端の子として置き、各節の本文を子の報告にする。goal と Outcome は task C の目的のうち 11.15 より前に書かれた問いから作り、
  sentinel（`research_case.py` の `SENTINELS`）の検査を通す。時点で checkpoint が 1 回だけ発火するように、driver が `children_per_checkpoint` を範囲 1〜8 から選ぶ（選べなければ D1.5）。
- repo の試験と stub の確認は合成の testdata だけを使う（research-driver の WU）。

##### D2.2 付記 1（2026-11-24、WU research-goal）: goal と Outcome の出典の節と規則

- 直す前の誤り: `research.py` の `build` は、問いを 11.15 節の見出し、判定方法を見出しの後の最初の段落から作っていた。実データの 11.15 節は sweep の 1 本目の結果の節で、
  最初の段落は「結論:」で始まる結果の段落である。そのため goal と Outcome が 11.15 の時点の結果になり、問いと判定方法になっていなかった（operator の stub の 1 回で人が確認、2026-11-24）。
- 出典（`research.GOAL_SOURCES`）。どれも 11.15 節より前に書かれた節で、task C の目的の「追加の依頼（2026-11-15）」の C（node の停止で欠けた cell を欠測にせず、
  原因の見当をつけ、避ける手を打ってから測り直す）を扱う:
  - 問い（Outcome の `title`、goal の「目的」）: 11.2 節の「人の答え」を含む箇条のうち、「止ま」を含む文。人が 2026-11-15 に答えた k = 24 の cell の停止の扱い。
  - 判定方法（Outcome の `acceptance` の 1 件、goal の「判定方法」）: 11.5 節の「停止の割合」を含む箇条の全文。停止を host × cell の数で数える数え方と、11.15 より前の割合。
  - 背景（goal だけ）: 11.1 節の「人の決定」を含む箇条の全文。アプリ同居を欠測で閉じる案を人が採らなかったこと。
  - 使わない節: 0 節（後で書かれた結論）、11.15 節以降の節、9.10 節（本文の 2 行が 11.15 より後に書き換えられていた。下の根拠）、12〜16 節（後の run で書き足された）。
- 規則: 箇条は `- ` で始まる行のうち印を含む最初の 1 行、文は「。」で切る。出典の節・箇条・文が無いとき、出典が 0 節のとき、文書の中で 11.15 節より後ろにあるときは
  `ValueError` で止まり、代わりの節を探さない。11.15 節の見出しと最初の段落は、問いにも判定方法にもしない。Outcome の `id` は `research-question` のままで、時点で変わらない。
  人の答えは指示の文なので、Outcome の `title` も指示の文になる。研究ケースの本文を repo に写さないため、問いの形に書き直さない。
- 11.15 より前に書かれた根拠: task C の作業場所に、run 25 の前の c-results.md の写し `/local/celeris/data/workspaces/01M7C56WK4E86BTZNDKAX7SP7C/target/.ctmp/c-results.pre-r25.md`
  （sha256 `baa2a1858f04844330593fccd57e1380f4d1b8c75e910f8e253c5d8d59b0f8a0`、mtime 2026-11-16 08:42Z、最後の節は 11.13）がある。11.15 節は run 26（2026-11-16 10:3xZ）である。
  出典の 3 節（11.1・11.2・11.5）は、この写しと最終の c-results.md（`research_case.SHA`）で同じ文である（`check-goal --written-before` が「前の版と同じ文の出典: 3 / 3 節」）。
  9.8・9.9 節も同じ文だが、9.10 節は 2 行が違う。節の見出しの時刻は 11.1 が 2026-11-15 12:20Z、11.2 が run 14（2026-11-15 12:1x〜12:3xZ）、11.5 が run 17（2026-11-15 13:30Z）。
  task C の `target/` は掃除で消えうるので、同じ sha256 の写しを子 8 の root の artifacts の `research-evidence/c-results.pre-r25.md`（`SHA256SUMS` 付き）に置いた。repo には入れない。
- 検査: `python3 -I scripts/dev/parent-eval-blind/research.py check-goal --src <c-results.md> --points 11.16,11.17,11.20 [--written-before <前の版>]`。
  各時点の build の `Leak`、または goal・Outcome に 11.15 節の見出し（番号の有無とも）か最初の段落があれば exit 1。出力は節の番号と件数だけで、本文を出さない。
  実 LLM の run の前に実データで流す。

### D3: 同じモデル・同じ予算と承認の json

#### D3.1 上限（3 方式で同じ値）

| 項目 | 値 | 掛け方 |
|---|---|---|
| harness・model・account | 承認の json の `harness`・`model`・`account` の 1 組（推奨は子 6 の 4 run と同じ `claude_code`・`claude-sonnet-5-5`） | 全方式・全 run（評価・版・Critic）で同じ。driver が `--llm-config` の 1 つの TOML を全時点に使う |
| 1 run の上限 | `max_turns` 30・`max_wall_secs` 600・`max_retries` 0 | seed の root・子・版を書く task の `Budget` に同じ値を書く |
| 評価の試行 | `max_evaluation_attempts` 1（`[project_controller.checkpoint]`） | `invalid` の出力も結果として数え、試行し直さない |
| 1 時点の費用の上限 | `max_usd_per_day` 3.0 USD（`[project_controller.budget]`） | 一時 daemon ごと。方式 (a) は使わない |
| 待ち時間 | 1 run の記録を待つ上限 900 秒（run の 600 秒と起動の余裕） | driver の待ち。超えたら失敗の記録 |
| 全体 | 48 run・25 USD | D3.4 の止め方 |

#### D3.2 承認の json（人が書き、root の artifacts に置く。commit しない）

```json
{
  "approval_id": "strategy-v1-8-pilot-<日付>",
  "approved_by": "human:<名>",
  "approved_at": "<ISO 8601>",
  "expires_at": "<ISO 8601>",
  "account": "<人が選ぶ account>",
  "model": "claude-sonnet-5-5",
  "harness": "claude_code",
  "scope": {
    "arms": ["a", "b", "c"],
    "cases": ["C-mid", "D-done", "F-done", "research-11.16", "research-11.17", "research-11.20"],
    "environment": "non-production"
  },
  "limits": {"max_usd": 25, "max_wall_secs": 600}
}
```

- 検査は既存の `pilot_gate.py check-approval`（`--arms a,b,c --cases <時点>`）。`limits` は `max_usd`・`max_wall_secs`・`max_quota_pct` だけを受けるので、run の数（48）は欄にせず、
  driver が計画した run の数（D3.3）と実際の run の数（`run.json`）で数えて断る。
- driver は Celeris の worker の run の中（`CELERIS_RUN_ID` がある）では実 LLM を断る（子 6 の `parent-eval-blind.sh` と同じ）。

#### D3.3 実 LLM の run の見積り（方式 × 時点 × 反復）

時点は 6（replay 3・研究 3）、反復は 2。

| 方式 | 1 時点 | 案件・反復ごと | run の数（上限） | USD の見積り |
|---|---|---|---|---|
| (a) | 0 | 0 | 0 | 0 |
| (b) | 評価 1 | — | 6 × 2 = 12 | 12 × 0.5 = 6.0 以下（子 6 の実測は 1 run 0.12〜0.14 USD、4 run で 0.487 USD） |
| (c) | 評価 1 | v1（版を書く run 1・Critic 1）、最初の pivot・識別実験の後に v2（1・1） | 評価 12 ＋ v1 2 × 2 × 2 = 8 ＋ v2 2 × 2 × 2 = 8 = 28 | 28 × 0.5 = 14.0 以下 |
| 予備 | 基盤の失敗（run が `completed` で終わらず記録が無い）の再実行だけ | — | 8 | 4.0 以下 |
| 計 | | | **48** | **24.0 以下（上限 25）** |

- 1 run 0.5 USD は版を書く run と Critic の実測が無いための上側の見積りで、評価の run は子 6 の実測から 0.15 USD 前後と見る（期待値の計は約 12 USD。推定）。
- 予備は内容が悪い・answer key と食い違うことを理由に使わない（結果の選び直しになる）。使った予備は run ごとに理由を記録する。

#### D3.4 止め方

- 実行の順: 反復 1 の全時点 → 反復 2 の全時点。時点の中の方式の順は、`pilot-key.json` に記録した乱数で決める。
- 各時点の前に、使った USD ＋ 3.0 が 25 を超える、または使った run ＋ その時点の計画の run が 48 を超えるなら、その時点を始めずに止め、残りを「未実施」と記録する。
- 方式 (a) は LLM を呼ばないので、上限で止まっても流す。

### D4: 漏れの否定対照（未来の情報・answer key を入力と作業 tree に入れない）

#### D4.1 検査の対象と語

- 対象: 方式・時点ごとの seed の spec、作業 tree の全 file（方式 (c) の版と Critic の判定を含む）、run の後の `request.json`・`prompt.md`（run の dir から取る）、`reads.json`。
- 語と印:
  - replay: `replay/sentinels.json` の語のうち `first_seen` が時点の cutoff より後のもの、answer key の印（`ideal_verdict`・`ideal_action`・`answer_key`・`celeris.replay-answer-key/1`）、
    `answer_key.json` の各時点の `why` の文、cutoff より後の event の時刻と本文（子 6 の `parent_checkpoint_blind_no_answer_key_or_future` と同じ規則）。
  - 研究ケース: `research_case.py` の `SENTINELS` のうち初出が時点より後のもの、時点より後の節の見出し、子 0 の `research-case.md` の「時点ごとの採点の参照」の表の文。
  - 読み: `reads.json` に answer key・`research-case.md`・`c-results.md` の全体・replay の時点より後の dir の path があれば `forbidden`（子 6 の `parent-eval-blind-compare.py` の `check_reads` と同じ）。
- 規則は既存の `replay.py`（`verify` の N1〜N3）・`research_case.py` の `leaks`・`parent-eval-blind-compare.py` の `check_reads` を import して使い、Python に写し直さない（規約 2）。

##### D4.1 付記 1（2026-11-24、WU compare-leak）: 時点以前にも同じ本文がある後の event の本文は漏れに数えない

- 変更: replay の「cutoff より後の event の本文」の規則は、後の event の `outcome`（40 字超）のうち、cutoff 以前の event の `outcome` のどれかに同じ文字列が
  含まれるものを数えない（`parent-eval-blind-compare.py` の `Rules.leaks`）。時点より後にだけある本文は従来どおり漏れに数える。
- 理由: operator の実 run（iter-1・iter-2 の C-mid の方式 a・b）で、`input/tree/events.jsonl` は cutoff 以前の行だけを持つのに、定型文
  `interrupted: aborted (task no longer running under this lease)` が後の event の本文と一致して漏れとされ、`compare --pilot` が exit 1 になった。
  この定型文は replay の events で C-mid の cutoff より前の task C の `worker_finished` に 57 件（実 run の C-mid の入力にも 57 行）、後の task D に 27 件ある。時点以前の本文にもある文字列は、入力にあっても時点以前の event から来うるので、
  未来の情報の根拠にならない（誤検出）。replay の 12 時点のうち、この除外に当たるのは C-mid・C-done の上の 1 種だけ。
- 変えないもの: sentinels の語・answer key の印と理由の文・期待の組・後の event の時刻・`events.jsonl` の cutoff 後の行・研究ケースの規則・読みの検査。
  後の event そのものが入力に入れば、時刻と cutoff 後の行の規則で引き続き漏れになる。`testdata/pilot-runs-leak` の出力（`metrics.json`）は変更の前後で同じ byte。
- 子 6 の試験 `parent_checkpoint_blind_no_answer_key_or_future` の規則は変えない（子 6 の盲検の時点 E-mid・D-done では、除外に当たる本文が 0 種）。上の「子 6 と同じ規則」は、本文の除外を除いて同じ、と読む。
- 試験: `test_pilot_compare.py` の `test_body_also_before_the_cutoff_is_not_a_leak`（変更前に落ちる）と `test_body_only_after_the_cutoff_is_a_leak`（否定対照）。

##### D4.1 付記 2（2026-11-25、WU compare-reads。人の決定 2026-11-25）: 書き込みの heredoc と名の部分一致を禁じた読みに数えない。invalid の記録を採点材料に示す

- 変更 1（`parent-eval-blind-compare.py` の `check_reads`）: Bash の command では、heredoc の本文（演算子の行の次の行から区切りの語の行まで）と
  redirect（`>`・`>>`）の書き先を、禁じた名と照合する文から除く。redirect の書き先の切り出しは `mutation` が書き込みに数える正規表現と同じ
  （書き込みに数えるものを読みに数えない）。heredoc の本文の中の、作業 tree・home の外の絶対 path（`reads_outside` と同じ切り出し）は
  引き続き禁じた名と照合する。
- 変更 2（同じ）: 末尾が `/` の禁じた名（`replay/`・`agent-docs/adr/`）は、path の成分の頭（文の先頭・`/`・空白・引用符の直後）でだけ一致とする。
  ほかの名（`strategy-v1-6`・`c-results.md`・`answer_key` など）は部分一致のまま。
- 前提: 評価器と版を書く run の作業 tree と一時 home には、禁じた file（answer key・`research-case.md`・`c-results.md`・子 0 の `replay/`・ADR・子 6 の進捗）
  が無い。heredoc の本文に書いた相対の名や redirect の書き先から、禁じた file は読めない。禁じた file を読むには外の絶対 path が要り、
  それは `reads_outside` が今までどおり記録し、heredoc の本文の中にあっても禁じた名と照合する。この前提が崩れる（driver が作業 tree に
  禁じた file を置く）ときは、この付記を見直す。
- 理由: operator の実 run（root の artifacts の `pilot-runs/iter-1`・`iter-2`）に当てた `compare --pilot`（`pilot-compare-v3`）が、8 点・16 件の
  禁じた読みで exit 1 になった。16 件はどれも実際の読みではない。
  - 6 件（研究ケースの方式 b・c）: `cat > artifacts/review.json <<'EOF'` と `python3 -I - <<'EOF'`（review.json を書く script）の heredoc の本文に、
    driver の入力（`request.json`・`prompt.md`）にもある名 `c-results.md` があった（変更 1 で消える）。
  - 10 件（replay の方式 c の版の段、2 反復で 5 件ずつ）: 版の path `artifacts/strategy/pilot-replay/v1.json` の `pilot-replay/` が禁じた名 `replay/` に
    部分一致した（変更 2 で消える。うち 4 件は heredoc の本文にだけあり、変更 1 でも消える）。
- 変えないもの: `reads_outside`（読みの文の全体から取る）、Read の path と Grep・Glob の pattern の照合（部分一致。末尾が `/` の名は成分の頭）、
  sentinels の語・answer key の印と理由の文・期待の組・時刻・event の本文・研究ケースの規則、子 6 の呼び方（`--pilot` なし）の出力。
  `testdata/pilot-runs-leak` の `metrics.json` は変更の前後で同じ byte。実 run の `metrics.json` は漏れ・`excluded`・`pass` と各点の `leaks`・`forbidden_reads` の
  ほかは変更の前後で同じ。
- 変更 3（`pilot_score.py blind`）: record の status が `invalid` の記録に `- 評価の記録の状態: invalid（run: <run.json の status>、<violations>）。判定と次の手は
  gate の既定値で、評価器の答えではない` の 1 行を出す（方式の名は出さない）。人の決定 3 で、方式 c の 4 点の invalid（turn の上限で評価器が答えを
  出せなかった）は流し直さず、そのまま採点材料に渡す。
- 試験: `test_pilot_compare.py` の `test_cat_heredoc_body_is_not_a_read`・`test_python_heredoc_writing_review_is_not_a_read`・
  `test_strategy_path_pilot_replay_is_not_the_replay_dir`（3 件とも変更前に落ちる）、否定対照 `test_real_reads_of_forbidden_names_stay_leaks`
  （Read の path の `replay/answer_key.json`・`cat c-results.md`・`cat agent-docs/progress/2026-11-22-strategy-v1-6.md`・heredoc の外の引数・heredoc の本文の中の
  外の絶対 path・heredoc の後の行が exit 1）、`test_heredoc_split_follows_bash`。`test_pilot_score.py` の
  `test_invalid_record_says_the_verdict_is_the_gate_default`（変更前に落ちる）。

#### D4.2 混ぜた入力の否定対照（exit 1 を確かめる）

- C-mid の入力に F-done にだけある event を 1 件足す。
- C-mid の入力に `answer_key.json` の `why` の文を 1 つ足す。
- `research-11.17` の slice に 16 節（停止の総括）を足す（`research_case.py verify` の否定対照と同じ）。
- 3 件のどれかが exit 1 にならなければ、検査が壊れているとみなし、実 LLM の run を始めない。

#### D4.3 いつ検査するか

- run の前: seed の spec と作業 tree を検査し、exit 1 ならその run を起こさない。
- run の後: `request.json`・`prompt.md`・`reads.json` を検査し、exit 1 ならその結果を採点から外し、外した理由を報告に書く。

### D5: 盲検

- driver が pilot の始めに、案件ごとに 3 方式へ乱数の id（`secrets` の 8 文字）を振り、出力の dir と file の名に id だけを使う。対応表は root の artifacts の
  `/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts/pilot-key.json` に置き、commit しない。
- `pilot-scoring.md`（root の artifacts）は id ごとの記録を並べ、`pilot-key.json` の sha256（64 桁）を書く。記録から方式の名・run id・判断 id・harness の名を外す。時点の中の記録の順は乱数で並べ替える。
- answer key（`replay/answer_key.json`）と研究ケースの参照の表は `pilot-scoring.md` に埋め込まない。「採点の後に開く」節に path だけを書く。
- 採点者は人（rubric.md の手順 1〜5）。実装者・CoS・operator は採点しない。close-audit が人の採点の後に `pilot-key.json` の sha256 を確かめてから開封する。
- 既知の限界: 方式 (a) の記録は評価の本文を持たず、方式 (c) の記録は戦略の版を持つので、内容から方式を推測できる。伏せるのは名前だけで、この限界を採点の結果と一緒に報告する。

##### D5 付記 1（2026-11-25、WU machine-axes。人の決定 2026-11-25）: 採点の分担と採点の場所

- 分担: 人（rmaeda）は 4 軸（execution success・epistemic progress・parent goal achievement・unsupported claims）とレビュー時間（分）だけを付ける。
  pivot の時点とコストの 2 軸は人が付けず、close-audit が人の採点の後に `pilot-key.json` を開いてから機械で付ける（規則は D7 付記 1）。
  rubric.md の「採点の手順」3 にこの分担を書いた。「軸と基準」から末尾まで（合格線・史実の参照の点）は変えていない。
- 採点の場所: 人は claude.ai の非公開の Artifact（operator が作る web 画面）で採点する。研究ケースの文をその Artifact に置くことは人が許可した
  （11-24 の研究ケースの使い道の決定にある「人の採点」に当たる）。operator が画面の入力を root の artifacts の `pilot-scoring.md` の採点欄に書き写す。
  採点欄の採点者は `human:rmaeda`。書き写したのが operator であることは close-audit が進捗に書く。
- 作り直さないもの: score の段の成果物（`pilot-scoring.md` の記録・id・採点欄の印、`pilot-key.json`〈sha256 `36841f6b189dbecbbb0217108f1118ab235c17c73fe800528ea2b6f8de64aace`〉、
  repo 用の写し `agent-docs/progress/2026-11-24-strategy-v1-8/pilot-scoring.md`）。人は今の id で採点している。
- そのため材料に埋め込んだ「採点の手順」節は、材料を作った時点の手順 3（6 軸を人が付ける）のままである。`pilot_score.py` の `check-blank`・`check-human`・
  `check-scored` は「採点の手順」節の有無だけを見て、「軸と基準」「合格線」は rubric.md との一致を見る。
- 検査の口（`scripts/dev/parent-eval-blind/pilot_score.py`）:
  - `check-human <md>`: 人の 4 軸が 0〜2 で根拠が空でなく「機械:」で始まらない、採点者が `human:<名>`、レビュー時間（分）が数、pivot の時点とコストの行は
    空か「機械:」の根拠と 0〜2 の点。operator が書き写した後に流す。
  - `machine-axes --scoring <md> --key <pilot-key.json> --metrics <metrics.json> --answer-key <answer_key.json> [--out <json>]`: check-human、
    `pilot-key.json` の sha256（採点の file の値）、`metrics.json` の sha256（key の `metrics_sha256`）、answer key の sha256（metrics の `answer_key_sha256`）を
    確かめてから、2 行だけを点と「機械: <規則> <数値>」で埋める。ほかの byte は変えない（元は空の行なので人の入力は失われない）。もう一度流しても同じ byte。
    `--out` に id ごとの方式・規則・反復ごとの数値を書く。
  - `check-scored <md>`: check-human に加え、2 軸が 0〜2 の点と「機械:」の根拠で埋まっている。
  - `tally`: 開封・表・合格線の判定。`--metrics` を受けない（wrong_pivots による上書きは machine-axes に移した）。表の 2 軸の見出しに「（機械）」を付ける。
- close-audit の手順（`ART=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts`）:
  `check-human $ART/pilot-scoring.md` → `machine-axes --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json --metrics $ART/pilot-metrics/metrics.json
  --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json --out $ART/pilot-machine-axes.json` → `check-scored $ART/pilot-scoring.md` →
  `tally --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json --out $ART/pilot-tally.json`。WU pilot-materials の進捗に書いた `tally … --metrics …` は使えない。
- 子 9 で同じ分担を使うかは人の確認を待つ（子 8 の進捗の「提案」）。

##### D5 付記 2（2026-11-27、WU scorer-graded。人の決定 2026-11-27）: 4 軸の採点者は人が決めた model、検査は check-graded

- 人の決定: 人 rmaeda が 2026-11-27 10:3xZ に operator（`external_operator:claude-code-ops-session`）の対話 session で、4 軸を自分で採点せず
  Fable の model に盲検で付けさせると決めた。operator は `claude-fable-5-1`（5.2 はこの環境に無い）の subagent を 1 回だけ走らせた（wall 335.7 s。
  入力は採点 Artifact と同じ `data.json` だけ〈sha256 `2899d50e5022ba8f26f0de3ea513948ba715addd8ef8af072d6af9a6422ead9e`〉、tool は data.json の Read 2・出力の Write 6）。
  4 軸の点と根拠を root の artifacts の `pilot-scoring.md` に書き写した（書き写しの前の sha256 `fa074884b9113ddcf564a93d32718e8119d82b66377d4ebc1359e30d456754d9`、
  後 `02f9b38b27c9e1ef9d349cc3a34fadc4ff224be86fb094d65cdc577a872a5d30`）。採点者は `model:claude-fable-5-1`。
  記録は `agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json`（scorer・axes・decided_by `human:rmaeda`・decided_at `2026-11-27`・relayed_by・input・書き写しの前後の sha256・wall と tool の数・review_minutes_note）。
- D5 の「採点者は人（rubric.md の手順 1〜5）」と付記 1 の「人は 4 軸を付ける」を次に替える: 4 軸の採点者は人（`human:<名>`）か、人の決定の記録がある model（`model:<id>`）。
  実装者・CoS・operator は採点しない（operator は model を走らせて書き写しただけで、点を付けていない）。rubric.md の「採点の手順」1・3 をこれに合わせて直した。
  「軸と基準」から末尾まで（合格線・史実の参照の点）は変えていない。
- 検査の口: 付記 1 の `check-human` を `check-graded` に改名した（別名は残さない）。採点者の規則だけが変わり、ほかの規則（4 軸の点と根拠、機械の 2 行、レビュー時間が数）は同じ。
  - `human:<名>` は決定無しで通る。
  - `model:<id>` は `--decision <json>` の決定の `scorer` が同じ id で、`decided_by` が `human:<名>`、`decided_at` が YYYY-MM-DD、`axes` が採点者の 4 軸と同じときだけ通る。
  - ほかの採点者（`operator:` など、名の無い `human:`・`model:`）は決定があっても通らない。
  - `check-scored`・`machine-axes`・`tally` も `--decision` を受け、同じ規則を当てる。`tally` は model の採点者の行のレビュー時間（分）に「model の wall・人の時間ではない」と添え、
    json の採点者ごとに `kind`（human・model）と、model なら `minutes_note`・`decided_by`・`decided_at` を書く。
  - `blind` が作る材料の「この文書について」の採点者の文を、この規則に合わせた（今の材料は作り直さない。埋め込んだ「採点の手順」節は有無だけを見る）。
- close-audit の手順は付記 1 のものを次に替える（`ART=/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts`、`DEC=agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json`）:
  `check-graded $ART/pilot-scoring.md --decision $DEC` → `machine-axes --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json --metrics $ART/pilot-metrics/metrics.json
  --answer-key agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json --decision $DEC --out $ART/pilot-machine-axes.json` →
  `check-scored $ART/pilot-scoring.md --decision $DEC` → `tally --scoring $ART/pilot-scoring.md --key $ART/pilot-key.json --decision $DEC --out $ART/pilot-tally.json`。
- 限界（close-audit が未確認・限界として報告に書く）:
  - 4 軸は人ではなく model の採点である。人の独立の採点は無い。
  - 評価器（`claude-sonnet-5-5`）を同じ provider の model（Fable）が採点した。同じ provider の model の癖を共有している可能性を除けていない。
  - 採点欄のレビュー時間（分）0.93 は subagent の wall を 6 表で等分した値で、人のレビュー時間ではない。D6 の「人のレビュー時間」の指標は測れていない。
  - 採点者が data.json だけを読んだことは operator が transcript で確かめたもので、この WU は transcript を見ていない。
- 試験: D7 付記 1 の `test_check_human_*` は同じ assertion のまま `test_check_graded_*` に改名した。新しい `test_check_graded_*`
  （model の採点者は決定があるときだけ通る・決定の欄の誤りで落ちる・ほかの採点者は落ちる・`human:` は決定無しで通る・`--decision` が check-scored・machine-axes・tally に届く・
  commit した決定の記録が今の採点者を認める・材料の採点者の文）は改名の前の script で落ちる。

### D6: 指標

| 指標 | 定義 | 出し方 | 区分 |
|---|---|---|---|
| 親目標の達成 | 方式が親の Outcome の到達を正しく扱ったか | rubric の parent goal achievement（人）。補助: 評価の `axes.goal` と D6 の `goal_reached`（D8 の修正の後の入力）の食い違いの数 | 人の点 ＋ 決定的な数 |
| 情報の獲得 | 支持・反証・未確定の整理 | rubric の epistemic progress（人）。補助: `check_evaluation` を通った `learned`・`refuted` の件数（参照が slice の中にあるもの） | 人 ＋ 決定的 |
| 未達発見までの task 数 | 方式が初めて verdict `at_risk`・`off_track` で next が `continue` 以外を出した時点の、終端の子の数（replay は 2・4・6。どれでも出なければ史実の 8 と親の closeout）。研究ケースは節の数 | 記録から決定的に出す。answer key の最初の検知の時点との差を、採点の後に出す | 決定的 |
| 不要な Worker 実行とコスト | 検知の時点より後に史実で走った worker の run と USD（replay の全 events から数える。比較の側だけが読む）、と方式自身の LLM run・USD（`run.json`） | pilot-compare の script。研究ケースで数えられなければ未確認 | 決定的 |
| 誤った pivot | answer key で `on_track`・`unknown` の時点に `pivot`・`discriminating_experiment` を選んだ数。見逃し（answer key が continue 以外の時点に continue）も数える | 採点の後に比較の script | 決定的 |
| 根拠の無い主張 | 参照の無い主張・slice の外を指す主張 | `check_evaluation` の unsupported の数 ＋ rubric の unsupported claims（人） | 決定的 ＋ 人 |
| 認可と secret | 承認の外の run の数、本番の DB・daemon・config への接触、一時 DB の外の承認、出力の secret | `check-approval` の結果、driver の一時 home の記録、`task_core::strategy::contains_secret` を出力の file に当てた件数（Python に規則を写さず、task-api の試験の binary から呼ぶ） | 決定的 |
| 人のレビュー時間 | 採点に要した分 | 人が `pilot-scoring.md` の採点欄に方式ごとに書く | 人の申告 |
| 実装・試験の LoC | 子 8 の変更の production 全言語・Rust・TS・own_total_with_tests | `scripts/dev/source-size-report.py --exclusive-classes --rev <rev> --format json` を base（`git merge-base HEAD main`）から取り出して base と HEAD に当てる（CLAUDE.md） | 決定的 |

子 6 の条件 1 のため、方式 (b)・(c) の C-mid・D-done・F-done の verdict・next の answer key との一致（反復ごと）と、2 反復の間の一致も表にする。
子 6 の未確認は次で報告する: 独立の評価から pivot か識別実験が起き、v2 が子 5 の gate を通ったか（方式 (c) の D1.3）。評価器が file を読んだか（`reads.json` の件数）。

### D7: 合格線と rollout

- 合格線は `agent-docs/progress/2026-11-21-strategy-v1-0/rubric.md` の「合格線」のまま変えない（人の答え）。
- 合格を実証できなければ（採点が無い・合格線を満たさない・漏れの検査が落ちた・run が上限で足りない）、rollout しない。本番の controller の checkpoint の評価は mode `shadow` のままで、
  子 8 は mode を変えない。`enabled` にするのは人の手順（`docs/ops/project-controller.md` の「本番で有効にする人の手順」）で、子 8 の範囲の外である。
- canary は始めない。合格した後に検討するときも、人が認めた 1 案件と予算を質問で人に聞いてからにする。

##### D7 付記 1（2026-11-25、WU machine-axes。人の決定 2026-11-25）: 機械が付ける 2 軸の規則

規則は反復ごとに当て、id の点は反復の最小とする。rubric.md の「軸と基準」の 2/1/0 点の基準を次の数値に当てる。

- pivot の時点:
  - 検知できた時点: replay は answer key の `first_detectable`（**E-mid**）、研究ケースは D2.2 の反証の **11.20**（yield でも止まった時点）。
  - 検知: metrics の `first_detection`（compare `--pilot`: verdict が at_risk・off_track で next が continue 以外の最初の時点。方式 a は無し）。
    record の status が invalid の記録は検知に数えず、同じ定義（compare の `DETECT_VERDICTS` を import して使う）で後の時点を見る。
  - 時点の差: replay は answer key の `order`（A-mid 1・A-done 2・C-mid 3・C-done 4・D-mid 5・E-mid 6・D-done 7・E-done 8・F-mid 9・F-done 10・G-mid 11・G-done 12）、
    研究ケースは `research_case.py` の `POINTS`（11.15〜11.21）の順で、符号付きの差（検知 − 検知できた時点）。
  - 点: 差が 1 以下で 2（検知できた時点より前の検知は差が負で 2。rubric の「1 時点後までに」）、2〜3 で 1、4 以上か検知無しで 0。
    その反復の `wrong_pivots`（answer key で on_track・unknown の時点の pivot・discriminating_experiment）が 2 以上なら 0（合格線の節の規則。tally から移した）。
    研究ケースは answer key が無く metrics に `wrong_pivots` が無いので、誤った pivot は数えない。
  - この pilot の時点で取りうる差: replay は C-mid −3・D-done +1・F-done +4、研究ケースは 11.16 −4・11.17 −3・11.20 0。したがって 1 点は出ず、
    replay は C-mid か D-done の検知で 2、F-done の検知か検知無しで 0、研究ケースはどの時点の検知でも 2、検知無しで 0 になる。
- コスト:
  - replay: 比 = max((`unneeded_after_first_detectable` の worker run ＋ 方式の `llm_runs`) ÷ **137 run**, (同じ USD ＋ 方式の `usd`) ÷ **264.2525 USD**)。
    `unneeded_after_first_detectable` は E-mid の cutoff から検知（無ければ H の統合）までの史実の worker run と USD（list price）。
    基準の 137 run・264.2525 USD は metrics の `history`（E-mid の cutoff から H の統合まで。rubric の「137 run・264.25 USD」）から読む。
  - 点: 比が **25 %** 以下で 2、**75 %** 以下で 1、超えたら 0。人の採点・review の時間（分）は人が採点欄のレビュー時間に書く（点に入れない）。
  - 研究ケース: 史実の基準が無いので全方式 0（根拠に「史実の基準が無い・未確認」と書く）。方式の間の合計点の差には効かない。
- 合格線（rubric.md の「合格線」）は変えない。tally の合格線の判定は、採点の file の pivot の時点（機械の点）を使う。
- 試験: `scripts/dev/parent-eval-blind/test_pilot_score.py` の `test_check_human_*`・`test_machine_axes_*`（分担の前の script で落ちる）。

### D8: 評価器の入力の欠けを、測定の前に eval-input・eval-seed が直す

- 段 prep の WU eval-input が `review::parent_evaluation::request` の `goal.outcomes` を承認済み Roadmap の Outcome（`id`・`title`・`acceptance`・`reached`）で埋め、
  `reached` を D6 の読み口（`task_ops::project_controller_view::recorded_outcomes`、root ごとの判定は `task_core::project_proposal::root_satisfies`）から入れ、
  子の `criteria` を受け入れ条件と結果で埋め、`facts` の `goal_reached` を同じ D6 の値にする。`goal_reached` が真になる条件は子 6 の ADR の付記（WU eval-input）が決める。この ADR は条件を決めない。
- WU eval-seed が盲検の seed と live の入力に Roadmap の Outcome と子の criteria を入れる。
- pilot は段 prep の統合（eval-input・eval-seed を含む）の後の commit で流す。run の後の `prompt.md` に「（Outcome の記録は無い）」が出た run は、修正の前の入力とみなして採点から外す。
- 子 6 の 4 run（E-mid・D-done）は修正の前の入力なので、pilot の結果と合算しない（D2.1）。

### D9: 機密・HPC・canary・外部公開

- 研究ケースの本文（`c-results.md`・`c-manifest.json`・知識ベースの 3 頁）は repo に写さない。repo の試験は合成の testdata だけを使う。
- 研究ケースは内部の評価だけに使う（人の答え）。この ADR は、承認の json の harness・model で一時 daemon の reviewer・worker の run が読むことを内部の評価とする。
  ChatGPT・Muse・外部の MCP client には送らない。`pilot-scoring.md` は root の artifacts（ローカル）に置き、commit しない。
- HPC クラスタ（Sirius・Pegasus）での実験はしない（新しい測定は要らない）。canary・外部公開・本番昇格・publish はしない。本番の DB・daemon・config に書かない。

### D10: 評価器の prompt を answer key に合わせて直さない

- `change_rules::review::parent_checkpoint_section` の規則の文（E1〜E6・出力の形・axes の定義）に、answer key の値・時点の名・期待の組を足さない。
  eval-input が新しく埋める入力の欄を出すための変更は入力の変更で、これに当たらない。
- close-audit が `git diff $(git merge-base HEAD main) HEAD -- crates/task-worker/src/change_rules/review.rs` を読み、該当する変更が無いことを task の進捗に書く。

### D11: 段と WorkUnit

| 段 | WU | 役割 |
|---|---|---|
| prep | pilot-design | この ADR と進捗の骨組み（コード・script は変えない） |
| prep | eval-input・eval-seed | D8 の入力の修正と seed |
| harness | arm-driver | driver を方式 a/b/c に広げ、replay の 3 時点を stub で流す（D1・D3・D5 の id の振り方） |
| harness | research-driver | 研究ケースの 3 時点を合成の testdata の stub で流す（D2.2） |
| harness | pilot-compare | 指標・漏れ・secret の比較と盲検の材料の script（D4・D6） |
| （段の外） | operator | 人の承認の json（D3.2）の範囲で実 LLM の run を worker の外で流す |
| score | pilot-materials | operator の出力から `pilot-scoring.md` を root の artifacts に作る（D5） |
| （段の外） | 人 | rubric で採点する |
| close | close-audit・close-verify | 開封・集計・LoC・未確認・寄与を進捗と ADR に書き、検査を提出時の HEAD で再実行する |

### D12 付記（2026-11-27、WU close-audit）: 結果と rollout の判断

- 開封: close-audit が D5 付記 2 の順で流した。`check-graded --decision` exit 0（採点者は 6 表とも `model:claude-fable-5-1`、`human:` は 0 表）→ `pilot-key.json` の sha256 が
  `36841f6b189dbecbbb0217108f1118ab235c17c73fe800528ea2b6f8de64aace` で採点の file の値と一致 → `machine-axes` exit 0 → `check-scored --decision` exit 0 → `tally --decision` exit 0。
  4 軸の点は書き換えていない。機械の 2 軸は machine-axes だけが付けた（当て直すと同じ byte）。
- 開封の後は盲検が要らないので、D5 の「`pilot-key.json` を commit しない」を外し、同じ byte の写しを `agent-docs/progress/2026-11-24-strategy-v1-8/pilot-key.json` に置いた。
- 結果（4 軸は model、2 軸は機械。合計は a 5・b 10・c 11）:
  - 方式 b・c とも合格線を満たさない。両事例の条件は replay で偽（pivot の時点が 2 反復の最小で 0。b は unsupported claims 0、c は parent goal achievement 0）、研究ケースで真。a との差は b 5・c 6。失格の記録は無い。
  - replay では、方式 c の iter-1 が C-mid、方式 b の iter-2 が D-done で at_risk ＋ ask_human を出した（A〜H の完走の前）。もう一方の反復では検知が無かった。
  - 24 件の評価の記録で next の `pivot`・`discriminating_experiment` は 0 件、verdict `off_track` も 0 件。方式 c の v2 と子 5 の gate（D1.3）は起きなかった。
  - 方式 c の 12 点中 4 点が turn の上限（30）で invalid。D3.3 の上限と同じ値で流し、流し直していない。
  - 研究ケースでは方式 b・c が反証の 11.20 を `refuted` に記録したが、実験設計を直した版は出ていない。
- rollout の判断: 合格を実証できなかったので **rollout しない**（D7）。本番の controller の checkpoint の評価は mode `shadow` のまま。canary・HPC・外部公開・本番昇格・publish はしない。
  評価器の prompt（`change_rules`）は answer key に合わせて直していない（D10。`git diff $(git merge-base HEAD main) HEAD -- crates/task-worker/src/change_rules/` が空）。
- 限界（D5 付記 2 のとおり）: 4 軸は人ではなく model の採点で、評価器と同じ provider の model が採点した。レビュー時間 0.93 分は model の wall で人の時間ではない。
  採点者が data.json だけを読んだことは operator の申告で、close-audit は transcript を見ていない。
- 詳細（表・指標・LoC・未確認・寄与・提案）は task の進捗 `agent-docs/progress/2026-11-24-strategy-v1-8.md`。

## 足さないもの

- 新しい Goal の正本・Conversation DB・Manager・trait・状態機械・DB 表・migration・events の種類・Agent サービス・production の API・config の欄。
- 評価器の prompt の規則の文の変更（D10）。answer key を読む production の経路。
- 方式 (a) を LLM で流すための新しい review の口（D1.1。未確認に書く）。

## 実装前 brief

1. 根本原因: 子 6 の第二ゲートは、独立の評価器の質（誤 pivot・見逃し）を測れていない（2 時点 2 回・入力に Outcome が無い）。3 方式の比較の手順が無い。
2. 責務 owner: 比較の手順はこの ADR。入力の修正は `review::parent_evaluation`（eval-input）。driver・比較・材料は `scripts/dev/parent-eval-blind*` と task-api の試験の seed（後の段）。
3. 既存の再利用先: 子 6 の driver・`pilot_gate.py check-approval`・`parent-eval-blind-compare.py`・試験 `parent_checkpoint_blind_*`、子 0 の `replay.py`・`research_case.py`・rubric、子 2・5 の版と Critic と gate。
4. 選択肢: (i) 方式ごとに新しい評価の口を作る（不採用: production の口が増える）。(ii) 方式 (a) も LLM で局所の review を流し直す（不採用: slice に差分が無く別の入力になる。未確認に書く）。
   (iii) 子 6 の driver の seed と controller の mode だけで 3 方式を作る（**採用**）。
5. 新しい状態・分岐・fallback・port: 無し（driver の引数の方式は script の中の分岐で、production には無い）。
6. 撤去対象: 無し（この WU は文書だけ）。driver の時点の制限（E-mid・D-done だけ）と arms の名 `parent-eval-blind` は arm-driver が a/b/c に置き換え、旧い名を残さない。
7. 不変条件と確かめ方: D4 の否定対照（exit 1）、`parent_checkpoint_regression_shadow_no_reviewer_run`（方式 (a) の reviewer run 0）、`pilot_gate.py` の試験（承認の外の run を断る）、D10 の diff の確認。

## 守る不変条件

- 未来の情報と answer key は評価器の入力・作業 tree に入らない（D4）。
- 本番の DB・daemon・config・release に触れない。本番の controller の checkpoint は mode `shadow` のまま（D7）。
- 実 LLM の run は人の承認の範囲（48 run・25 USD・scope の arms と cases・non-production）の中だけで、worker の run の中では起こさない（D3）。
- 研究ケースの本文は repo に入らず、外部の LLM（ChatGPT・Muse）に送らない（D9）。
- 実装者・CoS・operator の自己評価を採点に入れない（D5）。

## 未確認

- 方式 (a) の局所の final review を同じモデルで流し直した結果（D1.1）。
- 方式 (c) の版を書く run・Critic・v2 の起票が一時 daemon で既存の入口のまま流れるか（arm-driver が確かめる）。
- 研究ケースの時点で checkpoint を 1 回だけ発火させられるか（research-driver が確かめる）。
- 版を書く run・Critic の 1 run の費用（D3.3 の 0.5 USD は上側の見積り）。
- 盲検は名前だけを伏せる。内容から方式を推測できるかは採点の後に人に聞く（D5）。
- 研究ケースの不要な実行（HPC の job）を検知の時点から数えられるか（D6）。

## 人の答え（2026-11-24、記録済み）

- 計画 v2 の段（prep・harness・operator の実 LLM・score・人の採点・close）を人が承認した。
- 実 LLM の pilot: 2 反復・最大 48 run・25 USD。operator が worker の外で流す。
- 研究ケース: 内部の評価だけに使う。ChatGPT・Muse に送らない。
- 合格線: rubric.md のまま。
- 評価器の prompt の文面は変えない。本番の controller は shadow のまま。HPC・canary・外部公開・本番昇格はしない。
