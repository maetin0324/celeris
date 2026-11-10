---
title: 健全性v2 G4 exc-expiry — profile の例外の期限（expires）と期限切れの例外の扱い
tasks: [01M72DBBE7AFCNXYWSB5M0YZW9]
status: running
updated: 2026-11-10
---
# 健全性v2 G4 exc-expiry — profile の例外の期限（expires）と期限切れの例外の扱い

完了日: 未（WorkUnit `expiry` の実装と試験は 2026-11-10 に終えた。task は、人が承認の材料 exc-expiry-policy-diff.md で policy の差分を承認し、operator がこの branch を main に no-ff merge した後に、WorkUnit `land` が done にする）

WorkUnit `expiry`（task 01M72DBBE7AFCNXYWSB5M0YZW9）。人の決定 exception-expiry「G4 で実装する（子 task exc-expiry）」による。決め事は [ADR 2026-11-07 repo health の契約](../../adr/2026-11-07-repo-health-contract.md) の「付記（2026-11-10、期限付き例外）」。変えた検査器は Celeris profile の `policy_paths`（`^scripts/dev/repo-health/`）に当たる policy の file なので、この branch は policy の branch になる（前例は [G3 ci-entry](../2026-11-07-health-v2-g3/ci-entry.md)）。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 変更
- 件名「G4 exc-expiry: 契約の ADR に期限付き例外の付記を書く」の commit: ADR に付記 1〜6（欄の意味・判定に使う profile・基準日・report の形・検証器 S13・試験名）と採らなかった案 3 件を書いた。
- 件名「G4 exc-expiry: profile の例外の expires と期限切れの例外の report（base 側で判定）」の commit:
  - `health/profile.py`: `[[rules."<id>".exceptions]]` に任意の `expires`（TOML の日付）。`type(v) is datetime.date` で確かめ、文字列・日時・時刻・数は `profile_invalid`（exit 2、report を書かない）。`Exception_.expired(as_of)`（基準日 > expires、基準日が無ければ期限付きは期限切れ）と `Profile.has_expiring_exceptions`。docstring に欄を足した。
  - `health/snapshot.py`: `Snapshot.base_date` = base の commit の `%ct`（UTC の epoch 秒）を UTC の日付にしたもの。base に届かなければ None。
  - `health/engine.py`: `_exception` は一致する例外のうち期限内の先頭を返し、無ければ期限切れの先頭を返す。期限切れなら finding の `exception` は null、`expired_exception` = `{reason, source, expires}`。期限内で期限付きなら `exception.expires`。profile に期限付きの例外があるときだけ `policy.exceptions_as_of`。`is_fail_grade` は変えていない。
  - `repo_health.py`: 要約の fail 級の式の写しを `engine.is_fail_grade(f)` の呼び出しに置き換えた。期限切れの例外の finding の FAIL 行の下に「例外は期限切れ: expires=… 基準日=… <profile>:<行>（理由）」を出す。
  - `validate_report.py`: 意味規則 S13（`expired_exception` と `exception` の排他、期限付きの例外があれば `policy.exceptions_as_of` がある、期限内は基準日 ≦ expires、期限切れは基準日が null か > expires）。
  - `schema/report-v1.schema.json`: `policy.exceptions_as_of`、`exception.expires`、`expired_exception` と `$defs` の `date`・`exception_source`。例 `examples/exception-expiry.json`（期限切れ 1 件と期限内 1 件。本物の検査器の出力）、不正な例 4 件（`expired-exception-with-exception`・`exception-expires-before-as-of`・`expired-exception-not-yet-expired`・`exception-expires-without-as-of`。どれも S13 だけで落ちる）。
  - `scripts/tests/test_repo_health_exception_expiry.py`（新規）: 試験 5 件と module 関数 `make_expiry_repo(root, case)`・`run_checker(repo, base, head, out)`。本物の検査器と本物の rule `size.responsibility-growth`（`file_lines = 3`）を一時 repo に当てる。commit の日付は `GIT_COMMITTER_DATE`、作者は `GIT_AUTHOR_*`・`GIT_COMMITTER_*` で固定するので、同じ case からは同じ sha が出る。
- `crates/`・`web/`・`docs/` は変えていない。既存の examples 12 件・bad-examples 21 件・既存の試験は変えていない。

