# ADR 2026-11-05: 第 3 フェーズ — 達成判定の根拠の連鎖と再発防止の不変条件（[Project Controller](2026-11-04-project-controller.md)・[long-horizon ベンチ](2026-11-04-long-horizon-bench.md)・[A9 pilot の不具合](2026-11-05-a9-llm-pilot-findings.md) への付記）

- 状態: 採用（設計）。この ADR は判定の規則と試験のケースを固定する。実装は第 3 フェーズの兄弟 task（親 01M6N7WD1WXXW0A5FV7RVZ67B7 の子 P0-B〜P2-J）が受け持つ。
- 根拠: 事実の照合と齟齬の表は `agent-docs/progress/2026-11-05-phase3/p0a.md`。コードの位置は main（`git merge-base HEAD main`）で確かめた。
- 前提として保つもの: 旧 milestones API は 410。Project は長期目的、Task は終わらせるもの。Rust が状態・検証・安全・認可を持ち、TS は提案だけを出す。A9 の修正（`root_row` の `IntegrationMerged` 限定、決定的な検査の優先、pilot の git 化、effect stub、公開手順）は再実装しない。

## 結論
2026-11-05 の実 LLM pilot（C06・C08 の各 1 試行）で controller が achieved、独立 oracle が 0/2 になった直接の原因は A9 付記 D1〜D4 で扱った。残る構造上の原因は 1 つで、**「達成」を主張する 6 つの層（controller の phase・Roadmap の Outcome・成果の根拠・oracle・Reviewer・check）が、互いを確かめずに別々の根拠で真になる**ことである。この ADR は、上位の層が真になる条件を下位の層の検証済みの事実に限る不変条件 I1〜I14 と、それを固定する試験のケース K1〜K43 を定める。

## 根拠の連鎖（main の現状）
上から順に、各層が何を根拠に真になるか。

| 層 | 真になる条件（main） | 場所 |
|---|---|---|
| L1 controller の phase `achieved` | TS が `wait{phase:"achieved"}` を返し、`validate` を通れば Rust は phase を `Achieved` にする。`validate` の `achieved_with_open_outcomes` は **TS が申告した `gap.open`・`gap.unknown` が空か**だけを見て、各 Outcome が根拠の root を持つかを Rust で数え直さない（`gap` が全 Outcome を覆うかも見ない）。TS を書き換えれば迂回できる | `task-core/src/project_proposal.rs` の `validate`、`project_decider_ts.rs` の `Wait` の分岐 |
| L2 Roadmap の Outcome `reached` | 人の `POST …/roadmap/outcomes/{oid}/status` だけが書く（`check_status` → `record_status_tx`）。controller の `mark_milestone` は `validate` で根拠の root を `root_satisfies` で確かめる（`milestone_evidence_unknown`）が、効果が無い（`next_wake_after_secs = 1` だけ）。判断の行に残るだけで Roadmap の状態は変わらない | `task-ops/src/roadmap.rs`、`project_decider_ts.rs` の `MarkMilestone` |
| L3 成果の根拠 | root が done・`reviewer_passed`・`integrated` で、`outcome_ids` に Outcome を名指す（controller が起票時に自分で書く）か acceptance を文字どおり含む | TS の `satisfies`、Rust の `root_satisfies` |
| L4 oracle | `out/<id>.txt` の sha256 が `hidden/expected.json` と一致（`bench_version` 1） | `crates/campaign/src/long_horizon.rs` の `outcome_reached` |
| L5 Reviewer | LLM の verdict。stub reviewer は全基準を無条件に pass。pilot の root の acceptance は Reviewer だけ | `review_verdict.rs`、`stub_harness.py` |
| L6 check | `Check::Command` は reviewer より先に走り、1 つでも落ちれば done にならない（A9 review-check で確認済み）。pilot の root は command check を持たない | `crates/task-dispatch/src/review.rs` |

