---
title: LoC 削減 v3 親 — closeout（固定計測で累計 production 純減 11,239 行を再計測、G の最終報告と一致）
tasks: [01M7J3N68JRNTP5Q4G934QWF36]
status: done
updated: 2026-11-17
completed: 2026-11-17
---
# LoC 削減 v3 親 — closeout

完了日: 2026-11-17

依頼キー loc-reduction-v3-20261116（人の依頼 chat message 01M7J34SXA8MTTA5NNBQD3424J、2026-11-16）の親。
親は削減を実装していない。子 A〜G がすべて Done になった後に、固定計測による累計の再計測、G の最終報告との照合、この進捗ファイルの commit だけを行った。

## 結論
- 最終目標を実測で満たした。開始時 main `415a19a7004c` から提出時の HEAD までに、production 全言語は **11,239 行**、production Rust は **9,815 行**、own_total_with_tests は **17,802 行** 減った。受け入れの size check（`- - 10000 1 1`）は exit 0。
- 親の再計測は G の最終報告（run 01M7MH0TQZ7YC62B28PTTNN131 の `artifacts/final-report.md`）の数値と一致した（production・Rust・own の差 0）。
- 統合後の release の gate: C〜F をすべて含む release `c9d94350740e` の gate（`scripts/selfdeploy/release.sh`）は全段合格した。G の 2 commit（試験の統合、production −3 行）はまだどの release にも入っておらず、release gate は未実施。
- 親は本番への昇格をしていない。`c9d94350740e` は 2026-11-17T11:22:07Z に external operator（`claude-code-ops-session`、`promoted.json` の actor）が昇格した。
- 20〜30 万行の中長期の目標は達成していない。健全性 v2 の前（B0、production 366,177）と比べると、HEAD はまだ +8,922 行多い。

## 子の状態（`celerisctl show <id>` で確認。7 件すべて Done）
| 子 | task | 結果 | 最終 review |
|---|---|---|---|
| A 監査 | 01M7J3NKV25NNRQ2R6VPWM16H9 | 候補 21 件、本体の変更なし | 全条件 pass |
| B 目的の伝達・純減の判定 | 01M7J3NKV5R4XFVSDEWDQ4MF22 | production 0、own +412（試験） | 全条件 pass（advisory 1: ADR-0072 に付記なし） |
| C pilot | 01M7J3NKV7PY9DPT0K9D1VX4B7 | production −2,701 | 全条件 pass |
| D task-core・task-ops | 01M7J3NKV8E6P9WAWGMJR3KB1F | production −2,558 | 全条件 pass（advisory 1: api.md の store 名） |
| E task-dispatch・celeris | 01M7J3NKVAH9NG53C5FEFNQJYK | production −1,753 | 全条件 pass（advisory 1: task-core の残り） |
| F task-worker・task-api | 01M7J3NKVC2KQ4NGJ4GESHJV0B | production −4,224 | 全条件 pass（advisory 2: api.md・SKILL.md の Console の記述） |
| G 試験の整理・最終評価 | 01M7J3NKVEDX67N19WBRME0CRN | production −3、own −180 | 全条件 pass（advisory 1: store/tests.rs の schema 検査の写し） |

## 再計測（受け入れ条件 2 の command。分類規則は開始時 main の source-size-report.py・toml に固定。提出時の HEAD で実行）
| 指標 | 415a19a7（固定の基点） | HEAD | 純減（fixed） | G の最終報告 | 差 |
|---|---:|---:|---:|---:|---:|
| production 全言語 | 386,338 | 375,099 | 11,239 | 11,239 | 0 |
| production Rust | 340,223 | 330,408 | 9,815 | 9,815 | 0 |
| own_total_with_tests | 1,043,479 | 1,025,677 | 17,802 | 17,802 | 0 |

unit（`git merge-base HEAD main` → HEAD）の純減は 3 指標とも 0（親の差分はこの進捗ファイルだけで、docs に数える）。
HEAD の class: external_test 455,363・inline_test 21,408・generated 43,822・vendor 111,936・fixture 36,305・evaluation_infra 145,206・tooling 25,253・docs 590,462・schema 66,352・migration 3,348・lockfile 10,090。
vendor・fixture・migration・lockfile は基点と同じで、production の行を vendor などへ付け替えた分は無い。docs は G の報告（590,402）より 60 行多い。G は試験の統合の commit で測り、その後に G の進捗ファイル（60 行）が入ったため。

### 区間の再測定（main の first-parent の境目の commit。すべて HEAD の祖先）
| 区間（最後の commit） | production | Rust | own_total_with_tests |
|---|---:|---:|---:|
| A（`ef0aa0a25a4f`） | 0 | 0 | 0 |
| B（`e19056a1dfb9`） | 0 | 0 | +412 |
| C（`92df7b85929b`） | −2,701 | −2,701 | −2,838 |
| D（`2244180ad42b`） | −2,558 | −2,558 | −3,507 |
| 無関係: 01M7KATBGC health-policy-allowlist-shrink（`8024be8c1adf`） | 0 | 0 | +261 |
| E（`a6a82aff767d`） | −1,753 | −1,753 | −4,229 |
| F（`c9d94350740e`） | −4,224 | −2,800 | −7,721 |
| G 試験の統合（`bcddc8aea01c`） | −3 | −3 | −180 |
| G 進捗ファイル（`ad00b8cf1d64`） | 0 | 0 | 0 |
| 計 | −11,239 | −9,815 | −17,802 |

