---
task: 01M5B84ZY6H4001ZYPSGN9VR34
tasks: [01M5B84ZY6H4001ZYPSGN9VR34]
title: 統合HEADの検査と専門分担適用証拠の記録（final）
work_unit: final-validation
status: done
completed: 2026-10-20
updated: 2026-10-20
---
# final: 統合 HEAD の検査と適用証拠

実装は追加していない（検査と記録のみ）。検査対象の統合 HEAD = `506d02a927cc`（integrate wu/application-proof）。

## 1. 検査結果（統合 HEAD）
- `cargo test -p celeris-campaign --test ablation_contract --test ablation_application` → exit 0。ablation_application 4 passed（22.46s）、ablation_contract 17 passed。
- `bash scripts/dev/test-parallel.sh` → exit 0。`CELERIS_TEST_SUMMARY`: passed 5921 / failed 0 / ignored 15（258 binaries）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 2. 適用の範囲（specialization のみ）
- 選択機構は専門分担（`M1-specialization`）の 1 つだけ。内部レビュー・KB 共有は選ばず、既定のまま。
- 固定 hash: `selection.sha256` = `b6132a4456d00be83fa03bf3a54deaaec5b9874fa7bc0397c0d8bdd8937964bc`（`crates/campaign/fixtures/ablation/selection.json` の正規化 JSON の sha256。`selection_is_fixed_and_matches_its_hash` が照合）。
- 契約試験（ablation_contract）: hash の安定性、1 欄ごとの改変の検出、再 seal された非対象差分の拒否、欠落/不一致の variant を再開・冪等照会で採用しないこと。
- 実適用証拠（ablation_application）: 無 LLM の通常 HTTP POST を実 `celeris` binary（一時 DB・token・org・workspace を所有）に送り、実 dispatcher から実スタブ process が起動。計画の通常採用で子・孫を実生成し、孫の retry（error→done）を events/runs で読む。off は全 task が単一 `coding` 課、on は `coding-spec` を含む 2 課。スタブ PID・run id・events の対応を assert。
- 非対象不変性（検証済み）: review と安全上限（`deny_tools`・`max_lane`・`max_attempts`）、arm の configs、全役割モデル、資料、予算、終了条件、外部 grader を B0 基準の `BaselinePin` と照合。canary と改変・欠落・不一致の拒否（409/4xx/Refused）を実 daemon 経由で確認。

## 3. 既存 task との重複確認
`application.md` §1 の調査（2026-10-19）の結果を引き継ぐ。設定確定 01M58VFRBA6HG0SSF3CG5BKM91 は done で manifest/lock/env-lock/preflight を再利用、無LLM統合 01M58VG5C4XGK4E70M6WJ6MEX0 と主比較 pilot 01M58VGDNQ8GWGJPB8SJ73XSPF は cancelled で実行証拠は流用していない。新スケジューラ・ルータ・hash・lock は作っていない。

## 4. 最新 main との差分と統合失敗の原因/修復
- 最新 main = `cb0cd7b8`。本 branch の祖先ではない（`git merge-base --is-ancestor main HEAD` が偽）。merge-base `c9654bff` からの main 側差分は 235 file（+28529/−370）。
- `git merge-tree --write-tree --name-only HEAD main` の衝突は 17 file: progress 4（cap-env-content-pin, resolved-routing-evidence-mode, result-location-scope, cap-probe-ro-mount-kind）、`crates/campaign/src/main.rs`、`crates/campaign/tests/env_lock.rs`、task-core の `cluster_job/tests.rs` `cron/store_tests.rs` `decision_log.rs` `delivery.rs` `store/{credential_wait_tests,decision_log,migrations,route_records,routing_evidence,routing_log_tests,tests}.rs`。migration 番号は main 0087、本 branch 0088 で衝突せず、`migrations.rs` の登録行だけが衝突。**main 取り込みは未実施**（本 WU の範囲外。取り込み後は上記検査の再実行が必要）。
- 本 task 内の統合失敗の原因/修復: contract 段は `integration-fix.md`（診断と最小修復、exit 143 は再現せず外部 SIGTERM の送り元は未確定）、daemon-obs の clippy `type_complexity` は型別名 `CapturedRun` で修復（`application.md`）。いずれも担当領域で修正済みで、本葉では修正していない。

## 5. 未検証・限定事項
- 主張しない: 内部レビューの品質、K2 の analysis/review の検証済み。K2 は `not_judged = [analysis, review]` のまま。
- 未検証: main 取り込み後の挙動、実 LLM/実 API、本番 daemon、内部レビュー off・KB 共有の variant、実 catalog からの env-lock（`fetch_catalog`）、root task の最終 review・統合。
- worker の PID は events/runs に無く、証拠はスタブ側ログと daemon の `Child::id()` の突き合わせ。試験 process が SIGKILL されると fixture dir が `$TMPDIR` に残る。
- test-parallel は `tests left 57 entries in TMPDIR` の警告（終了時に削除、exit 0）。

## 提案
- main 取り込みを統合段または人が行い、上の検査を再実行する。
- `WorkerStarted` に任意欄 `pid` を足す（別 task）。
- 本番移行: 不要（コード・試験のみ）。
