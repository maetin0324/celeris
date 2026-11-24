---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: eval-seed
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 8 eval-seed: 盲検の seed と live の入力に承認済み Roadmap の Outcome と子の criteria を入れる

## 結論
- `crates/task-api/tests/parent_checkpoint_live.rs` の seed を関数 `seed` に切り出し、盲検の seed（`parent_checkpoint_blind_seed`）と試験の中の request の写しが同じ関数で一時 DB に案件を置くようにした。
  - 承認済み Roadmap（版 1、Outcome `arch-v4` の 1 件）: title は slice の goal.md の「目的：」の最初の文、acceptance は「共通受入条件：」の 6 文（。で区切った文字どおり）。子 0 の replay の問い（`replay.py` の QUESTION）が親の目的を「goal.md の共通受入条件 (1)〜(6)」と定めているのに合わせた。提案と承認は store の公開の関数 `roadmap_propose_tx`・`roadmap_decide_tx`（actor `human:blind-seed`。一時 DB の中の段取りで、人の承認ではない）。
  - Outcome の status と evidence は cutoff 以前の記録だけから決めた: slice に親（P）の到達の記録（done への遷移・`ReviewerPassed`）は無い（P の行は created と ready だけ）。そのため Outcome の event を書かず、status は承認の時点の open、evidence は空、D6 の到達は偽。seed は P に到達の記録があれば落ちる（assert）。
  - 子の受け入れ条件: cutoff 以前の最後の `reviewer_passed` の `criteria`（`Check::Reviewer` の条件の index）と、判定の理由が `cmd=`（`Check::Command`。cmd と expected は理由から読む）・`path=`（`Check::ArtifactExists`）で始まる index を自分の条件とし、0 から隙間なく並ぶときだけ置く。本文は「（条件の本文は slice に無い）」。結果（pass・fail・unknown）は seed した `ReviewVerdict` の events から、本番と同じ `task_ops::project_snapshot::acceptance_criteria` が決める。
  - 子の events に `ReviewVerdict` を足し、`ReviewerPassed` の `criteria` を slice の値にした（前は空）。
- 写しの `goal.outcomes` は、seed した一時 DB から dispatcher の `review::parent_evaluation::goal` と同じ D6 の読み口（`task_ops::project_controller_view::reach`・`pp::unverified_outcomes`）で、子の `criteria` は `acceptance_criteria` で読む。
- `PARENT_CHECKPOINT_LIVE_WRITE=1` で E-mid・D-done の `request.json`・`prompt.md` を書き直した。両方の prompt から「（Outcome の記録は無い）」が消え、Outcome の行（`reached=false`）と acceptance 6 行、done の子の criteria の行が出る。
  - E-mid: A（5 条件: reviewer 3・command 1・artifact_exists 1）・B（6）・C（6）。D（reviewing）と E（ready）は review の記録が無いので criteria を持たない。
  - D-done: A・B・C に加えて D（6）。
- 子 6 の実 LLM の記録 `fixtures/parent_checkpoint_live/blind/` と slice `fixtures/parent_checkpoint_shadow/`（`reports.json` を含む）は差分 0。`crates/*/src` は差分 0。

## 実装前 brief
1. 根本原因: 盲検の seed が承認済み Roadmap も子の受け入れ条件も置かず、試験の中の request の写しは `goal.outcomes` と `criteria` を `Vec::new()` で組み立てていた。症状は E-mid・D-done の prompt の「（Outcome の記録は無い）」。eval-input で dispatcher の `request` を直しても、seed した DB に Outcome が無いので盲検の入力は空のまま。
2. 責務 owner: 盲検の入力を作る seed（`parent_checkpoint_live.rs` の `parent_checkpoint_blind_seed`）と写し（同 file の `request`）。到達と criteria の規則の owner は eval-input で直した `task_ops` の関数（変えない）。
3. 既存の再利用先: store の公開の関数（`project_create`・`roadmap_propose_tx`・`roadmap_decide_tx`・`create_task`・`append_event`・`cos_operation_apply_at`）、D6 の読み口 `reach`、`unverified_outcomes`、`acceptance_criteria`、既存の `slice`・`worker_finished`。
4. 選択肢: (a) 写しで Outcome と criteria を JSON から直に組み立てる → seed と写しの出どころが 2 つになり、D6 を通らないので不採用。(b) seed を関数にし、写しが一時 DB に同じ seed を置いて本番の読み口で読む → 採用。子の条件の本文を goal.md の子 A〜H の説明文で埋める案は、受け入れ条件でない文を条件として作ることになるので不採用。
5. 新しい状態・分岐・fallback・port: 無し（試験の file の中の関数 `seed`・`seeded_inputs`・`outcome_spec`・`child_criteria`・`child_events`・`rows_of`・`status_of` と型 `Seeded` だけ）。
6. 撤去対象: 写しの `criteria: Vec::new()`・`outcomes: Vec::new()`、seed の試験の本体にあった組み立て（`seed` に移した）、`ReviewerPassed` の空の `criteria`。
7. 不変条件と試験: 下の節。

