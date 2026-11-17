---
title: repo health: 許可の一覧から行を消すだけの変更を policy.changed に数えない（policy だけの branch）
tasks: [01M7KATBGCRQRZ5NWXHXVWYPZ8]
status: done
updated: 2026-11-17
completed: 2026-11-17
---
# repo health: 許可の一覧から行を消すだけの変更を policy.changed に数えない（policy だけの branch）

完了日: 2026-11-17。WorkUnit repair-1 の policy の差分（`engine.py`）も人が承認し、operator が main に merge した（下の「repair-1」）。

人の決定（2026-11-16、operator が中継）: 許可の一覧（`scripts/dev/human-text-lint.allow`・`scripts/dev/adr-allowed-duplicates.txt`）から行を消すだけの変更は、repo health の `policy.changed` に数えない。行を足す・書き換える変更は今までどおり数える。この task は policy の file・その試験・ADR・進捗だけを変える policy の branch（先例 01M76BYGKY7R8SBZ22AZGJ72SP、[ADR 2026-11-07-health-review-integration](../adr/2026-11-07-health-review-integration.md) D6.6）。

人は 2026-11-17 に policy の差分（`engine.py` の `policy_changed_files`、`profile.py` の `policy_allowlists`、`celeris.toml`、`rules/policy_changed.py`）を承認した（operator が中継）。operator が WU allowlist-shrink の branch を main に no-ff で merge し、task の branch を main まで fast-forward した。push はしていない。

人は 2026-11-17 に repair-1 の追加の policy の差分（`engine.py` の `_OTHER_LINE_BREAKS` と `_content_lines` の条件 1 つ、試験 2 件、ADR 付記 A1）も承認した（決定 repair-1-policy-merge、operator が中継）。operator が task の branch を main に no-ff で merge し、task の branch を main まで fast-forward した。

## 変更
- `health-policy-allowlist-shrink: 許可の一覧から行を消すだけの変更を policy.changed に数えない ADR を書き、契約 ADR D2 に付記する` — [ADR 2026-11-16-health-policy-allowlist-shrink](../adr/2026-11-16-health-policy-allowlist-shrink.md) と [ADR 2026-11-07-repo-health-contract](../adr/2026-11-07-repo-health-contract.md) の付記 2026-11-16。実装より先の commit。
- `health-policy-allowlist-shrink: profile の policy_allowlists に当たる許可の一覧の行を消すだけの変更を policy.changed に数えない` — profile の欄 `policy_allowlists`（`health/profile.py`）、判定 `policy_changed_files`（`health/engine.py`）、rule が engine の結果だけを読む（`rules/policy_changed.py`）、Celeris の profile の値（`profiles/celeris.toml`）、試験 10 件（`scripts/tests/test_repo_health_allowlist_shrink.py`）。
- `health-policy-allowlist-shrink: WorkUnit allowlist-shrink の進捗に変更・試験・判定を外した確認の結果を書く`、`health-policy-allowlist-shrink: 進捗に fmt・clippy・test-affected の結果を書く`、`health-policy-allowlist-shrink: WU の health check が policy の branch では通らない理由を進捗に書く` — WU の進捗（[allowlist-shrink](2026-11-16-health-policy-allowlist-shrink/allowlist-shrink.md)）。
- `integrate 01M7KATBGC: 許可の一覧から行を消すだけの変更を policy.changed に数えない（policy の branch、人の承認と operator の hand merge）` — operator が main に入れた no-ff の merge。
- `integrate 01M7KATBGC repair-1: '\n' 以外の改行の文字を含む許可の一覧は比べずに policy.changed に数える（policy の branch、人の承認と operator の hand merge）` — repair-1 の 3 commit を operator が main に入れた no-ff の merge。
- `health-policy-allowlist-shrink: 進捗を閉じ、policy の差分の成果物の要点と提出時の検査の結果を書く` — この進捗と、WU の進捗の status を done にする変更（WorkUnit closeout の 1 回目の run）。
- `health-policy-allowlist-shrink: hand merge の後の再検査の結果を進捗に書く` — 証拠を merge の前（policy の branch）と提出時の HEAD（merge の後）に分け、is-ancestor の確認を足す（WorkUnit closeout の replan の後の run）。
- `health-policy-allowlist-shrink: '\n' 以外の改行の文字を含む許可の一覧は比べずに数える付記 A1 を ADR に足す`、`health-policy-allowlist-shrink: '\n' 以外の改行の文字を含む許可の一覧は比べずに policy.changed に数える`、`health-policy-allowlist-shrink: repair-1 の修復と検査の結果を進捗に書く` — WorkUnit repair-1（reviewer の指摘の修復。[repair-1](2026-11-16-health-policy-allowlist-shrink/repair-1.md)）。
- 成果物（repo の外）: `allowlist-shrink-policy-diff.md`（前と後の判定の表、対象から外れる条件、再現の command と exit code、E の例、dispatcher への効き方）と、再現の script `allowlist-shrink-policy-diff-repro.sh`・その出力 `allowlist-shrink-policy-diff-repro.out.txt`。WorkUnit closeout の成果物の dir と task の成果物の dir の両方に置いた。