L1 は L3 だけを見て、L3 は L5・L6 の結果（done）を見る。L6 が空なら L5 の LLM の判断だけで L1 まで真になる。L2 は L1 と独立で、controller が achieved でも Roadmap の Outcome は未記録のまま残る。L4 は L1〜L6 のどれとも別の根拠で、L1 と L4 の不一致を誰も検出しない。

## 決定（不変条件）
各条件の後ろの括弧は担当の兄弟 task。

- **I1（phase は Rust が確かめて書く）**: phase `achieved` は、Rust が TS の申告した `gap` を使わずに snapshot から数え直し、同じ transaction の中で、達成の対象（approved の Roadmap の版があればその生きている Outcome 全部、無ければ依頼文の Outcome block 全部）がすべて I3 を満たす根拠で `reached` と記録済みであることを確かめたときだけ書く。TS の `wait{phase:"achieved"}` は要求であり、満たさなければ phase は `waiting` のまま、判断の行に `pc/achieved-unverified` と未達の Outcome の id を残す。（P0-B）
- **I2（mark_milestone は Roadmap に効く）**: controller の `mark_milestone` の効果は、効果の transaction の中で `check_status` と同じ検査を行い `record_status_tx` で書く。検査に落ちれば状態を書かず、理由付きで判断の行に残す（fail closed）。Roadmap が無い案件の到達は判断の行（`outcome_id` と根拠）を正本とする。（P0-B）
- **I3（到達の根拠）**: Outcome の `reached` の根拠になる root は、(a) この案件の root、(b) 最後の終端遷移が `review_pass` による done（reopen の前の合格を持ち越さない）、(c) 最後の review で全 `Check::Command` が合格、(d) 案件に repo があれば最後の `IntegrationMerged` の後に `IntegrationReopened` が無い、(e) 対象の Outcome を起票時に名指したか acceptance を全部含む、の全部を満たすものに限る。（P0-B）
- **I4（repo の無い案件）**: repo の無い調査案件は (d) を問わない代わりに、根拠の root が成果物（artifact または KB の参照）を持つことを要する。具体の規則は P0-B が決めて試験で固定する。依頼の自然言語の受入条件に hidden の oracle を要求しない。（P0-B）
- **I5（oracle は独立）**: 採点の合否は、target branch の最終の木に対する決定的な検査 script の exit code だけで決まる。controller の主張・reviewer の判断・LLM の回答文を入力にしない。検査は公開の契約から導ける条件だけを見る。`bench_version` が facts と oracle で違えば採点を拒否する（既存）。（P0-C）
- **I6（版の分離）**: oracle・課題・公開契約のどれかを変えたら `bench_version` を上げる。旧版の raw は改変せず別の dir に置き、新旧を同じ表で混ぜない。（P0-C・P0-F）
- **I7（決定的な検査が優先）**: `Check::Command` が 1 つでも落ちれば、reviewer の verdict に関わらず done にも `reached` にもしない。食い違いは event に残す（既存。後退させない）。（P0-E が保つ）
- **I8（検証できない基準を通さない）**: stub reviewer は、自分で確かめられない基準を pass にしない。pilot の root は公開契約に基づく command check を 1 つ以上持つ。check の無い経路が保証しない範囲を文書に書く。（P0-E）
- **I9（判断の根拠を残す）**: reviewer の verdict の理由、command check の出典（acceptance の index と command）と exit code を event に残し、pilot の facts と ops に写す。（P0-E）
- **I10（効果の記録は worker の外）**: 外部効果の記録は worker が書き換えられない場所に置き、facts はその記録から読む。記録が読めなければ効果 0 件とせず `observation_failed` にする。承認は daemon の承認 API を通し、承認前の効果は 0 件、rejected の効果も 0 件。同じ logical_id の効果は、crash の後の再実行・重複の呼び出し・並行の呼び出しのどれでも 1 件以下。（P0-D）
- **I11（二重の状態変更 0）**: Roadmap の版の差し替え、同じ event の重複、root の reopen、並行の承認・CAS、daemon の再起動のどれでも、Outcome の状態と phase の書き込みは 1 回以下。wake の側でも、同じ event からの wake は 1 件（`project_wakes` の `UNIQUE (project_id, wake_key)`）で、1 件の wake から効果は 1 回以下。人の編集（brief・authority・Roadmap）と wake は同じ transaction で書く。（P0-B・P1-G）
- **I12（一致率を測る）**: pilot の報告は試行ごとに「controller の achieved」と「oracle の合否」を並べ、不一致を不具合として数える（LLM の失敗に数えない）。（P0-C・P0-F）
- **I13（人の停止は自動で解けない）**: 再評価で動いてよいのは、人の回答・許可の根拠があるか、止まった原因が解消した場合だけ。人の明示の pause・cancel、未解決の `human_required`、危険な予算超過は、時間の経過や wake では解けない。却下された問いは、同じ分類の効果を作らず、同じ問いを再び聞かない。（P1-G）
- **I14（予算と計測は chat run も数える）**: 案件の予算の token と計測は、案件 CoS の chat run の token と起動を含む。main の `project_run_usage_since` は `runs` だけを読み、`chat_runs` を読まない（A6 の未解決 5）。日次予算の超過は翌日に再評価できるものとできないものに分類し、永続の許可なしに root を増やさない。（P1-G）

