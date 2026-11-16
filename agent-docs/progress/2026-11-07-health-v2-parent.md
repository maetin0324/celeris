---
title: 健全性v2 親 — Git repo 一般の Repository Health/Architecture Review と Celeris の肥大化・意味的重複のリファクタリング（closeout）
tasks: [01M6TYV9DTZESAJJMXRQVGZ56V]
status: done
updated: 2026-11-16
completed: 2026-11-16
---
# 健全性v2 親 — closeout

完了日: 2026-11-16（closeout の WorkUnit の完了。task の完了は final review が決める）

依頼キー `git-health-refactor-20261107-v1`。この親は子 12 件（B0・G1〜G4・R1〜R6・V0）の取りまとめで、実装は子が持つ。
closeout では、子がすべて Done であること、各子の成果が main に入っていること、V0 の最終報告の置き場所を確かめ、この file を書いた。
コードは変えていない。基準は `git merge-base HEAD main`。

## 結論
- 子 12 件はすべて `status: Done`（`celerisctl show <id>`、2026-11-16 に実行）。子孫 task 53 件のうち done 52 件、running 1 件（この親）。
- 各子の branch `celeris/<id>` は main の祖先で、main より先の commit は 0 件（`git merge-base --is-ancestor`・`git rev-list --count main..celeris/<id>`）。
- main を task branch に取り込んだ（`git merge --no-ff --no-edit main`、293 commit）。子 12 件の進捗 file（front matter の `tasks: [<id>]`）は
  main と HEAD の両方にある。
- V0 の最終報告: 進捗 [2026-11-15-health-v2-v0.md](2026-11-15-health-v2-v0.md)（status done）と、人向けの報告 `v0-report.md`
  （V0 の run の成果物。置き場所は下の「V0 の最終報告」）。release 候補は `bae3149de834` で、2026-11-15T23:51:27Z から本番の current。
- 本番昇格・credential・予算・他 repo について、健全性v2 の task と worker は本番昇格・credential の値の変更・予算の増額・他 repo の改変をしていない（根拠は下の節）。

## 子 12 件
| 子 | task id | status | 進捗 file | branch が main の祖先 |
|---|---|---|---|---|
| B0 | 01M6TYVJSPEYA2P8B7G56HSQ73 | Done | [2026-11-07-health-v2-b0.md](2026-11-07-health-v2-b0.md) | はい（先行 0） |
| G1 | 01M6TYVJSRM3SFQ3VHWQE4MZ9M | Done | [2026-11-07-health-v2-g1.md](2026-11-07-health-v2-g1.md) | はい（先行 0） |
| G2 | 01M6TYVJSTYGRPHERB93MM7J59 | Done | [2026-11-07-health-v2-g2.md](2026-11-07-health-v2-g2.md) | はい（先行 0） |
| G3 | 01M6TYVJSWEY3ZE50S02ETYDNM | Done | [2026-11-07-health-v2-g3.md](2026-11-07-health-v2-g3.md) | はい（先行 0） |
| G4 | 01M6TYVJSXBZMSBC0YGAT2TTX0 | Done | [2026-11-10-health-v2-g4.md](2026-11-10-health-v2-g4.md) | はい（先行 0） |
| R1 | 01M6TYVJSZDSCB697193J01FYW | Done | [2026-11-10-health-v2-r1.md](2026-11-10-health-v2-r1.md) | はい（先行 0） |
| R2 | 01M6TYVJT0Z1CBGNK8VTNK8ZJH | Done | [2026-11-10-health-v2-r2.md](2026-11-10-health-v2-r2.md) | はい（先行 0） |
| R3 | 01M6TYVJT2HG9NY5FW9FMCZS9A | Done | [2026-11-10-taskstore-consumer-ports.md](2026-11-10-taskstore-consumer-ports.md) | はい（先行 0） |
| R4 | 01M6TYVJT4S29ZMP6P0D4831JG | Done | [2026-11-11-health-v2-r4.md](2026-11-11-health-v2-r4.md) | はい（先行 0） |
| R5 | 01M6TYVJT53598CPKMGSQGA7XM | Done | [2026-11-10-health-v2-r5.md](2026-11-10-health-v2-r5.md) | はい（先行 0） |
| R6 | 01M6TYVJT7EJ6WWHEF0EBH3JPS | Done | [2026-11-15-health-v2-r6.md](2026-11-15-health-v2-r6.md) | はい（先行 0） |
| V0 | 01M6TYVJT97XH88TEEAVRD7JK6 | Done | [2026-11-15-health-v2-v0.md](2026-11-15-health-v2-v0.md) | はい（先行 0） |