## 判定の前と後（成果物の表の要点）
前 = merge の第 1 親（WU が走ったときの `git merge-base HEAD main`）の検査器と profile、後 = HEAD の検査器と profile。同じ編集をした head に当てた。
- false になる（後だけ）: `human-text-lint.allow` から E の 3 行を消す、`adr-allowed-duplicates.txt` から 1 行を消す、空行と列 0 の `#` の行だけを足す、E と同じ形（3 行の削除と policy でない file の変更）。
- 前も後も true: 1 行足す、1 行書き換える、`adr-allowed-duplicates.txt` を消す、profile から行を消す、検査器のコード（`rules/policy_changed.py`）に行を足す。
- 後の検査器に前の profile（`policy_allowlists` が無い）を読ませると、9 場合とも前と同じ true。前の検査器に後の profile を読ませると `profile_invalid`（未知の項目）で exit 2。
- dispatcher の `policy_mixed`（D6.1）と D6.4 の人の承認は report の `policy.changed` が true のときだけ働く。false になる上の変更は、どちらの対象からも外れる。

## 証拠: 提出時の HEAD（repair-1 の merge の後）で実行
基準は `git merge-base HEAD main`。repair-1 の merge の後、main との差分はこの進捗の 2 file（この file と [repair-1](2026-11-16-health-policy-allowlist-shrink/repair-1.md)）だけ。下の試験数・結果は repair-1 の merge の後に再実行したもの（closeout の時点の 109 件から repair-1 の 2 件が増えた）。
- `bash scripts/dev/check-health.sh --out <tmp>` → exit 0。`mode=doc-only`、`policy.changed=false`、verdict pass、fail 級 0（finding 140 件は全部 existing）。必須 rule 15 個のうち 11 個が pass、4 個（`deps.violation`・`deps.cycle`・`size.responsibility-growth`・`source-size`）が doc-only の not_applicable で、pass か not_applicable でない必須 rule は無い。
- `bash scripts/dev/test-affected.sh` → exit 0（冒頭の `check-health.sh` は doc-only で exit 0、`test-affected: 試験対象の変更なし（docs のみ等）`）。
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` → exit 0（Ran 111 tests、OK）。
- reviewer の再現（base = HEAD、head = `human-text-lint.allow` に `#note<CR>slop_vocabulary<TAB>NEW-SNIPPET-NOT-IN-BASE<TAB>smuggled` の 1 行を足した一時 commit。作業ツリーは変えない）: `repo_health.py --rules policy.changed` → exit 1、`policy.changed=True`、rule `policy.changed` は fail。
- `sh scripts/dev/tests/repo_health_gate.sh` → exit 0（`repo_health_gate: all checks passed`）。
- task の受け入れ 4 の式（base = `git merge-base HEAD main`、`policy.changed` 以外の必須 rule が pass か not_applicable）→ `[]`・exit 0（`policy.changed=false`）。
- `git log --format=%s $(git merge-base HEAD main)` に件名「integrate 01M7KATBGC: 許可の一覧から行を消すだけの変更を policy.changed に数えない（…）」と「integrate 01M7KATBGC repair-1: '\n' 以外の改行の文字を含む許可の一覧は比べずに policy.changed に数える（…）」の両方がある。
- `git merge-base --is-ancestor HEAD main` → この進捗の commit を足す前の HEAD（repair-1 の merge）で exit 0。進捗の commit を足した提出時の HEAD では、その commit が main に入るまで exit 1 になる（closeout と同じ。受け入れ 8 は operator が task の branch を main に merge するか fast-forward した後に通る）。
- `git diff --name-only $(git merge-base HEAD main) HEAD` → `agent-docs/progress/2026-11-16-health-policy-allowlist-shrink.md` と `agent-docs/progress/2026-11-16-health-policy-allowlist-shrink/repair-1.md` だけ。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → exit 0。
- 成果物 `allowlist-shrink-policy-diff.md`・`allowlist-shrink-policy-diff-repro.sh`・`allowlist-shrink-policy-diff-repro.out.txt` が task の成果物の dir にあることを確かめた（作り直していない）。
- cargo の試験は走らせていない（この task は Rust・web を変えていない）。

