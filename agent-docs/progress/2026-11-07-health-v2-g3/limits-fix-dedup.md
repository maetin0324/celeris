---
title: 健全性v2 G3 limits fix-dedup — health の往復・使い切りの重複実装を 1 か所にまとめる
tasks: [01M70C8NHEANARRKZD9RGKZ87D]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 limits fix-dedup — health の往復・使い切りの重複実装を 1 か所にまとめる

WorkUnit `fix-dedup`（task 01M70C8NHEANARRKZD9RGKZ87D）。final review の不合格 `health:duplicate_implementation`（fingerprint の集合の sha256 の二重実装）と、同じ review の advisory 2 件（往復の止め方の組が 2 か所・使い切りの質問の語が 2 つ）を直す。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D4.3・D4.4。基準は `$CELERIS_WU_BASE`。

結論: 完了。受信箱の鍵の sha256 は `task_core::health_repeat::fingerprint_set_sha256` だけが求める。WU の check・段の統合・task の review の往復は `check_item::health_repeat_stop` の 1 関数を通り、修復の上限の使い切りの質問は `check_item::repair_limit_question`（先頭の語 `health_repair_limit`）の 1 関数が作る。`health_repair_cap` の語は crates/ と ADR から消えた。新しい config・予算の欄は足していない。2000 行を超える file（`dispatcher.rs` 2784 行）は変えていない。

## 実装前 brief
1. 根本原因: limits の WU を作るときに、往復の判定（`health_review::limits::judge`）が受信箱の鍵の sha256 も自分で求めた（`fingerprints.join("\n")`、末尾の改行なし）。task-core の `fingerprint_set_sha256` は各 fingerprint の後に `\n` を付けるので値が違う。本番の呼び出し側はこの値を読み捨てていたが、`RepeatVerdict::Repeat.set_sha256` の doc は「受信箱の鍵の一部」と書き、`limits_tests.rs` が違う値を固定していた。review-wire は `check_item.rs` の `Dispatcher::health_repeat_stop`（method で `self.store` を使う）を呼べず、`review_limits::stop_on_repeat` に judge → 追記 → 質問の組を写した。そのとき質問の語も文字列で書き、使い切りの語を `health_repair_cap` にした。
2. 責務 owner: 受信箱の鍵は task-core の `model/health_repeat.rs`（`Event::health_findings_repeated` が呼ぶ）。往復の判定は `health_review/limits.rs`。往復の止め方と質問の文・使い切りの質問の文は `health_review/check_item.rs`（`stops_without_replan` が先頭の語を知る所）。
3. 既存の再利用先: `task_core::health_repeat::fingerprint_set_sha256`、`check_item::HEALTH_FINDINGS_REPEATED`・`HEALTH_REPAIR_LIMIT`、`HealthRepeatSite::label`、既存の試験 `health_limits_*`・`health_enforce_*`。
4. 選択肢: (a) `limits.rs` から hash を消す（採用。呼び出し側は `..` で読み捨てていて、鍵は `Event::health_findings_repeated` が作る）。(b) `limits.rs` から task-core の関数を呼ぶ（同じ値を 2 か所に持つことになり、使い道も無いので採らない）。往復の止め方は、`Dispatcher` の method を store を受ける自由関数にして review からも呼べるようにした（`Dispatcher` を review の子 module に渡す案は、`try_review_repair` の引数を増やして `review_verdict.rs` を変えるので採らない）。使い切りの質問は、先頭の語だけでなく文も 1 関数にした（数の書き方が 2 通りあると CoS と人が 2 つの書式を読むことになるため）。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: `RepeatVerdict::Repeat` の hash の欄、`limits.rs` の private の hash の関数と `sha2` の use、`limits_tests.rs` の `sha_of`、`review_limits.rs` の judge の呼び出しと質問の文字列 2 つ、`Dispatcher::health_repeat_stop`（method）、ADR の付記の `health_repair_cap`。
7. 不変条件: 受信箱の鍵の hash の実装は task-core の 1 か所だけ。3 経路の往復は同じ関数で判定・追記・質問の文を作る。使い切りの質問の先頭の語は `health_repair_limit` の 1 つで、`stops_without_replan` が知る語と同じ。試験は下の「試験」。