## 試験のケース（K）
実装 task は、まず下の陰性ケースを赤で確かめてから直す。試験名は実装 task が決め、各進捗の「受入とテストの対応」に書く。

| K | 入力 | 期待 | 条件 | 担当 |
|---|---|---|---|---|
| K1 | TS が achieved を返すが、1 つの Outcome の root が未取り込み | phase は waiting、`pc/achieved-unverified` | I1 | P0-B |
| K1b | 改変した TS が `gap.open=[]`・`gap.reached` に全 id を入れ、実際は根拠の root が無い Outcome がある | Rust が拒否し、phase は waiting（TS の改変で迂回できない） | I1 | P0-B |
| K2 | root が無い Outcome がある | achieved にならない | I1 | P0-B |
| K3 | reviewer だけ pass、command check fail | done にならず reached にならない | I3・I7 | P0-B・P0-E |
| K4 | 違う案件の root を根拠に指す | `roadmap_evidence_unknown`、状態を書かない | I3 | P0-B |
| K5 | Outcome 3 件のうち 2 件だけ到達 | phase は waiting | I1 | P0-B |
| K6 | 全 Outcome が I3 を満たす | mark_milestone ごとに reached が 1 件ずつ記録され、最後に achieved | I1・I2 | P0-B |
| K7 | approved の版を差し替え、Outcome が 1 件増える | achieved が waiting に戻る（人の変更の wake で） | I1・I11 | P0-B |
| K8 | 同じ `IntegrationMerged`・`ReviewerPassed` が重複して届く | reached の記録は 1 件 | I11 | P0-B |
| K9 | done の root を reopen し、2 回目の review が fail | 前の合格を根拠に使わない | I3(b) | P0-B |
| K10 | 2 つの承認と controller の判定が同じ revision で競合 | CAS で 1 つだけ通る | I11 | P0-B |
| K11 | mark_milestone の効果の途中で daemon を止めて再起動 | 状態の記録は 0 件か 1 件 | I11 | P0-B |
| K12 | repo 付き案件で `NoCommits`・`NotGit`・`IntegrationNotApplicable` | integrated は偽（既存の `root_row_` 5 件を保つ） | I3(d) | P0-B が保つ |
| K13 | 公開契約を満たす別の表現（語順・空白・追記行） | oracle は合格 | I5 | P0-C |
| K14 | 公開契約を満たさない出力・空の木 | oracle は不合格 | I5 | P0-C |
| K15 | worker の作業場所に hidden の script や値が無い | leakage の否定試験が合格 | I5 | P0-C |
| K16 | v1 の raw（`a9/llm/`）を v2 の oracle で採点しようとする | `bench_version` の不一致で拒否 | I6 | P0-C |
| K17 | 承認前に publish を呼ぶ | 効果 0、`effect_before(approval)` が fail | I10 | P0-D |
| K18 | approve → publish を 2 回 | 効果は exactly once | I10・I11 | P0-D |
| K19 | worker が効果の記録 file を書き換えようとする | 書けない（sandbox の外） | I10 | P0-D |
| K20 | 効果の記録が読めない | facts を作らず `observation_failed` | I10 | P0-D |
| K21 | stub reviewer に検証できない基準を渡す | pass にしない | I8 | P0-E |
| K22 | reviewer の理由・check の exit code | event・facts・ops に残る | I9 | P0-E |
| K23 | 同じ facts で controller が achieved、oracle が fail | 報告の不一致件数が 1 | I12 | P0-C |
| K24 | 旧 milestones API を呼ぶ | 410 のまま（既存 `milestone_lifecycle_endpoints_are_gone`） | 前提 | 全員 |
| K25 | daemon の承認 API で reject してから publish | 効果 0、facts に reject の記録が残る | I10 | P0-D |
| K26 | 効果の記録の前後で publish の process を落とし、同じ logical_id で再実行 | 効果は最後に 1 件（記録の前に落ちた場合は再実行で 1 件、記録の後に落ちた場合も 1 件のまま）。承認前の crash は 0 件 | I10・I11 | P0-D |
| K27 | 同じ logical_id の publish を並行で 2 つ走らせる（時間差は試験の hook で決定的に作る） | 効果は 1 件。main の `pilot_effect.py` は記録を読んでから追記し、lock が無いので、この試験は修正前に赤になるはず（赤は未確認） | I10・I11 | P0-D |
| K28 | 実 LLM と同じ config harness の経路で worker が publish の command を使う（LLM は呼ばず、harness は stub） | 公開された command が worker に届き、効果の記録まで通る。既存の stub 配線も通る | I10 | P0-D |
| K29 | wake の event を取り込む途中と効果の途中で daemon を止めて再起動（既存 `project_wake_crash_*`・`project_controller_replay_crash_each_stage` に brief の wake を足す） | wake は 1 件、効果は 1 回以下 | I11 | P1-G |
| K30 | 同じ event を 2 回取り込む | `wake_key` が同じで wake は 1 件、評価は 1 回 | I11 | P1-G |
| K31 | 時計を 3 日飛ばす（既存 `project_controller_replay_suspend_clock_jump_3days`） | 溜まった wake で効果が重ならない、問いも重ならない | I11 | P1-G |
| K32 | 同じ失敗を上限より多く起こす（既存 `project_controller_replay_worker_fails_same_way_5x`） | `max_same_failure_repeats` で止まり、問いは 1 件、root は増えない。人が解消した後にまた同じ失敗なら、もう一度止まる | I11・I13 | P1-G |
| K33 | brief と authority を同じ tick で人が編集する（main では brief・authority・Roadmap の編集は wake を書かない） | wake は編集ごとに編集と同じ transaction で 1 件。評価は最新の revision を読み、古い revision の効果は CAS で 1 つも通らない | I11 | P1-G |
| K34 | clear: 人が問いに許可で答える（`authority_answered`）か、止まった原因が解消する | 再評価が 1 回走り、許可の範囲でだけ動く。根拠が判断の行に残る | I11・I13 | P1-G |
| K35 | deny: 人が問いを却下する（`authority_question_dismissed`）か、許可を取り消す（既存 `authority_ledger_revoked_after_decision_creates_no_root`） | 再評価しても同じ分類の効果は 0。同じ問いを再び聞かない（既存 `authority_ledger_answered_question_is_not_asked_again`・`authority_ledger_same_question_not_asked_twice`） | I13 | P1-G |
| K36 | 7 日以上の仮想時刻を流す（既存 `project_controller_replay_external_never_replies` は 240 時間） | 上の K29〜K35 の不変条件が保たれ、wake・問い・効果の件数が上限内 | I11・I13 | P1-G |
| K37 | 人の pause・cancel、未解決の `human_required`、危険な予算超過で時間を流す（既存 `project_controller_replay_paused_project`・`..._cancelled_project`・`..._budget_day_cap`・`..._token_cap`、`authority_ledger_human_required_cannot_be_lifted`） | 自動で解けない。効果 0 | I13 | P1-G |
| K38 | 人の回答待ちだけの案件を仮想時刻で 24 時間流す | 評価の回数が修正前より減る。修正前の回数は同じ台本で実測して並べる（main は `ANSWERED_RECHECK_SECS`=3600） | I13 | P1-G |
| K39 | 案件 CoS の chat run の token で日次の上限を超える | `tokens` に chat run が入って guard が止まり、起動の回数が計測に出る。同じ通知・問いは 1 件 | I14 | P1-G |
| K40 | 日次予算の超過の翌日に時計を進める | 再評価できる分類だけ再評価する。永続の許可が無ければ root は増えない | I14 | P1-G |
| K41 | repo の無い調査案件で、root が成果物を持つ／持たない | 持てば `reached`、持たなければ reached にならない。hidden の oracle を要求しない | I4 | P0-B |
| K42 | v2 で全 10 課題の replay を再採点し、同じ初期状態と oracle で old と new を比べる | 陰性を含めて期待どおり。合否は LLM の回答を読まない | I5・I6 | P0-C |
| K43 | GET と UI で未達・達成・待機理由・承認範囲・次の wake を見る | `pc/achieved-unverified` と未達の Outcome の id が見える | I1 | P2-J |