### make_expiry_repo の 3 例（unit record が材料に使う）
| case | base の commit の日付 | 基準日（UTC） | base の expires | head の expires | head の commit の日付 | 結果 |
|---|---|---|---|---|---|---|
| expired | 2026-03-02T10:00:00+09:00 | 2026-03-02 | 2026-03-01 | 2026-03-01 | 2026-02-01 | exit 1。size の finding は `exception` null・`expired_exception.expires` 2026-03-01、fail 級 1 件 |
| in_term | 2026-03-02T01:00:00+09:00 | 2026-03-01 | 2026-03-01 | 2026-03-01 | 2026-12-01 | exit 0。`exception.expires` 2026-03-01（当日まで有効）、fail 級 0 件 |
| extended | 2026-03-02T10:00:00+09:00 | 2026-03-02 | 2026-03-01 | 2027-03-01 | 2026-03-03 | exit 1。size の finding は base の期限で期限切れ、`policy.changed` も fail、fail 級 2 件 |

head の日付で判定すれば expired と in_term の結果が逆になる。in_term の base の日付は JST では 2026-03-02 で、UTC で判定するときだけ期限の当日になる。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`、ci.sh は `git merge-base HEAD main`）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_exception_expiry.py' -v` → exit 0、5 件 ok（`test_exception_expired_finding_counts_as_fail`・`test_exception_expiry_in_term_exempts_finding`・`test_exception_expiry_extension_by_candidate_does_not_pass`・`test_exception_expiry_rejects_invalid_value`・`test_exception_expiry_report_validates`）。
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → exit 0、78 件 OK（変更前の 73 件 + 5 件）。
- `python3 -I scripts/dev/repo-health/validate_report.py scripts/dev/repo-health/schema/examples/*.json` → exit 0、`13 valid, 0 invalid`。不正な例 25 件は 1 件ずつ exit 1（`test_repo_health_contract` の `test_bad_examples_each_invalid_with_location` が合格）。
- 期限の無い profile の report が変更前と byte 単位で同じ: `$CELERIS_WU_BASE` の `scripts/dev/repo-health`・`source-size-report.py`・`check-architecture-map.py` を `git archive` で `$TMPDIR` に展開した旧い検査器と、この branch の検査器で同じ入力を評価し `cmp` → 差分 0。入力は (a) 期限の無い例外を持つ一時 repo（例外の付いた finding あり、7315 byte）、(b) この repo の `--base main~1 --head main`（Celeris profile、185583 byte）。
- `sh scripts/dev/repo-health/ci.sh --base "$(git merge-base HEAD main)"` → exit 1、`verdict=fail reviewable=true fail_grade=1 new=1 worsened=0`。fail 級の finding は `policy.changed` の 1 件だけ（変えた policy の file 11 件: `health/engine.py`・`health/profile.py`・`health/snapshot.py`・`repo_health.py`・`validate_report.py`・`schema/report-v1.schema.json`・例 1 件・不正な例 4 件）。必須 rule で pass でないのは `policy.changed`（fail）だけ。Celeris profile には期限付きの例外が無いので、report に `policy.exceptions_as_of` は無い。
- `bash scripts/dev/test-affected.sh` → exit 1。最初の段の check-health.sh が上と同じ `policy.changed` だけで exit 1 になる（D6.6。`--list` の対象は 0 件で、crate と web は変えていない）。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0（Rust は変えていない）。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → ADR、`scripts/dev/repo-health/` の下の 11 件、`scripts/tests/test_repo_health_exception_expiry.py`、この進捗だけ。

## 未解決
- この branch は main に merge するまで、`ci.sh`・`check-health.sh`・`test-affected.sh` が `policy.changed` で exit 1 になる（ADR health-review-integration D6.6）。D6.2（policy の file だけの変更を exit 0 にする扱い）は働かない。試験（`scripts/tests/`）・ADR・進捗は policy の file でない。
- 人の承認と operator の no-ff merge は後続の WorkUnit（record が承認の材料 exc-expiry-policy-diff.md を作り、land が merge の後に main の祖先と check-health.sh を確かめる）。
- 今の Celeris profile（`profiles/celeris.toml`）の例外 2 件には期限を付けていない。付けるかどうかは決めていない。

## 提案
- 期限切れが近い例外を前もって知らせる仕組み（例: 基準日から 14 日以内に切れる例外を report の warning にする）は、この付記に含めていない。要るなら別の付記で、期限の何日前から知らせるかを決めてから足す。