## 守る不変条件と、既存の試験で検出できない理由
- 盲検の seed は承認済み Roadmap の Outcome（goal.md の共通受入条件の文字どおり）を置き、status・evidence は cutoff 以前の記録だけから決める。
- 子の受け入れ条件は cutoff 以前の review の記録から置き、slice に無い本文は作らない。
- E-mid・D-done の request・prompt に Outcome と criteria が出て、「（Outcome の記録は無い）」が出ない。
- Outcome・criteria を含む入力に、cutoff より後の値（時刻・報告の本文・未来にだけ現れる語）と answer key の印が入らない。
- 既存の試験で検出できない理由: 前の写しは `goal.outcomes`・`criteria` を空で組み立て、commit した fixture（空の入力）と byte で比べるだけだった。seed は Roadmap も条件も置かず、どの試験も「入力に Outcome が無いこと」を見なかった。否定対照 `parent_checkpoint_blind_no_answer_key_or_future` は時点より後の時刻と 40 字を超える報告の本文だけを見ていて、Outcome の文に入りうる最終の値（`17 → 17`・`+919` など）を見なかった。

## 試験（`crates/task-api/tests/parent_checkpoint_live.rs`）
- 新規 `parent_checkpoint_blind_request_has_outcome_and_criteria`: commit した E-mid・D-done の prompt に「（Outcome の記録は無い）」が無い、`- outcome \`arch-v4\`: …（reached=false）` と goal.md の文と一致する acceptance 6 行がある、request の Outcome が 1 件で reached が偽、cutoff 以前に `reviewer_passed` のある子だけが criteria を持ち全部 pass、A の条件の種類が reviewer 3・command・artifact_exists。
- 強めた否定対照 `parent_checkpoint_blind_no_answer_key_or_future`: 子 0 の replay の sentinels（`replay/sentinels.json`、15 語）のうち `first_seen` が時点より後の語が request・prompt・reports に無いことを足した（answer key の印・理由・期待の組・時点より後の時刻と本文の検査は前のまま）。
- `parent_checkpoint_live_request_renders`: 写しの出力と commit した request・prompt・reports が byte で一致（写しは `seed` の一時 DB から Outcome と criteria を読む）。

### 変更前の fixture で落ちた出力
試験の file を変えた後、fixture を書き直す前に実行した（`cargo nextest run -p task-api --test parent_checkpoint_live --no-fail-fast`、exit 100）。
```
FAIL parent_checkpoint_blind_request_has_outcome_and_criteria
  panicked at crates/task-api/tests/parent_checkpoint_live.rs:680:9:
  E-mid.prompt.md に （Outcome の記録は無い）
FAIL parent_checkpoint_live_request_renders
  panicked at crates/task-api/tests/parent_checkpoint_live.rs:611:9:
  assertion `left == right` failed: E-mid.request.json が本物の型の出力と違う
Summary [1.000s] 6 tests run: 4 passed, 2 failed, 3 skipped
```
強めた否定対照は、この時点（前の fixture）でも pass した。

