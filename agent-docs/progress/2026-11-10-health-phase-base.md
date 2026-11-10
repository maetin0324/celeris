---
title: 段の統合の health が段の途中で取り込んだ review target の commit を段の変更として数えない
tasks: [01M72XBPY2PDZA1KFJ0FJWWY1N]
status: done
updated: 2026-11-10
---
# 段の統合の health が段の途中で取り込んだ review target の commit を段の変更として数えない

完了日: 2026-11-10

## 経緯
G4（task 01M6TYVJSXBZMSBC0YGAT2TTX0）の段 design の統合の health が、人の承認の後に main に入った policy の file
（`scripts/dev/repo-health/` の 11 file。子 task 01M72DBBE7AFCNXYWSB5M0YZW9）を段の変更として数え、`policy.changed` で
fail した。base が段の最初の unit の `base_commit`（G4 の計画を作った時の main）だったため。修復の unit では直せず、
D4.3 の往復で Blocked になった。

## 変更
- `ADR health-enforcement-loop D1.2 付記: 段の統合の health の base を review target の merge-base まで進める規則` —
  `agent-docs/adr/2026-11-08-health-enforcement-loop.md` の D1.2 に付記を足した（実装より先の commit）。段の基点 P と、
  統合の前の HEAD H と review target の merge-base M の関係、採った規則と退けた 2 案、target を解決できないときの扱い、
  WU の check と task の review の確認結果を書いた。
- `段の統合の health: 段の途中で取り込んだ review target の commit を段の変更として数えない（base を merge-base まで進める）` —
  `health_review/check_item.rs` に `phase_health_base`（P が M の祖先なら M、そうでなければ P。git の失敗・H を読めない・
  target を解決できないときは `Err`）と `Dispatcher::phase_review_target`（`review_target_ref` を呼ぶだけ。案件の repo の
  参照が無い作業場所は `Ok(None)`）を足し、`HealthTarget.base` を `Result<String, String>` にした。`Err` は検査器を起こさず
  `HealthGateOutcome::unavailable`（`health_review/gate.rs`。`health_gate` の Unavailable と同じ証跡の作り方を共有）で
  `health_check_error: facts_unreadable:phase_base: …` にする（合格にしない。D3.4 の Blocked）。旧い決め方（P、P が
  無ければ統合の前の HEAD をそのまま base にする `git_head`）は消した。試験 `health_enforce_phase_base_*` 5 件
  （`health_review/check_item_phase_base_tests.rs`）。
- `段の統合の health: base の決め方を Dispatcher::phase_health にまとめ、start_integration の行を増やさない` —
  `start_integration`（`dispatcher/phase_integration.rs`）の呼び出しを 1 行の `self.phase_health(…)` にした。最初の版は
  repo health の `size.responsibility-growth`（`start_integration` の行 205 → 210、dispatcher の module の行 +4）を
  悪化させたので直した。

## 確かめたこと（同じ問題が他の経路にあるか）
- WU の check の health: base は WU の `base_commit`（WU の branch を切った Task の branch の HEAD か依存先の WU の HEAD。
  `dispatcher/work_units.rs` の WU の作業場所の準備）。Celeris は WU の branch に target を取り込まないので
  `base_commit..WU の HEAD` は WU の worker の commit だけ。同じ問題は無く、変えていない。
- task の review の health: base は review 前の同期の後の `target_sha`、head は `reviewed_sha`（`health_review/review_item.rs`）。
  同期の後の head は `target_sha` を含むので、差分は task の変更だけ。同じ問題は無く、変えていない。
- 統合の修復の後の再統合: 同じ `start_integration` を通るので同じ規則になる。

## 試験（守る不変条件）
- `health_enforce_phase_base_target_commits_taken_mid_phase_do_not_count`: 段の途中で Task の branch を main の承認済みの
  policy の commit まで ff-only で進めても合格。検査器が受け取る `--base` は M、`--head` は統合の HEAD。同じ HEAD を
  base = P で評価すると `policy.changed` で落ちる（合格は base の決め方による）。
- `health_enforce_phase_base_worker_policy_change_still_fails`: 段の worker が policy の file を変えたら base = M でも
  `policy.changed` の Fail（I3）。指摘の path は worker の file だけ。
- `health_enforce_phase_base_worker_fail_grade_change_still_detected`: 段の worker の fail 級の変更（size）は base = M でも
  検出され、target 側の policy の変更は数えない。
- `health_enforce_phase_base_without_target_intake_keeps_phase_start`: target を取り込んでいない段（M が P より前）は
  base = P のまま。前の段で統合済みの policy の変更を数えない。
- `health_enforce_phase_base_unresolved_target_does_not_pass`: target を解決できない・target の ref で git が失敗した
  ときは検査器を起こさず `health_check_error: facts_unreadable:phase_base: …`（`stops_without_replan` に当たる）。
- 検査器は試験の repo の base に commit した stub（`git diff --name-only base head` から finding を作り、受け取った
  base・head を log に書く）。時間に依存せず、CPU を焼く負荷をかけない。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo nextest run -p task-dispatch health_enforce_phase` → exit 0（9 passed: 新しい 5 件と既存の
  `health_enforce_phase_integration_*` 4 件）
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
  3712 passed、11 skipped。check-health は verdict=pass、fail_grade=0、new=0、worsened=0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 本番への移行
- config の変更は無い。DB の migration・event の形の変更も無い（`IntegrationCheckFinished.health.base_sha` に入る値が
  変わるだけ）。この変更を含む release を promote すれば効く。
- promote の後に operator が見る値: G4（task 01M6TYVJSXBZMSBC0YGAT2TTX0）は Blocked のままなので、人が comment か回答で
  再開した後の段 design の統合で、`IntegrationCheckFinished`（cmd `celeris:repo-health/1`）の `health.base_sha` が
  `git merge-base <統合の前の G4 の branch の HEAD> main`（子 01M72DBBE7AFCNXYWSB5M0YZW9 の main への merge commit 以降の
  main の commit）になり、段の最初の unit adr の `base_commit` でなくなること。verdict が `policy.changed` で fail しない
  こと。
- 確かめ方: G4 の task の events（Web の task の画面の event、または `celerisctl` の task の events の表示）で、上の event の
  `health.base_sha` と `verdict` を読む。

## 未解決
- dispatcher の流れの試験（`start_integration` → `phase_health` → `review_target_ref`）は、案件の repo の参照の無い作業場所
  （target が `Ok(None)`、base = P）だけを通る（既存の `health_enforce_phase_integration_*`）。案件の repo を持つ task が
  段の途中で target を取り込む流れは、base の決め方の関数（`phase_target`・`evaluate`）の試験 5 件で確かめ、dispatcher の
  流れの試験は足していない。`review_target_ref` は review 前の同期と同じ関数で、その解決は既存の試験が確かめている。

## 提案
- policy だけの子 task の上陸の手順（親の branch を main の merge commit まで進める）は、この変更の後は段の統合の health を
  止めない。G3 ci-entry・G4 exc-expiry の計画で置いた「段を分ける」「人が段の統合を通す決定」の回避は、次の計画から要らない。
