---
title: docs/ を人向け（docs/）と agent 向け（agent-docs/）に分け、並列 task でも衝突しない記録の形にする
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-02
---
# docs/ を人向け（docs/）と agent 向け（agent-docs/）に分け、並列 task でも衝突しない記録の形にする

完了日: 2026-10-02。方式は [ADR-0128](../adr/0128-docs-layout.md)。分類と移動先の一覧は `scripts/dev/docs-layout.tsv`
（human 28・agent 164・delete 14 行。移動 176 件）。段ごとの WorkUnit の記録は [2026-10-02-docs-layout/](2026-10-02-docs-layout/move-docs.md) の下にある。

## 結果

- `docs/`（人向け）: 31 ファイル。SPEC.md・architecture-map.md・api/・protocol/・guides/（8 本）・ops/（5 本）と、移行期間の案内 `docs/adr/README.md`・`docs/progress/README.md`。
- `agent-docs/`（agent 向け）: 184 ファイル。ADR・進捗・報告・作業規則。入口は [agent-docs/README.md](../README.md)。
- 進捗は task ごとのファイル（D3）、索引は `sh scripts/dev/progress-index.sh` で生成（D4）、ADR は番号でなく日付+slug（D5）。
- 検査台本: `check-doc-links.sh`・`check-doc-layout.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`（自己試験は `scripts/dev/testdata/`）。
- `docs/DESIGN.md` は人の決定（design-md）どおり削除し、CLAUDE.md の参照は SPEC.md へ変えた。

## land-verify: main の取り込み（ADR-0128 D6）

main `f1904ecd` を merge した（merge commit `48cd7afa`、親 `675a9c96` と `f1904ecd`）。衝突は `docs/architecture-map.md` の 1 件で、
新パス側の表を採り、main が足した launcher の行（`browser_launcher/mod.rs`・ADR-0115/0116）を新パスで足した。

task の base `0d438ec1` 以降に main が旧配置へ足したものを、次のように移した（tsv の末尾 9 行にも記録）:

| 旧（main が足した場所） | 新 |
|---|---|
| `docs/adr/0116-browser-launcher-implementation.md`・`0125-deterministic-time-tests.md`・`0127-skills-native-delivery.md` | `agent-docs/adr/` の同名 |
| `docs/progress/time-dependent-tests.md` | `agent-docs/progress/2026-10-02-time-dependent-tests.md` |
| `docs/progress/time-dependent-tests-{dispatch,injection,kill}.md` | `agent-docs/progress/2026-10-02-time-dependent-tests-fix/{dispatch,injection,kill}.md` |
| `docs/progress/phase-skills-progressive.md` | `agent-docs/progress/2026-10-02-skills-native-delivery.md` |
| `agent-docs/PROGRESS.md` に入った節「browser: ADR-0115 権限分離 launcher」「browser: ptrace 境界分離 launcher 実装・実 process 実証完了」（land-main・land-main2・land-main3・pick-chrome を含む） | `agent-docs/progress/2026-10-02-browser-launcher.md` |
| 同「codex・opencode への skill の付属ファイルと段階的な読み込み」 | `agent-docs/progress/2026-10-02-skills-native-delivery.md` の末尾 |
| 同「release 準備失敗の切り分け（0d438ec1）」 | `agent-docs/progress/2026-10-02-release-prepare-failure.md` |
| 同「時間依存試験の決定化」（(6)・人が実行する手順・実環境での確認を含む） | `agent-docs/progress/2026-10-02-time-dependent-tests-fix.md` |
| `docs/ops/browser-launcher-host-setup.md` | 移さない（人が行う host 準備手順なので docs/ops/ に残し、docs/README.md の一覧に足した） |

移した節の本文は変えず、相対リンクだけ新配置へ直した。各ファイルに D3 の front matter を付けた。
main が `agent-docs/PROGRESS.md` の既存行（prompt-rule 節の引用）を書き換えた 1 行は main の版のまま残した。

## 削除したファイル（D9）

内容は `git show <最後の commit>:<旧パス>` で取り出せる。

