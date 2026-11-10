---
title: 健全性v2 R5 map-api-c — API試験215件の契約対応
tasks: [01M7388T31FEX7W469M2TJS236]
status: done
updated: 2026-11-10
---
# 健全性v2 R5 map-api-c — API試験215件の契約対応

完了日: 2026-11-10

commit: `phase 1: map api-c test contracts and setup candidates`

## 範囲と結果

`mapping/api-c.json` の51 file、215試験関数（tests s〜z: 111件、src: 104件）にcapabilityと個別の契約を記入した。
scannerが不変条件と判定した76試験は候補のmembersに含めない。scanの基点・tests・組のmembersは雛形のまま保持した。
全体のcontracts.jsonの組立てとold_implの判断は後続のassemble・old-impl担当で行う。

## 作業前brief

1. 根本原因: 対象試験と製品契約の対応が雛形では未記入で、重複の機械的検出だけでは整理可否を判断できない。
2. 責務owner: 対応表のapi-c fragment。製品のownerはarchitecture-mapのHTTP API、Browser identity・信頼端末、TS制御層、CoS監査、再帰task木、知識・文書、Web端末の行を参照した。
3. 既存の再利用先: contracts.pyの走査・不変条件判定・fragment検査、B0 capability-map、commit済み雛形を使用する。
4. 選択肢: file単位で一律にする案と関数別に上書きする案を比較し、入力と結果が違う契約は関数別に記載した。
5. 新しい状態・分岐・fallback・port: 無し。分類用の追加capabilityだけを5件宣言した。
6. 撤去対象: 無し。候補の採用は判断材料の記録であり、試験や本体を消さない。
7. 不変条件: 全215件に有効なcapabilityと200字以内の契約があり、全3組を判断し、不変条件の試験を候補に入れず、crates/とscannerを変更しない。

## capabilityごとの関数数

| capability | 件数 |
|---|---:|
| accounts-quota | 6 |
| api-secret-administration | 19 |
| browser-live-view-control | 14 |
| browser-policy-broker-approval | 2 |
| browser-site-policy-trusted-devices | 10 |
| budget-pricing | 2 |
| cos-chat-threads-runs | 1 |
| cos-inbox-triage-operations-audit | 12 |
| cron-jobs | 1 |
| delivery-merge-candidate | 4 |
| execution-metrics | 3 |
| http-api-auth-principals | 30 |
| knowledge-pages-skills | 5 |
| pre-review-sync-auto-resolve-integration-repair | 2 |
| recursive-task-tree | 11 |
| release-selfdeploy-wiring | 5 |
| repository-docs-artifacts | 10 |
| review-runs-final-review | 1 |
| routing-audit-outcome | 1 |
| routing-eval | 5 |
| scratch-build-cache-disk-guard | 1 |
| script-runtime | 28 |
| task-lifecycle | 35 |
| terminal-sessions | 4 |
| worker-protocol-adapters | 1 |
| workunit-scheduling-stage-integration | 2 |
| 合計 | 215 |

## 組の採否

- duplicate_setup: 3組中1組を採用、2組を不採用。
  `job_wait_extend.rs` と `review_rework.rs` の `seq` は同じEvent列への連番付与で、共通の試験helperへ束ねる候補にした。
  keep_testsには全5試験を残し、action=merge、mutation_targetは `crates/task-api/src/cos/basis/job_wait_extend.rs` とした。
  実施時には待ちの充足・質問の前後の判定を反転して残存試験が検出することを確かめ、差し戻し側の最新runの選択も別途確認する。
  mode_pathとsha_of/shaは1行の文字列生成だけで整理効果が小さく、不採用とした。
- equivalent_scenario: scannerがこのfragmentへ割り当てた組は0件。採用0、不採用0。
  cos/basisの単体試験はevent列から根拠を選ぶ契約、cos_plan_conforms等の結合試験は認証・操作監査・書込みを通す契約であり、名前の類似だけで同値と判断しない。
  他fragmentがownerである組の採否は変更していない。
- old_implはこのleafの担当外。木metadataの無いtaskの1節点表示や旧fieldの拒否は現行の境界契約として記述し、削除候補を追加していない。

## extra_capabilities

B0の既存能力で表せない5領域を、実在するowner pathと理由付きで補った。

- knowledge-pages-skills: 知識pageのpath境界、skill管理、組織への継承mount。
- repository-docs-artifacts: repository文書・成果物配信のpath、MIME、Range、symlinkの境界。
- api-secret-administration: 管理用秘密fileのCRUD、0600、指紋と値の非公開。brokerによるcredential注入とは別の契約。
- execution-metrics: 索引とeventの集計一致、groupとcontinuationの集計。
- terminal-sessions: human:web限定の端末、cluster接続とwork dir、session開閉監査。

## 証拠

提出時のHEADで実行。基準は `$CELERIS_WU_BASE`。mainの同期は行っていない。

- 条件0（215件の契約）: `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --check-fragment agent-docs/progress/2026-11-10-health-v2-r5/mapping/api-c.json` → exit 0、ok。
- 条件1（全owner組の採否）: 同じfragment検査 → exit 0。3組すべてに採否と理由があり、採用1組にkeep_tests・action・mutation_targetを記入。
- 条件2（不変条件の除外）: 同じfragment検査とscanner結果の照合 → exit 0。不変条件76件と候補membersの共通要素は0件。
- 条件3（試験とcrates/を変更しない）: `git diff --exit-code "$CELERIS_WU_BASE" -- crates/` → exit 0、差分無し。
- `git diff --name-only "$CELERIS_WU_BASE" | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0、write-set R5: ok。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、変更はapi-c fragmentとこの進捗の2 fileだけ。
- `bash scripts/dev/test-affected.sh --list` → exit 0、対象crate無し。
- `bash scripts/dev/test-affected.sh` → exit 0、docsのみで試験対象無し。repo-healthはpass、new=0、worsened=0。
- `cargo clippy --workspace -- -D warnings` → exit 0。vendorのdeno_coreに既存のset_linger非推奨warningが1件。
- `cargo fmt --all -- --check` → exit 0、整形差分無し。
- `sh scripts/dev/progress-index.sh --check`、`sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh` → いずれもexit 0（ADR 414 files）。

全体の試験・browser/userns・web e2eはrelease gate専用のため実行していない。

## 未解決

候補の共通化とmutation実行はこの調査の範囲外。ここでは試験を変更していない。

## 提案

採用したseq共通化は後続の整理で扱い、全5試験の入力列とassertionを保持する。
追加capabilityはassemble時に他fragmentの追加分類との重なりを照合する。
