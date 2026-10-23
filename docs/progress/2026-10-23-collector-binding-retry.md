# 収集障害を GradeBinding・check/export まで保持する（再監査 3/P0 再試行）

---
tasks: [01M5KVHDY26YCNYGZKDTANGJX6]
---

前回 commit `e75f3e76` の受理記録・track・台帳・collector 分類を引き継ぎ、差し戻しになった受け入れ条件 3 を修正した。確認した main は `13d64acd1780` のままで、実装 commit は `9481ce03`、本修正の main 反映 commit は未確定（取り込み前）。最新 main で解消済みの追加項目はない。

## 実装

`GradeOutcome::ObservationFailure(Vec<CollectIssue>)` で収集分類と詳細を保持する。bind は非空の既知分類、dominant failure_class と grade の一致、final_pass=false を検査する。従来の observation 欄を持たない grade は従来どおり扱う。

check/export の `observation_failed` は収集失敗の cell 数、`observation_failures` は分類別 cell 数。同一 cell の同一分類は重複計上しない。複数分類のある cell は分類別の数に重複するため、この map の和を分母に使わない。これらの値は grader_env_failure / grader_undetermined から分離し、JSON と CSV の両方に出す。not_submitted 以外は complete=false とし、原因を直して再収集・再採点する案内を出す。not_submitted は未投入という確定結果である。

workflow fault は引き続き別軸とし、fault_breakdown に注入状態 × observation_failed を出す。分母・経過時間・消費の計算と採点履歴の保存方式は変えない。受理応答喪失の reconciliation は追加していない。

## 検証

- 通常の launcher 試験に、偽 scheduler の取得失敗 → grade → binding → check/export の確認を追加。A の fetch_failed が別分類で現れ、再収集の案内と CSV 出力に残る。再収集成功後に分類が消え、分母・時間・消費・trials.jsonl が変わらず、旧 grade が grades.history.jsonl に残る。
- 全7分類について、同一分類の重複を数えず、注入済み workflow fault と別軸で数えること、矛盾した failure_class を拒否することを確認する。
- 初回 test-affected は通知試験 credential_wait_notify が、HTTP 要求なしの切断を空の通知として記録し失敗した（test-affected-first.log、exit 100）。偽 webhook を、完全な要求だけを記録し、応答前に受信記録を確定するよう局所修正した。本番の通知処理は変更していない。
- `cargo test -p celeris-campaign --test grade_failure_class --test launcher`: exit 0（6 + 13 passed、実 daemon rehearsal 1 ignored）。
- `cargo test -p celeris --test credential_wait_notify wait_notify_register_verify_and_resume_once_through_the_fake_webhook -- --exact`: exit 0（1 passed）。
- `bash scripts/dev/test-affected.sh`: 最終 exit 0（2587 passed、4 skipped、doc tests 成功）。
- `cargo clippy --workspace -- -D warnings`: 最終 exit 0。
- `cargo fmt --all -- --check`: 最終 exit 0。

## 証拠の保存と測定開始条件

検査ログは `/local/celeris/data/workspaces/01M5KVHDY26YCNYGZKDTANGJX6/artifacts/` の targeted.log、test-affected.log、clippy.log、fmt.log に保存する。試験の raw・grade・集計は tempfile 内の合成データで、試験終了時に削除する。実測定の raw・grade・集計は新規作成しておらず、既存の履歴も変更していない。

K2/HPC 比較はまだ開始不可。前回報告の未解決条件を継続する: Sirius の qstat Variable_List に trial 印が残ることの実機確認、測定用 A の cap-env/prompt への wrapper 導入、およびタスク1・2の成立が必要。取得元は Sirius の scheduler 記録、設定元は測定 campaign の A cap-env と case prompt。これらの実資産・実機の確認を fixture 成功で代替しない。本番設定の変更、実 pilot、有料評価は実施していない。

## 再試行 3（attempt 3）: executor 失敗 cell の観測障害の集計

前回レビューの指摘（checker の ObservationFailure 集計が executor passed の分岐の中だけにあり、failed/timeout 等の cell の fetch_failed が `observation_failed` に出ない。faults の reconcile も executor_* しか出さない）を再現・修正した。

- `checker::recompute_graded`: `observation_failed`/`observation_failures` を executor の結果と独立に全 cell で数える。排他の内訳用に `grader_observation_failed`（executor passed で収集失敗）を足し、export の `failure_breakdown.observation_failed` はこれを使う（内訳の和は n のまま）。再収集・再採点の案内（incomplete_reasons）も全 cell の分類から出る。
- `faults::reconcile`: `fault_breakdown`（注入状態 × executor/grade 結果）とは別に `observation_breakdown`（注入状態 × `observation_failed`/`no_collector_failure`）を数え、export の JSON/CSV に出す。workflow fault と観測障害を同じ軸に混ぜない。
- 試験 `grade_failure_class::fetch_failure_is_counted_for_executor_failed_and_timeout_cells_and_clears_on_regrade`: scripted executor で全 cell を failed/timeout にし、fetch_failed の grade 行で bind → check/export を通す（observation_failed = n、grader_failed = 0、fault_breakdown は executor_failed、observation_breakdown に observation_failed、complete=false と再収集案内）。その後に失敗記録の無い状態で実 grade_out を再実行し、観測障害が消え前の grade が grades.history.jsonl に残ることを確かめる。pilot の grade case は実験 case でないので、1 回目の grade 行は grade_out と同じ形で書いた（実験 case の実 grade_out 経路は launcher 試験が覆う）。
- ADR `agent-docs/adr/2026-10-23-a-submit-audit-and-collect-classes.md` D6 を更新。

検証結果は task の完了報告に記す。K2/HPC 比較の開始可否の結論（開始不可）と未解決条件は上記から変わらない。
