---
title: WU・段の統合の health の base を、review target を取り込んだ branch に合わせる
tasks: [01M79CHBRY3CBHKPNECPPM6G32]
status: running
updated: 2026-11-13
---
# WU・段の統合の health の base を、review target を取り込んだ branch に合わせる

完了日: 2026-11-13（close-out 時点。task 全体の完了は review が決める）

基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。

## 変更（commit は件名）
- `phase 1: adr-base: WU の check と段の統合の health の base を、取り込んだ review target の分だけ進める ADR を足し、D1.2 付記 2026-11-10 を置き換え済みにする` — ADR 2026-11-13-health-base-merged-target。I3 を保つ理由と退けた案を書いた。
- `phase base-fn: WU・段の統合の health の base を target_base::health_base の 1 関数にし、旧 phase_health_base と旧規則の試験を消す`（と、その後の module 位置の 2 件） — `health_review/target_base.rs`。旧規則 `phase_health_base`・WU の `base = wu.base_commit` の固定・`check_item_phase_base_tests.rs` を撤去。
- `phase base-tests: 8 形の base 規則の決定的な試験（target_base_tests.rs）を足し、進捗を更新` — prefix `health_base_` の 8 件。新しい規則を外すと落ちる。
- `wu/verify-nextest: health_review の試験を nextest で検証し、cargo test の失敗の原因を記録する`、`fix-cargo-test: 同梱の検査器を試験の種類ごとに直列化して cargo test でも health_review を通す` — 試験の保管場所の直列化（本番コードは変えない）。
- close-out（この WU）: `docs/architecture-map.md` の repo health の強制の行に `target_base.rs`・ADR・試験・不変条件を足した。

## 旧い規則の説明の確認
- `grep -rnE "base = M|基点 P|phase_health_base|wu\.base_commit|target_base" docs agent-docs/guides scripts crates`（md・sh・py・toml）: 該当は `docs/architecture-map.md` だけ（今回足した行）。旧い規則を説明する文書は残っていない。ADR 2026-11-08 の付記は「置き換え済み」と明記済みの当時の記録（ADR は対象外）。
- architecture-map の health の base の行は旧い規則を指していなかった（base の規則の記述が無かった）ので、新しい規則を足した。

## 証拠（提出時の HEAD で実行）
- `python3 scripts/dev/check-architecture-map.py` → OK（パスの存在）。
- 実装 WU の証拠（impl-base 進捗）: `cargo nextest run -p task-dispatch -E 'test(health_base_)'` 8 件合格、`bash scripts/dev/test-affected.sh` exit 0、clippy・fmt exit 0。
- この WU の最終の `test-affected.sh`・`progress-index.sh --check` の結果は報告本文に書く。

## 未解決
- ADR D6 の証跡欄 `intake`（`HealthIntake`）と、衝突時の 2 回の結果の reason の並記は未実装（impl-base 進捗の未解決）。
- R4 の WU sync-policy を新しい規則で再検査した結果は本番でしか確かめられない（下の手順）。

## 提案
- plan の check で health の試験を回すときは `cargo test` でなく nextest か個別実行にする（`cargo test` は同梱検査器の process 全体の設定が競合する。直列化の修正は入ったが nextest が正）。
- ADR D6 の `intake` 欄が要るなら別 task にする。

## 本番への移行（人が実行する）
config の変更は要らない（新しい config 欄・旧欄の削除は無い）。daemon の再起動は release の昇格で行う。

1. 運用者が、この変更を含む release を build・gate・昇格する（`scripts/selfdeploy/` の通常の手順。promote には `SD_ACTOR` が要る）。
2. 昇格後、動いている release が新しい規則を含むことを確かめる（例: `celerisctl` の release 表示で commit が昇格した release であること）。
3. 受信箱の `authorization-01M79A6FVE7C9BRZ2JPVDF2RVN`（R4 の WU sync-policy `01M74TGFAQM3QZKFXMQER7SMTD` の止まり）に「再検査」と答える。
4. WU の check `celeris:repo-health/1` が再評価され、base が「WU の base と main 9091b3aa の merge」（衝突のため 2 回目の base = 9091b3aa も評価）になる。期待: verdict=pass、fail-grade 0 件。
5. 確認: task 01M74TGFAQM3QZKFXMQER7SMTD が Blocked から戻り、check が pass になること。まだ fail-grade が出る場合は、その指摘（rule_id・位置）を受信箱に書いて報告する（ADR D3.4 のとおり、WU が target の分でない変更を持てば fail は正しい）。
6. R4 の段の統合でも同じ規則が使われる。統合で health が落ちたら report の `base_sha` が merge commit（親が start と target）か target 自身であることを確かめる。