| 旧パス | 段（WU） | 理由 | 最後の commit |
|---|---|---|---|
| `docs/DESIGN.md` | move-docs | 人の決定（design-md）で削除。仕様の入口は docs/SPEC.md、CLAUDE.md の参照もそちらへ | `ea69c29a` |
| `docs/gui/bootstrap/CLAUDE.md` | move-docs | 初期配布用の作業規則。現行は gui/CLAUDE.md にあり重複。人の確認で削除 | `926e19c0` |
| `docs/gui/bootstrap/GOAL_TEMPLATE.md` | move-docs | 初期配布用テンプレート。現行は gui/docs/GOAL_TEMPLATE.md にあり重複。人の確認で削除 | `926e19c0` |
| `docs/gui/bootstrap/PROGRESS.md` | move-docs | G0〜G5 を未着手とする初期状態。現行の進捗は gui/docs/PROGRESS.md。人の確認で削除 | `926e19c0` |
| `docs/gui/bootstrap/README.md` | move-docs | run-gphases.sh による gui/ 作成を指示するが gui/ は実装済み・台本は無い。人の確認で削除 | `926e19c0` |
| `docs/gui/bootstrap/agents/auditor.md` | move-docs | 初期コピー元。現行は gui/.claude/agents/auditor.md。人の確認で削除 | `926e19c0` |
| `docs/gui/bootstrap/agents/implementer.md` | move-docs | 初期コピー元。現行は gui/.claude/agents/implementer.md。人の確認で削除 | `926e19c0` |
| `docs/ops/adr-0079-r5b-runbook.md` | move-docs | ADR-0079 R5b の一度きりの作業手順。人の確認で削除（JSON フィクスチャは crates/task-api/tests/fixtures/ へ切り出し） | `243009c3` |
| `docs/ops/home-nfs-migration-2026-09-25.md` | move-docs | 2026-09-25 の一度きりの移行手順。人の確認で削除 | `d0e78511` |
| `docs/web/dogfood.md` | move-docs | 一度きりの作業文書（手順と未決定の記録が混在）。現行の運用は docs/ops/web-parallel-operation.md。人の確認で削除 | `c63d53c2` |
| `docs/execution-architecture-2026-09-24.md`（移動後 `agent-docs/reports/execution-architecture-2026-09-24.md`） | cleanup-reports | ADR-0072 前の現状調査。Task:run 1:1・dispatcher.rs の行番号が今の実装と食い違う | `a42f9a54` |
| `docs/execution-decomposition-report-2026-09-25.md`（移動後 `agent-docs/reports/…`） | cleanup-reports | E6 時点の分析。挙げた問題は Phase F で実装済み、gate 提案は ADR-0079 で置き換わった | `a42f9a54` |
| `docs/execution-parallel-report-2026-09-28.md`（移動後 `agent-docs/reports/…`） | cleanup-reports | 案件計画（ADR-0074 D3）は ADR-0079 で廃止。決定は ADR-0074 本文にある | `a42f9a54` |
| `docs/notes/build-cache-tiering-input-2026-09-28.md`（移動後 `agent-docs/notes/…`） | cleanup-reports | ADR-0075 の入力メモ。方針は ADR-0075 と実装に入り、現状の記述（sccache 未導入など）は食い違う | `a42f9a54` |

統合（ファイルは残る）: `docs/celeris-api-v1.md` → `docs/api/v1/overview.md` の中身は cleanup-api で `docs/api/v1/gui-api.md` §3.125 へ統合した。
`overview.md` は gui/docs/celeris-api-v1.md（変更禁止）の参照を保つ転送ページとして残る（統合前の最後の commit `a42f9a54`）。
guides・ops・api・protocol の各文書は節の削除・修正だけで、ファイルの削除は無い（cleanup・cleanup-ops・cleanup-api の記録）。

## 検証（land-verify、HEAD = merge 後）

