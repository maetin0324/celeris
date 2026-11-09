---
title: 健全性v2 G3 loop-points delivery-path — delivery で health の証跡の base_sha を target_sha と照合する
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 loop-points delivery-path — delivery で health の証跡の base_sha を target_sha と照合する

WorkUnit `delivery-path`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1、unit loop-points の子）。evidence の final review の advisory 1（delivery の照合が health の証跡自身の `base_sha` を event の `target_sha` と比べていない）を直し、[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D2.6 の delivery の照合を review の証跡で確かめる。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 変更
commit（件名）:
- 「G3 loop-points delivery-path: delivery で health の証跡の base_sha を束縛の target_sha と照合する」
- 「G3 loop-points delivery-path: 進捗を書く」

流れ:
1. `crates/celeris/src/delivery.rs`（`check_required_evidence`）: `task_core::required_checks::verify_required_checks` が合格した後に `health_evaluated_target` を呼ぶ。`(repo, target_sha, reviewed_sha)` に束縛された最新の `RequiredChecksEvaluated` の health の項目のうち、証跡自身の `base_sha` が `target_sha` と違うものがあれば `Err("health の証跡の基底 <base_sha> が束縛の基底 <target_sha> と一致しません")` を返す。`Err` は既存の経路（`request_rereview`）で配送の行の `detail` に残り、task は再 review に戻る（ff しない）。
2. PolicyOnly の証跡は今までどおり通さない（`verify_health` が `health は policy の変更だけの不合格で、承認を束縛から判定できません` を返す）。G2 の承認を `(repo, target_sha, reviewed_sha)` に結び付ける event が無いことを確かめた（下の「判断」）。
3. `crates/celeris/src/delivery/health_evidence_tests.rs`: `deliver`・`record_health` を `pub(super)` にした（新しい試験の module から使う）。

判断（PolicyOnly）: 今の event から G2 の承認を束縛に結び付けられない。
- 承認は `Approval` の子 task の状態（SQLite の task の行）で、子の title（`health_review::policy_review::approval_title`、`pub(crate)`）は repo と `reviewed_sha` の先頭 12 文字しか持たず、`target_sha` を持たない。
- review が残す承認の記録は `ReviewVerdict.reason` の自由文（`human(approval task <id>): health:policy approved by a human: <files>`）で、repo も基底も持たない。
- これを照合に使うには自由文の解析と task-dispatch の title の書式の写しが要り、束縛にならない。通すには新しい event の欄が要り、この unit の範囲外（objective の「新しい event の欄が要るなら範囲外として replan」）。done の条件は PolicyOnly を通すことを求めないので、この unit は「通さない」で閉じ、下の「提案」に書いた。

## 試験（守る不変条件と、既存の試験で検出できない理由）
`crates/celeris/src/delivery/health_enforce_tests.rs`（`delivery/tests.rs` の子 module）。既存の `health_evidence_delivery_*` は証跡を手で作り、`base_sha` を常に `target_sha` にするので、別の基底を評価した証跡と、review の証跡と delivery の照合の食い違いを検出できない。
- `health_enforce_delivery_review_evidence_passes`: review と同じ経路（経路共通の入口 `gate::health_gate` → review の項目 `review_item::item`）で作った health の証跡を、review の記録と同じ形（`RequiredChecksEvaluated`、指紋は `definition_digest(cmds, 項目の検査器)`）で残すと、`check_required_evidence` を通り main に ff される。証跡の `base_sha` が束縛の `target_sha` と同じことも確かめる（evidence-delivery-verify の提案 1）。同梱の検査器は一時 dir に書いた合格の stub を `gate::read_bundled_checker` で読み、引数で渡す（process の OnceLock は設定しない）。
- `health_enforce_delivery_base_sha_mismatch_is_rejected`: ほかの欄は合格の証跡と同じで `base_sha` だけが違う証跡は取り込まれず、`detail` に理由と証跡の基底が残り、再 review に戻る。この変更の前は合格していた入力。
- `health_enforce_delivery_policy_only_is_rejected_without_bound_approval`: review が合格として記録した PolicyOnly の項目は、同じ review run に人の承認の `ReviewVerdict` があっても取り込まれない。

## 証拠（提出時の HEAD で実行。最後の commit「G3 loop-points delivery-path: 進捗を書く」の後）
- `cargo nextest run -p celeris --no-tests=fail -E 'test(/health_enforce_delivery_/)'`: exit 0、3 tests run: 3 passed。
- `cargo nextest run -p celeris -E 'test(/health_enforce_delivery_|health_evidence_delivery_/)'`: exit 0、11 passed（既存の 8 件を含む）。
- `cargo fmt --all -- --check`: exit 0（差分なし）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/test-affected.sh`（対象 crate は celeris・celerisctl）: exit 0、nextest 1007 passed・2 skipped。check-health は verdict=pass・fail_grade=0・new=1（`dup.contract` の warning。`UnavailableReason` が `task_core::required_checks::health` と `task_core::route_select::availability` の 2 か所にある。facts の unit の変更で、この unit の変更ではない）。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を範囲の正規表現で絞る）: 範囲外の path なし。

## 未解決
- 人が承認した policy の変更（PolicyOnly）の候補は delivery の照合で止まり、再 review に戻る。再 review の後も同じ理由で止まると考えられるが、その後の流れ（`ReviewResyncHalted` で受信箱に出るか）は試験で確かめていない（未確認）。

## 提案
1. `base_sha` の照合の正本を `task_core::required_checks::verify_health`（health の証跡の照合の owner）に移す。この unit の範囲は `crates/celeris/src/delivery*` だけで、`required_checks.rs` は reuse-sync の範囲なので delivery.rs に置いた。移したら `delivery.rs` の `health_evaluated_target`（束縛の最新の証跡を引く処理が `required_checks::latest_bound` と重なる）を消す。
2. PolicyOnly を承認つきで通すには、G2 の承認を `(repo, target_sha, reviewed_sha)` に結び付ける event の欄が要る（例: `policy_review_gate` が承認の verdict と同じ transaction で、承認の子の id と束縛を持つ構造化した記録を残す）。そのうえで `verify_health` がその記録を照合する。ADR D2.6 の「PolicyOnly で G2 の承認がある」の実装に当たるので、後続の unit か task で範囲（task-core の events・task-dispatch の policy_review・required_checks）を取って行う。