区間の和は固定計測と一致する（差 0）。`git log --first-parent 415a19a7004c..HEAD` の 87 commit のうち、件名が `loc-reduction-v3` でないものは 01M7KATBGC の 4 件だけで、production は変えていない。

## 安全不変条件（受け入れ条件 1）
- 子の受け入れ: C〜G の最終 review はすべての条件を pass にした。各子は提出時の HEAD で test-affected・clippy・fmt を exit 0 で通した（D: nextest 8,529 passed、E: 3,925 passed・反例の試験 287 passed、F: nextest 8,446 passed・web unit 705・名指しの e2e、G: nextest 8,436 passed・web 705・repo-health new 0）。
- 状態遷移・replay・atomicity: D は名指しの replay・atomicity・P4-A の隔離の試験 72 件を通した。E は lease・restart・replan・stale-result の反例の試験を残して通した。
- 試験の検出能力: G は統合・削除した試験 4 組ごとに失敗注入を行い、残る試験が FAIL することを確かめた（G の進捗の対応表）。
- 認可・秘密: F は認可と provider ごとの差を残した。review で差し戻された 3 件（legacy スレッドへの通知の写し、消した route を開く e2e、run ログの 360px の試験）は F の試行 3・4 で直した。
- DB のデータ: 削除は呼び手の無い入口・写し・互換の経路で、migration・events の書き換えは無い（migration の行数は基点と同じ 3,348）。
- 統合後の release の gate: release `c9d94350740e`（C〜F を含む。F の最後の commit が HEAD）の `gate.json` は ok=true、failed_step 無し。nextest 8,490 passed・22 skipped（doctest exit 0、userns あり）、cargo-clippy・cargo-fmt-check exit 0、web unit 101 file・705 passed、web e2e 295 passed、browser の台帳 ok。`verify.json` も ok（migrate・件数の一致・主要 GET・N-1 互換・smoke）。
- G の 2 commit（試験 −177 行と使われない再 export 1 個の削除）は release gate を未実施。次の release で人か operator が `release.sh` を実行して合否を確かめる。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 受け入れ条件 2 の size check（`- - 10000 1 1`）→ exit 0、`OK []`。純減 production fixed 11,239・Rust fixed 9,815・own fixed 17,802、unit は 3 指標とも 0。
- 区間の再測定（上の表、開始時 main の script・toml で 10 点）→ 区間の和が固定計測と一致。
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし・docs のみ。repo-health verdict=pass、new 0・worsened 0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → ok。
- release `c9d94350740e` の `gate.json`・`verify.json`・`promoted.json`（読み取りだけ）→ gate ok=true、verify ok=true、昇格は external operator。

## 変更（commit の件名）
- `loc-reduction-v3 親: closeout の進捗ファイルを足す（固定計測の再計測と G の最終報告の照合、release gate の結果）`

## 未解決
- G の 2 commit を含む release の gate は未実施（人か operator が次の release で実行）。
- 子が残した撤去の残り（下の提案 1〜3）は、どの子の write set にも入らなかったので残っている。

## 提案（人が起票を決める）
1. task-core の残り（E の未解決。責任者は「task-core を write set に持つ後続の task」）: 呼び手の無い `report.rs` の `is_cluster_login_needed_headline`、`notify.rs` の `notification_scan_at`・`notification_scan_mark`（試験 `migration_0019_resumes_notification_scan_from_the_existing_ledger` だけが呼ぶ）を消す。消した仕組みを指す doc comment 3 か所（task-core `notify.rs` の `NotificationKind::ClusterLoginNeeded`、task-ops `daemon.rs` の `script_tick_wait`、task-ops `root_done.rs` の `RootDoneSummary`）を直す。生成 schema の作り直しを伴う。
2. 文書の残り（D・F の review の advisory）: `docs/api/v1/api.md` の 2762〜2763 行（Console を現行の画面として説明し `POST /console/instruct` を案内）、412 行（store 名 `project_set_status`。本体は `project_set_status_tx`）、4096 行付近の型の所有表の `console.rs`、`config/skills/cos-operator/SKILL.md` の 52 行の `console`。
3. `GET /api/v1/inbox` の撤去（F の未解決、承認済み）: `scripts/selfdeploy/verify.sh` の probe（高 risk の selfdeploy）の変更と同時に行う。
4. 人の判断が要る候補: 未配線の `run_tmpdir::sweep_stale`・`browser_supervisor::reap_on_start`・`source_state_view`、未配線の `AdapterPolicy::ModelFamily` の選択関数（D）、C7 旧 harness 設定の互換 `from_legacy`（先に ADR で実行契約の置き場を決め、本番 config を移す）、C8 `[scratch]` 必須化。
5. 試験: `browser_h3_wire.rs` と `browser_injection_wire.rs` の起動の道具の共通化（約 400〜500 行、release gate でしか走らないので release で確かめる task にする）。