| コマンド | exit | 要点 |
|---|---|---|
| `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok`（全体） |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | `check-doc-layout: ok`（merge 前は cleanup-reports で消した 4 本の行が agent のままで 4 件違反。delete に直した） |
| `sh scripts/dev/check-adr-numbers.sh` | 0 | `ok (116 files)` |
| `sh scripts/dev/progress-index.sh --check` | 0 | `ok` |
| `python3 scripts/dev/check-architecture-map.py` | 0 | 191 件のパスを確認 |
| `cargo fmt --all -- --check` | 0 | 差分なし |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告なし |
| `cargo test -p task-ops --lib` | 0 | 400 passed |
| `cargo test -p task-api --lib` | 0 | 72 passed、2 ignored |
| `cargo test -p task-worker --lib` | 101 | 703 passed、7 failed、4 ignored。失敗は `browser::tests` の 7 件だけで、全て `isolated_runtime_unavailable`（run の sandbox では隔離 browser runtime を起こせない環境要因。この task の crates の差分は docs パスの文字列だけ） |

workspace 全体の `cargo test --workspace` は daemon の workspace check に任せた（Objective どおり）。

## 未解決

- `cargo test -p task-worker --lib` の `browser::tests` 7 件は、隔離 runtime が使える環境（daemon の workspace check）で通ることを確かめる必要がある。
- `deploy/systemd/celeris-web@.service` のコメントが旧パス `docs/web/parallel-operation.md` を指したまま（cleanup-ops の提案。deploy/ は昇格で sha12 確認になるため未変更）。
- `gui/docs/celeris-api-v1.md`（gui/ 側の写し）は正本とずれている（refs-repo の記録）。gui/ はこの task では変えない。
- `docs/ops/selfdeploy.md` §4e（一時回避の撤去手順）は、本番の drop-in が残っているので残した。人が撤去したら消してよい。
- `gui/app/` のコメントは gui/docs/ と旧 `docs/DESIGN.md` を指す（gui/ の範囲外）。

## 提案

- 移行期間の終わり（merge-base が move-docs の merge より前の `celeris/*` task branch が無くなったとき）に、次を後続 task で行う:
  - 旧ディレクトリの案内 `docs/adr/README.md`・`docs/progress/README.md` を削除する。
  - `agent-docs/PROGRESS.md` と `agent-docs/progress/phase-F.md` の壊れたリンクを直し、`check-doc-links.sh` の対象外（`MIGRATION_EXCLUDE`）から外す。
    同時に `scripts/dev/check-adr-numbers.sh` も対象外から外し、旧ディレクトリの列挙を消す（refs-repo の提案）。
  - それまでに main に入った旧配置の ADR・進捗・PROGRESS.md 末尾の節は、land する task が本ファイルの「land-verify」節と同じ規則で移す。
- `deploy/systemd/celeris-web@.service` のコメントを `docs/ops/web-parallel-operation.md` へ直す（昇格の sha12 確認が要る）。
- `crates/task-worker/src/scratch/tests.rs:835` の ETXTBSY は `crate::test_support::write_executable` で書くよう直す（cleanup-ops の提案）。

## sync-main-2: main の取り込み（5d6df9f3）

main `5d6df9f3` を取り込んだ。衝突は `agent-docs/PROGRESS.md` の時間依存試験節のみ。main が旧 `docs/PROGRESS.md` に追加した原因・方式の 6 行を、ADR-0128 D6 に従い `agent-docs/progress/2026-10-02-time-dependent-tests-fix.md` の該当節へ移した。凍結した `agent-docs/PROGRESS.md` には追記せず、`docs/PROGRESS.md`・`docs/DESIGN.md` は復活させていない。

| 文書検査 | exit |
|---|---:|
| `sh scripts/dev/check-doc-links.sh` | 0 |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 |
| `sh scripts/dev/check-adr-numbers.sh` | 0 |
| `sh scripts/dev/progress-index.sh --check` | 0 |

build cache 判定: **残っている**。`crates/task-dispatch/src/dispatcher/tests/mod.rs:3026` の sccache 系 helper は `run_until_idle(&mut d, 60).await`、同 `:3027` は直後に `Status::Done` を assert する。`crates/task-dispatch/src/dispatcher/tests/build_cache.rs:396-401` の `every_cargo_path_uses_the_scratch_target_dir` は `run_until_state` で `Done` を待つ形に直っている（同 `:489-494` の task 単位経路も同様）。