## 証拠: merge の前（policy の branch）の結果
基準は merge の第 1 親（merge の前の `git merge-base HEAD main`）。今の HEAD の結果とは別に、policy の branch で何が起きたかの記録として残す。
- 再現: `bash allowlist-shrink-policy-diff-repro.sh . "$TMPDIR/allowlist-repro"`（成果物の dir の script）→ exit 0。9 場合の結果は上のとおり。後の report 9 件は `validate_report: 9 valid, 0 invalid`。E の例（base = HEAD、head = `human-text-lint.allow` から 3 行を消す）は `changed=false exit=0`、head の blob は E の branch の同じ file の blob と同じ。
- `bash scripts/dev/check-health.sh --base <merge の第 1 親> --out <tmp>` → exit 1。必須 rule 15 個のうち 14 個が pass、`policy.changed` だけが fail。finding 709 件のうち fail 級は `policy.changed` の 1 件（new 1、worsened 0、existing 708）。evidence は `engine.py`・`profile.py`・`celeris.toml`・`rules/policy_changed.py`。差分に ADR・進捗・試験も入るので D6.2 の「変更 path が全部 `policy.files`」に当たらず、`policy review required` の exit 0 にならない。
- `bash scripts/dev/test-affected.sh` → exit 1（WU allowlist-shrink の run、merge の前）。冒頭の `check-health.sh` が上の `policy.changed` の fail で止まった。先例 01M76BYGKY と同じで、人の承認と operator の merge で通した。
- task の受け入れ 4 の式（`policy.changed` 以外の必須 rule が pass か not_applicable）→ base = merge の第 1 親で `[]`・exit 0（`policy.changed=true`）。

## repair-1（2026-11-17）: `'\n'` 以外の改行の文字の抜け道を塞ぐ
- 指摘（reviewer 01M7KJH9BREHTYP8GWC2M2ZWJW、受け入れ 5 が不合格）: `human-text-lint.allow` に `#note<CR>slop_vocabulary<TAB>NEW-SNIPPET-NOT-IN-BASE<TAB>smuggled` を 1 行足すと、engine は `'\n'` だけで分けて注釈の 1 行として除き `policy.changed=false`、text mode で読む `check-human-text.py` には新しい許可が入った。
- 直し: [ADR 付記 A1](../adr/2026-11-16-health-policy-allowlist-shrink.md) を先に commit し、`engine.py` の `_content_lines` が `str.splitlines` の区切りのうち `'\n'` 以外の文字（9 個）を含む内容を None（比べずに数える）にした。試験 2 件を足した。詳細と証拠は [repair-1](2026-11-16-health-policy-allowlist-shrink/repair-1.md)。
- 人の承認と merge: `engine.py` は policy の file なので、この差分も `policy.changed=true` になった。人が 2026-11-17 に成果物 `allowlist-shrink-policy-diff.md` の「追加の policy の差分」節で承認し（決定 repair-1-policy-merge）、operator が task の branch を main に no-ff で merge した（件名「integrate 01M7KATBGC repair-1: …」）。merge の後、main の `human-text-lint.allow` と `adr-allowed-duplicates.txt` に `'\n'` 以外の改行の文字は無い（operator が確認）。

## 未解決
- WorkUnit closeout の 1 回目の run は、計画の check（`check-health.sh` の後、必須 rule が pass 以外なら不合格）が merge の後の doc-only の not_applicable で必ず落ちるため、`plan_issue` を申告した。replan の後の check は pass と not_applicable を合格に数え、この run で exit 0 を確かめた。
- LoC 削減 v3 E（01M7J3NKVAH9NG53C5FEFNQJYK）の branch は 2026-11-17 の時点でこの merge を含まない。E は base がこの merge を含む main になった後（review 前の main 同期の後）に後の判定で評価される。同期の後の E の report は未確認。
- `HealthPolicyState.files`（`crates/task-dispatch/src/health_review/input.rs`）は blob の違う policy の file の一覧なので、数えなかった許可の一覧も reviewer の prompt に `changed=false` と並んで出る。`changed=false` では D6.1・D6.3・D6.4 は働かない（ADR D4）。Rust は変えていない。

## 提案
- policy の file と、その試験・ADR・進捗を同じ branch で変える task は、WU の health check（`celeris:repo-health/1`）と task の review の D6.1 で必ず止まる（01M76BYGKY とこの task の 2 件）。計画では、policy の file だけを変える WU を別の段に分ける（D6.2 の PolicyOnly で合格する）か、人の承認と operator の merge を最初から計画の人の check に置く。どちらにするかは人か planner が決める。

## 本番への移行
無し（repo の検査だけ。config・DB・API は変えない）。