各子の WorkUnit・leaf の記録は、上の file と同名の dir（例 [2026-11-15-health-v2-r6/](2026-11-15-health-v2-r6/)）にある。
G1 の進捗 file の front matter は `status: running` のまま（task は Done。下の「未解決」1）。

## V0 の最終報告
- repo の正本: [2026-11-15-health-v2-v0.md](2026-11-15-health-v2-v0.md)（結論・R1〜R6 の撤去と残しの根拠・純増減・依存と共有状態・test target・
  review の費用・不変条件・本番への移行と rollback・未解決・提案）。release 候補の手順と rollback は
  [2026-11-15-health-v2-v0/release.md](2026-11-15-health-v2-v0/release.md)。
- 人向けの報告 `v0-report.md`: V0 の run の成果物 `/local/celeris/data/workspaces/01M6TYVJT97XH88TEEAVRD7JK6/wu/report/artifacts/v0-report.md`。
  WU の成果物の dir は後で片付けられることがあるので、この closeout の run の成果物にも同じ内容を写した（`v0-report.md`）。

### meaningful simplification の要点（V0 の進捗からの引用）
- 「実質の削減（本体）は R1 −369・R6 −741 の計 −1,110 行。R4・R5 の試験の file 分割（約 1.9 万行の移動）、class の付け替え 1 file（147 行）、
  生成・vendor への移動は削減に数えていない（生成・vendor への移動による本体の見かけの削減は無い）。」
- 「本体のみ（product_only）は B0 の基準から +19,935 行（366,177 → 386,112）。健全性 v2 が実際に減らした本体は R1 −369・R6 −741 の計 −1,110 行で、
  増加の大半は G2・G3・R4 が足した機能（slice の計 +18,884）。」
- 「複数の crate が書く SQLite 表は 3 → 1 に縮んだ（R3）。crate 依存の辺 49・crate 循環 0・module 循環 3 は変わらない。
  複数の書き手の event 種類は 20 → 25 に増えた。全体として縮んだとは言えない。」
- 「test target 352 → 287（−65、R5 の束ね）。」
- 機能の保持: R1〜R6 の撤去・寄せごとに、影響する capability と、機能が残ったことを示す試験を V0 の「機能維持/撤去の根拠（R1〜R6）」の表が並べる。
  例: R1 は 5 adapter の `ResultFile`・`lenient_evidence` の写しを `result_report.rs` などの 1 つに寄せ、共通の `result_contract_` 5 件・`run_common_` 6 件で確かめた。
  R2 は EventSink の browser_* 10 method を `BrowserAuthority`・`BrowserControl` に分けた。R4 は `Dispatcher` の field を 116 → 16 にした。
  R6 は 22 件を消し 4 件を寄せた。R5 が消した試験関数は 1 本だけ（人の決定 `test-removal` の a と mutation の KILLED が根拠）。
- 不変条件: 「対応表 139 行のうち、HEAD の試験で 133 行が通り、release gate で 4 行を確かめた。未検証は 2 行
  （`no-unwrap-outside-tests` と `cap:delivery-merge-candidate#1`）。落ちた不変条件の試験は無い。」
- release gate（`bae3149de834`）: 全段 exit 0、nextest 8,581 件合格・失敗 0、web の段 5 つ合格。

## 本番昇格・credential・予算・他 repo の改変
- 本番昇格: 健全性v2 の task と worker は promote していない。R6 を含む `bae3149de834` の昇格は、外部 operator
  （`actor: external_operator:claude-code-ops-session`）が人の永続の許可（`policy_ref: memory:promote-without-asking`）に基づき
  2026-11-15T23:51:27Z に行った（`/local/celeris/state/releases/bae3149de834/promoted.json`、読み取りだけで確かめた）。
  B0 の配送の失敗の後の昇格（人の決定 promote-fix に従った外部 operator の操作）は B0 の進捗の「配送の失敗」節にある。
  この closeout は release・本番 host・本番 DB に書いていない。
- credential: 子孫 53 task の events に credential の操作の event は無い（承認の event は下の policy の承認 5 件だけ）。
  コードでは R2 の launcher-login-fix が credentiald の最初の要求の待ちを直した（ADR
  [2026-11-11-credentiald-passed-stream-wait.md](../adr/2026-11-11-credentiald-passed-stream-wait.md)）。これは credential の path class のコード変更で、
  credential の値・登録は変えていない。
- 予算: 子孫 53 task の events に予算の欄を変える event は無い。本番 DB の `config_events`（6 件）に 2026-11-07〜16 の予算・上限の変更は無い。
  案件の予算の上限を config 以外の経路で変えたかは未確認。V0 の報告によると、健全性v2 の Claude の費用は計 1,350 USD。