## 理由
- L1 から L6 のどれか 1 層の誤りが、そのまま「達成」として外に出る構造が、A9 の 0/2 を作った。各層を個別に直しても（A9 の D1〜D4）、上位の層が下位を確かめなければ次の抜け道で同じことが起きる。上位を下位の検証済みの事実で縛るのが最小の手当てである。
- 判定を Rust に置くのは、TS の bundle を変えても I1〜I3 を迂回できないようにするため（共通受入 (b)）。TS は提案を出し続けてよく、Rust が拒否した理由が判断の行に残る。

## 採らなかった案
- **controller（TS）に oracle の script を走らせる**: controller が採点を再実装することになり、I5 の独立性が崩れる。D11 の snapshot だけを読む隔離にも反する。
- **phase `achieved` を廃止し、Roadmap の状態だけを見る**: Roadmap の無い案件の終わり方が無くなる。phase は残し、書く条件を絞る。
- **reviewer の prompt に「tree を確かめよ」と足すだけ**: 実 LLM の挙動に頼り、決定的に固定できない（A9 付記 D4 と同じ理由）。

## 旧い仕組みの扱い
- `project_decider_ts.rs` の `MarkMilestone` の「記録だけ（A5 の前）」の分岐は P0-B が消す（効果を持たせる）。`Wait{phase:Achieved}` を検査なしで写す分岐も消す。
- `docs/ops/project-controller.md` の「`not_git`・`no_commits` は取り込み済みと同じに扱う」の 2 行は A9 D1 で古くなっていたので、この ADR と同じ変更で消した。運用手順の全面の見直しは P2-J。
- 互換 flag・新旧の判定の併存は置かない（[破壊的変更を是とする方針](2026-10-11-breaking-changes-by-default.md)）。

## 語の読み方
P1-G の受入の「clear と deny」は、記録に定義が無い。この ADR は、人の回答の 2 種類として読む。clear は許可または原因の解消（K34）、deny は却下または許可の取り消し（K35）。P1-G が別の意味だと判断したら、`2026-11-05-phase3-p1g.md` に書いて直す。

## 未確認
- I3(b) の reopen の前の合格の持ち越し: `root_row` は `ReviewerPassed` で立てた `reviewer_passed` を戻さない。status が done でなければ `satisfies` は偽なので実害は status が `review_pass` 以外の理由で done になる経路に限るが、その経路が今あるかは確かめていない（K9 で P0-B が確かめる）。
- 実 LLM の reviewer が command check の結果を受けて振る舞いを変えるかは、P0-F の再試験まで分からない。