## 変更
- `crates/task-dispatch/src/health_review/limits.rs`: `RepeatVerdict::Repeat` は `fingerprints` だけを持つ。hash の関数と `sha2` の use を消し、doc に「鍵の hash は task-core の 1 か所が求める」と書いた。
- `crates/task-dispatch/src/health_review/check_item.rs`: `health_repeat_stop(store, task, site)` を自由関数（`pub(in crate::dispatcher)`）にした。質問の文は `repeated_question`（`HEALTH_FINDINGS_REPEATED` と `HealthRepeatSite::label`）で作る。使い切りの質問の文を `repair_limit_question(site, bucket, (修復の合計, max_repairs), (bucket の修復の数, max_repairs_per_class), 指摘)` にまとめ、段の統合の `phase_health_stop` がそれを呼ぶ。両方の質問の最後に「直し方を comment で書くと再開します」を足した（review の旧い文にあった案内を 3 経路にそろえた）。
- `crates/task-dispatch/src/health_review/review_limits.rs`: `stop_on_repeat` は `check_item::health_repeat_stop` を呼んで block するだけ。`stop_on_cap` は `check_item::repair_limit_question` を呼ぶ。`limits::judge` の呼び出しと質問の語の文字列は無い。root への追記は block の前になった（WU の check・段の統合と同じ順。受信箱の項目は源が Blocked を出た行 id と比べるので、`Reviewing → Blocked` の遷移が追記の後でも消えない）。
- 質問の文の変化: review の使い切りは `health_repair_limit: review: health の修復が上限に達しました（修復の合計 1/3 回、health:deps.violation の修復 1/1 回）。…` になった（旧: `health_repair_cap: …（bucket health:deps.violation: 1 回 / max_repairs_per_class 1、修復の合計 1 回 / max_repairs 3）`）。段の統合は「統合の修復 {n}/{max} 回」を「修復の合計 {n}/{max} 回」にした（数えているのは task の repair WU の全部なので、語を事実に合わせた）。review の往復の質問は `health_findings_repeated: review: …` になった。
- 試験の期待値: `dispatcher/tests/health_limits_review.rs`・`health_limits_budget.rs` の `health_repair_cap: ` を `health_repair_limit: review: ` に、回数の書式を新しい文に直した。`limits_tests.rs` の `repeat()` は fingerprints だけを組む。
- `agent-docs/adr/2026-11-08-health-enforcement-loop.md` の 2026-11-09 の limits の付記（同じ行）: (a) に `set_sha256` は task-core の `health_repeat::fingerprint_set_sha256` の 1 か所が求めることと、3 経路が `check_item::health_repeat_stop` を通ることを書いた。(b) の質問を `health_repair_limit: <位置>: …` に、(c) の質問の先頭を `health_repair_limit:` にした。

## 試験
- `health_limits_repeat_set_sha_does_not_depend_on_order`（`limits_tests.rs`、名前は保った）: `judge` の fingerprints が評価の並び・重複に依らず昇順・重複なしになることと、その fingerprints を `task_core::health_repeat::fingerprint_set_sha256` に渡した値が並べ替え・重複のある入力と同じになることを確かめる。task-core の試験は hash の関数だけを見るので、judge の出力を渡したときの一致はこの試験だけが見る。
- 3 経路が同じ関数を通ることは、既存の dispatcher の試験の質問の先頭（`health_findings_repeated: WU a の check: `・`health_findings_repeated: 段 build の統合: `・`health_findings_repeated: `〈review〉）と、`health_repair_limit: 段 build の統合: `・`health_repair_limit: review: ` で確かめる。試験は消していない・弱めていない（期待値の文だけを新しい文に直した）。

## 確かめ方（grep。提出時の HEAD で実行）
- `grep -nE 'Sha256|fn set_sha256|set_sha256:' crates/task-dispatch/src/health_review/limits.rs crates/task-dispatch/src/health_review/limits_tests.rs` → 該当なし（exit 1）
- `grep -nE 'limits::judge|"health_' crates/task-dispatch/src/health_review/review_limits.rs` → 該当なし（exit 1）
- `git grep -n health_repair_cap -- crates agent-docs/adr` → 該当なし（exit 1）

## 変更（commit の件名）
- G3 limits fix-dedup: fingerprint の集合の sha256・往復の止め方・使い切りの語を 1 か所にまとめる

## 証拠（提出時の HEAD で実行。最後の同期の後に再実行する）
- `cargo nextest run -p task-core -p task-ops -p task-dispatch --no-tests=fail -E 'test(/health_limits_|health_enforce_/)'` → exit 0、88 passed
- `cargo nextest list --workspace -E 'test(/health_limits_/)' --message-format oneline | grep -c health_limits_` → 39
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-affected.sh` → 下の「test-affected」

## 未解決
- `limits.md`（limits の記録）の未解決 2・3・4 は、この直しの後の事実と合わない（hash の規則・2 つの語・review の文字列）。Objective どおり `limits.md` は変えず、reclose が改める。

## 提案
- 無し。
