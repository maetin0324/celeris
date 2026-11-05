# ADR 2026-11-05: 第 4 フェーズ P4-B — 達成の後の根拠の失効・Roadmap の Outcome の checks・試行の上限からの根拠つき再開

- 状態: 採用（実装済み）。進捗は `agent-docs/progress/2026-11-05-phase4/p4b.md`、raw は `agent-docs/progress/2026-11-05-phase4/p4b/`。
- 前提: [P4-A の契約](2026-11-05-phase4-p4a-baseline-contract.md) の G1〜G3、[P0-B](2026-11-05-phase3-p0b.md)（Rust の数え直し・RecordOutcome・commit の CAS）、[P0-E](2026-11-05-phase3-p0e.md)（Outcome block の checks → root の `Check::Command`）。

## 決定
1. **wake の種類 `evidence_changed`（G1）**。案件の root（`is_root_task`）の event のうち、`Transitioned { from: done, to: 非終端 }`（reopen・retry）と
   `IntegrationReopened` を `evidence_changed:<task>:<event id>` の wake にする（`dispatcher/project_wake.rs` の `wakes_for_event`）。
   `project_wakes.kind` の CHECK に足すため migration 0115（`0115_project_wake_evidence_changed.sql`）で表を作り直す（行はそのまま写す）。
2. **achieved の案件の Rust の数え直し（G1）**。phase `achieved` で人の変更の wake が無く、根拠の失効の wake（`evidence_changed`・`review_failed`・
   `root_failed`、`project_guard::evidence_wake`）があれば、評価が `project_decider_ts::recheck_achievement` で snapshot と到達の記録から
   `unverified_outcomes` を数え直す（TS は呼ばない）。未達があれば guard `achievement-revoked` が効果なしで phase を `waiting` に戻し
   （規則 `pc/guard/achievement-revoked:<id,…>`、記録に `unverified`・`recorded`・`truncated`・wake の key）、`next_wake_after_secs = 0` で次の
   tick に判定（TS）へ回す。根拠が残っていれば従来の `achieved-quiet`。数え直しが読めなければ lease を放して判断しない（fail closed）。
   cron・新しい root の done などの wake では数え直さない（P0-B より前に achieved になった案件を動かさない）。
3. **旧い達成の記録は消さない**。`pc/achieved` の判断の行、Roadmap の `reached` の記録、適用済みの `mark_milestone` の行は残す。再達成は
   P0-B と同じ `verify_achievement`（到達の記録 ＋ I3 を満たす root）で決まる。到達の記録は Outcome ごとに 1 回のまま（二重の記録 0）。
4. **人の Roadmap の改版**は従来どおり人の承認（`post_decide`、`requires_human`）で `human_epoch` が上がり、人の変更の wake で TS が評価し、
   `wait(achieved)` は `verify_achievement` が数え直す。acceptance を変えた Outcome は旧い root が文字どおりの条件を持たないので未達になる。
5. **Roadmap の Outcome の `checks`（G2）**。`RoadmapOutcomeSpec.checks: Vec<OutcomeCheck>`（`Project.request` の Outcome block と同じ形・同じ検証:
   8 件まで、空・4096 byte 超の command は `roadmap_invalid`）。`snapshot_outcomes` が snapshot の Outcome に写し、`launch_root_spec`（P0-E）が
   controller の root の `Check::Command` にする。checks は版の一部なので、変えるには提案と人の承認が要る（CoS は提案だけ）。checks が空の版の
   JSON には欄を出さない（既存の版の行と同じ形）。
6. **試行の上限からの根拠つき再開（G3・C10）**。TS の問い `attempts_exhausted:<o>` で `blocked` の案件は、`o` を I3（`root_satisfies`）で満たす root が
   案件にあれば再開する（`project_resume`、根拠 `{"reason":"outcome_evidence","roots":[…]}`）。人が CoS に指示して作った root が合格・取り込み済みに
   なった場合に当たる。再開は評価を 1 回起こすだけで、起票の許可・予算の門は通常どおり掛かる。

## G3 の判定（replay v2 の new 腕）
- **C06（`questions ["root_failed:o2:1"]`）は妥当な停止**。o2 は oracle では `required: false` で、人が公開を却下（`approval: reject`）したので
  o2:1 は cancelled。controller には Outcome を任意にする欄が無く、取り下げは人の Roadmap の版（`dropped: true`）か依頼文の変更でしかできない
  （人の認可の原則）。controller が人に聞いて止まるのは正しい。oracle の pass と `controller_achieved` の違いは bench の「任意の Outcome」の扱いの差。
- **C10（`questions ["attempts_exhausted:o1"]`）は不具合だった**。人は controller の問いではなく CoS への指示で答え、CoS の root o1:3 が合格したが、
  P1-G の再開は問いの key の許可・却下・依頼文の変更しか見ず、controller は `blocked` のまま o1 の到達を記録しなかった。決定 6 で直し、
  replay の raw は C10 new の `controller_achieved` だけが false → true に変わった（他 39 試行・grade・contract は不変）。

## 採らなかった案
- **すべての wake で achieved を数え直す**: 既存の replay（`project_controller_replay_achieved_stays_quiet`）が示すとおり、P0-B より前の達成
  （到達の記録が無い）を cron で戻してしまう。根拠の失効の wake に限った。
- **失効のときに Roadmap に `in_progress` の状態を追記する**: TS は `mark_milestone` を案件で 1 回しか出さない（過去の判断を見る）ため、記録を
  下げると再達成で記録を出し直せず、`achieved-unverified` の 60 秒の繰り返しになる。記録は残し、根拠（root）の数え直しで決める。
- **`root_satisfies` で Outcome の checks の command を root に要求する**: TS の `satisfies` は checks を見ない（snapshot に出さない）ので、
  食い違いが `achieved-unverified` の繰り返しになる。P0-E と同じく checks は root の `Check::Command`（落ちれば `reviewer_passed` が偽）で効かせる。

## 旧い仕組みの扱い
- guard の「achieved は人の変更の wake でだけ評価し、それ以外は常に畳む」を、根拠の失効の wake では数え直す形に置き換えた。
- `snapshot_outcomes` の「Roadmap の Outcome は検査を持たない」を消した。

## 未解決
- 人が既存の Outcome の **checks だけ**を変えた版を承認しても、旧い root（outcome_ids で対応）は根拠のまま数えられ、achieved は戻らない。
  TS に checks の有無を渡す snapshot の欄が要る（提案）。
- 再達成の後も Roadmap の `reached` の evidence は最初の root を指す（新しい root は判断の行の根拠と snapshot にだけ出る）。
- snapshot が上限で切られた（`truncated`）ときは根拠の root が欠けうるので、数え直しは未達側に倒す（誤 achieved 0 を優先）。