- 他 repo: 子孫 53 task のうち 48 件の repo は agent-platform だけで、残り 5 件は repo を持たない承認 task。G4 の非 Celeris の検証は
  isolated な Git fixture で行った（G4 の進捗）。実ユーザーの他の repo は変えていない。
- policy だけの branch の承認 5 件（profile の `[review]` 節 2 件、`--print-review-policy` 1 件、G4 の期限付き例外 2 件）は、
  `operator:claude-code`（人の委任で受信箱を受け持つ外部 operator）が `approval_decided approved=true` で承認した。

## 変更（commit の件名）
- `Merge branch 'main' into celeris-wu/01M6TYV9DTZESAJJMXRQVGZ56V/closeout` — main の取り込み（衝突なし）。
- `health-v2 親 closeout: 子 12 件の取り込みを確かめ、親の進捗 file を書く` — この file を足した。変更は `agent-docs/progress/` の下だけ。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、WU は `$CELERIS_WU_BASE`）
- `celerisctl show <id>`（子 12 件）→ 12 件とも `status: Done`。
- `git merge --no-ff --no-edit main` → 衝突なし。`git merge-base --is-ancestor main HEAD` → exit 0。
- `git merge-base --is-ancestor celeris/<id> main`（子 12 件）→ 12 件とも exit 0。`git rev-list --count main..celeris/<id>` → 12 件とも 0。
- `git grep -l "^tasks:.*<id>" main -- agent-docs/progress` と同じ command の `HEAD` 版 → 子 12 件とも main と HEAD で同じ file の一覧（上の表の file を含む）。
- 本番 DB の読み取り（python3 の sqlite3、`mode=ro`）: 子孫 53 task の status と repo、events の種類、`config_events`。
- 検査の結果は下の節。

## 検査の結果（提出時の HEAD で実行）
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (448 files)`）。
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。
- `python3 scripts/dev/check-human-text.py --root .` → exit 0（0 findings）。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh`（基準 `git merge-base HEAD main`。`CELERIS_WU_BASE` を外して実行）→ exit 0、docs だけで試験対象なし。
  `$CELERIS_WU_BASE` を基準にすると、objective の手順 2 で取り込んだ main の変更（293 commit）が差分に入り、`--list` は task-api・task-core・
  task-dispatch・task-ops・task-worker・routing-eval ほかと web を対象にする。この WU 自身の変更（`git diff --name-only main HEAD`）はこの file の 1 path だけ。
- `cargo clippy --workspace -- -D warnings` → 結果は報告の本文に書く。
- 範囲: `git diff --name-only main HEAD` → この file の 1 path。`sh "$CELERIS_WU_SCOPE_PATHS"` は main の取り込みを含むので 458 path を出す
  （取り込んだ main の path とこの file）。

## 未解決
1. G1 の進捗 file の front matter が `status: running`・`updated: 2026-11-07` のまま（task は Done、branch は main の祖先）。G1 の記録の更新は
   この closeout の範囲外として変えていない。
2. 親 task の受け入れ条件 0 の「web e2e が通る」は文字どおりには満たしていない。web の e2e の 18 件は健全性v2 の基準 `5923a76c9aa0` でも落ちる
   既存の不合格で、修正と release gate への追加は task 01M7H1RNG5C5AA76HNB2M9M098 が扱う（V0 の進捗、人の決定）。
3. V0 の未解決 1〜6（scratch の target の引き継ぎの v8 の link-search、gate.json の `cargo_test.passed`、不変条件の未検証 2 行と一部だけの 12 行、
   複数の書き手の event 種類の増加と `route_judgments`、R4 と R5 の帰属の重なり）は V0 の進捗のとおり残っている。
4. 案件の予算の上限を config 以外の経路で変えたかは未確認（上の「予算」）。

## 提案
1. V0 の提案 1（scratch の target の引き継ぎと seed の写しの恒久の直し、owner は `crates/task-worker/src/scratch.rs` の `allocate_locked` と
   `scratch/reflink.rs` の `seed_target`）を別 task にする。v8 の link-search の失敗は G4・R5・V0 で繰り返し出ている。
2. V0 の提案 3（`scripts/dev/test-parallel.sh` の summary の読み取りの直し）を別 task にする。
3. G1 の進捗 file の front matter を `status: done` と完了日に直す（G1 の記録の持ち主が直す）。`progress-index.sh --check` が
   task の status と front matter の食い違いを検出するようにするかは、別に判断する。
4. V0 の提案 4〜7（不変条件の決定的な検査、計測、共有状態、検査器の契約）は次の健全性の段の材料にする。