### 書き直しの後
- `PARENT_CHECKPOINT_LIVE_WRITE=1 cargo nextest run -p task-api --test parent_checkpoint_live parent_checkpoint_live_request_renders` → 1 passed。書き換わったのは E-mid・D-done の `request.json`・`prompt.md` の 4 file だけ（`reports.json` は同じ byte）。
- `cargo nextest run -p task-api --test parent_checkpoint_live` → 6 tests run: 6 passed, 3 skipped（skipped は `#[ignore]` の seed・sample・blind live の 3 件）。
- 出力: `E-mid children_terminal=3 children_total=8 reports=17 report_bytes=5490 bytes_request=16613 bytes_prompt=17741`、`D-done children_terminal=4 children_total=8 reports=17 report_bytes=5600 bytes_request=17628 bytes_prompt=18218`。
- seed の経路: `PARENT_EVAL_BLIND_SEED=<spec>` で `--run-ignored only parent_checkpoint_blind_seed` を E-mid・D-done について一時 dir の DB に実行 → 両方 pass。DB の `project_roadmap_versions` に版 1（`approved`、`human:blind-seed`）がある。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web（typecheck+unit。task-api の変更のため）。nextest 6377 passed・31 skipped、web の vitest 108 files・773 tests passed、repo_health_gate: all checks passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo nextest run -p task-api --test parent_checkpoint_live` → 6 passed、3 skipped（`#[ignore]`）。
- fixture の check（plan の check と同じ command）: E-mid・D-done の prompt に「（Outcome の記録は無い）」が無い、D-done の prompt に `outcome \`` がある、`git diff --quiet $(git merge-base HEAD main) -- fixtures/parent_checkpoint_live/blind/ fixtures/parent_checkpoint_shadow/` → exit 0。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を plan の正規表現で絞る）→ 範囲の外 0 件。変えた path は `parent_checkpoint_live.rs`、E-mid・D-done の `request.json`・`prompt.md`、この進捗の 5 件。
- `git diff --stat "$CELERIS_WU_BASE" -- crates/task-dispatch/src crates/task-ops/src crates/task-core/src crates/task-worker/src crates/task-api/src` → 差分 0。

## LoC（`git diff --numstat "$CELERIS_WU_BASE"`）
- production: 0（`crates/*/src` の差分 0）。
- 試験: `crates/task-api/tests/parent_checkpoint_live.rs` +463・−137（`cargo fmt` の整形を含む）。
- fixture（生成）: `E-mid.request.json` +106・−4、`E-mid.prompt.md` +24・−1、`D-done.request.json` +138・−5、`D-done.prompt.md` +30・−1。
- 純増の理由: seed を関数にし、Outcome と子の条件を slice から決める関数と、写しが一時 DB から D6 で読む関数を足した。新しい Manager・trait・状態機械・表は無い。

## 未確認
- 子の受け入れ条件の本文は slice に無い（`replay.py` の export の `KEEP` が task の acceptance を落とす）。本文は作らず「（条件の本文は slice に無い）」と書いた。条件の数と種類は `reviewer_passed.criteria` と判定の理由の形から決めた。A の index 5（理由が `reviewer(…)` で `reviewer_passed.criteria` に無い取り込みの判定）と index 6 以降（workspace の検査・health）は自分の条件に数えていない。
- `reviewer_passed` の無い子（E-mid の D・E、D-done の E）は、条件の数と種類が slice から決まらないので criteria を持たない。
- 子の criteria の結果は seed した `ReviewVerdict` の今の試行の値で、評価器の run の時点で切らない（eval-input の付記 4 と同じ）。
- Outcome の title・acceptance は goal.md の文字どおり。v4 の親 task の受け入れ条件（task の `acceptance` の欄）そのものは slice に無い（`goal.json` は title・objective・created_at だけ）。
- driver（`scripts/dev/parent-eval-blind/driver.py` の `run`）は seed の後に、API で別の Roadmap（Outcome `o1`「循環の削減」）を `base_version: null` で提案して承認する。seed が版 1 を承認済みにしたので、この提案は Stale で拒まれ、driver はそこで止まる見込み（driver を流して確かめてはいない）。driver は WU の範囲（`crates/task-api/tests/parent_checkpoint_live*`・E-mid/D-done の fixture・進捗）の外なので変えていない。

## 申し送り（WU arm-driver）
- `driver.py` の `run` にある Roadmap の提案と承認（`/roadmap/proposals` と `/proposals/1/decide` の 2 回の `self.api`）を消す。承認済み Roadmap は seed が置く（旧い仕組みを残さない）。消さないと driver は Stale で落ち、残せば seed の Outcome と違う作った Outcome `o1` が評価器の入力に入る。

## 親の目的への寄与
- 改善 C・D（途中の親の独立評価）: 盲検の pilot の入力（seed と E-mid・D-done の fixture）に、承認済み Roadmap の Outcome（D6 の到達つき）と子の受け入れ条件の結果が入った（確認済み: 上の試験）。評価の質が上がるかは、子 8 の盲検の評価と人の rubric で測る（未確認）。

## commit
- strategy-v1 8 eval-seed: 盲検の seed と live の入力に承認済み Roadmap の Outcome と子の criteria を入れる
- strategy-v1 8 eval-seed: WU の進捗に提出時の検査の結果を書く
