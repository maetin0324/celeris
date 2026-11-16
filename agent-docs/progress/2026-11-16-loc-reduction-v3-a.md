---
title: LoC 削減 v3 A — 削減対象の証拠付き監査と計画
tasks: [01M7J3NKV25NNRQ2R6VPWM16H9]
status: done
updated: 2026-11-16
completed: 2026-11-16
---
# LoC 削減 v3 A — 削減対象の証拠付き監査と計画

完了日: 2026-11-16

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 A。本体は変えていない（この file だけを commit した）。
候補表と各候補の詳細は run の成果物 `artifacts/candidates.md`（run 01M7J3NNX32FPYH73HYR6T6HTB）にある。

## 結論
- 確実に消せる根拠のある候補は 21 件（rank 付き）。人の承認が要る機能・設定・API・試験の廃止を含むものが 6 件。
- 下限の合計は production 全言語 4,091 行（Rust 3,541、own_total_with_tests 4,556）、上限の合計は production 7,710 行。
- **最終目標の production 純減 10,000 行は未達見込み。** 第 2 マイルストーン（累積 5,000 行）も下限の和では届かない。第 1 マイルストーン（1,500 行）は TaskStore の転送の層の撤去（C1、下限 1,500）だけで届く見込みだが、store の高 risk path で ADR の改訂が要る。
- 削減の大部分は抽象の層の撤去（C1 TaskStore の trait と 144 個の 1 行転送、C2 Dispatcher の 109 個の 1 行委譲、C3 単一実装の supertrait 16 個。合わせて下限 2,100）。残りは 20〜200 行の重複の解消と、承認が要る旧機能の廃止（Console 互換・deprecated API など）。

## 開始時 main の固定計測
`main` の `415a19a7004cb6a73b872e861fa8d1cf634dddbb` を `scripts/dev/source-size-report.py --exclusive-classes` で数えた（exit 0）。

| class / view | 行数 |
|---|---:|
| production 全言語 | 386,338 |
| production Rust | 340,223 |
| own_total_with_tests | 1,043,479 |
| external_test | 461,753 |
| tooling | 25,198 |
| vendor | 111,936 |
| generated | 44,892 |
| evaluation_infra | 145,216 |

V0 の比較器（`agent-docs/progress/2026-11-15-health-v2-v0/loc-compare.py --build`）でも同じ値（B0 の基準から product_only +20,161、own_total_with_tests +76,165）。

## 調べた範囲
- crate: task-core・task-dispatch・task-worker・task-api・task-ops（と呼び元の celeris・celerisctl・celeris-mcp・web・control-ts）。
- 重点: TaskStore の supertrait（C1・C3）、dispatcher と R4 の owner（C2、owner 自身の accessor は対象外とした）、chat/store（重複なし。legacy thread は本番に 222 件の data があり残す）、API の型（C10）、browser launcher（daemon runtime は launcher と supervisor を共有し、候補にしない）、重複する branch と fallback（C4〜C8・C12・C13・C21）。
- 候補にしなかったものと理由は `artifacts/candidates.md` の「調べたが候補にしなかったもの」の表にある（migrated_harness は本番で未解決の task が 72 件、script-runtime の DTO は依存の向きのための意図した写し、など）。
- 本番 DB は読み取り専用（`mode=ro`）でだけ開いた。

## 変更（commit の件名）
- `loc-reduction-v3 A: 削減対象の監査の進捗ファイルを足す` — この file だけ。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `python3 scripts/dev/source-size-report.py --exclusive-classes --rev <開始時 main> --format json` → exit 0（上の表）
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/loc-compare.py --build <artifacts>/loc-compare-415a19a7.json --head <開始時 main>` → exit 0
- `python3 scripts/dev/repo-health/repo_health.py --repo . --base HEAD~1 --head HEAD --profile celeris --policy-version 1` → exit 0（verdict=pass、既存の finding 714 件）
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates-scan.py` → exit 0（未参照の pub item 4 件）
- 受け入れ条件 3 の検査（merge-base からの差分が `agent-docs/` の下だけ）と、test-affected・clippy・fmt・progress-index の結果は run の報告に書く。

## 未解決
- 承認が要る 6 件（Console 互換の廃止、deprecated API の削除、旧 harness 設定の互換の削除、`[scratch]` 必須化、ablation の試験の削除、旧 worktree の配置の削除）は人の判断待ち。C 以降の子が実行の前に task の質問で聞く。
- 旧 worktree の配置が本番の workspaces に残っているかは未確認。
- `set_knowledge_probe` の扱いは R6 から続く人の判断待ち。

## 提案
- C1（TaskStore の転送の層の撤去）を D の最初に単独で行う。全 crate の `dyn TaskStore` を書き換えるので、E・F と並べると write set が重なる。先に ADR-0033 D3 と ADR 2026-11-10-taskstore-consumer-ports の改訂が要る。
- C の pilot は C9（task-ops docs の front matter の写し）、C6（CoS triage の一度きりの切替）、C2 のうち `dispatcher/cluster.rs` の 15 個の委譲。write set は file 単位で重ならない。
- 10,000 行の目標は、今回の監査の範囲では根拠が無い。目標を保つなら、承認が要る機能の廃止の範囲を人が広げるか、試験（external_test 461,753 行）の側の削減を別の目標として立てる必要がある。
