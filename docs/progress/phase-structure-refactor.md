---
tasks: [01M3RCEF8QZV26GY3EYEDRRZ5T, 01M3SB3GJSHG7RBFYA1HARWTYB]
---

# 構造リファクタリング: inline test 外出しと module 境界

## 最終状態（2026-09-30）

- Rust inline test は開始時 884606d の 100,831 行（193 ファイル）から、最終 HEAD aaa6a1ca の 6,160 行（75 ファイル）になった。大きな test 群は隣接する `tests.rs` へ移し（外部 test 46,789 → 143,942 行）、private item へのアクセスを維持した。
- config、dispatcher、store、handlers、daemon 起動配線、execution plan、worker/ops、GUI task detail の責務境界を crate 内 module に整理した。公開 crate 境界と既存 API を保ち、不要な trait や crate 分割は導入していない。
- 2,000 行を超える production ファイルは 10 本 → 1 本（`task-api/src/types.rs`、guardrail の例外に理由付きで登録済み）。`source-size-report.py --strict` は 0 active warning / 1 excepted、exit 0。
- `crates/task-core/migrations`・`docs/api/v1`・`docs/protocol`・`config/` の main f34f060 からの差分はゼロ。

監査からの段階分け: 巨大 inline test の移動を先に行い、P0 で config / dispatcher / store、P1 で GUI task detail / API handlers / celeris daemon、P2 で task-core / worker / ops の cohesion を扱った。責務の移動と関数本体の再設計を分離し、小さくレビューできる統合単位にした。

## 主要ファイルの LOC 前後比較（884606d → HEAD aaa6a1ca）

計測: `python3 scripts/dev/source-size-report.py --format json`（HEAD の script を 884606d の `git worktree` にも `--root` で適用）。「production」は手書き production 行（inline test を除く）、「inline test」は production ファイル内の `#[cfg(test)] mod`、「外部 test」は分割先 module 配下の `tests.rs` / `tests/` の行数。HEAD の「module 合計」は元ファイルと同名ディレクトリ配下の production 合計（括弧内はファイル数）。

| 対象 | 884606d production | 884606d inline test | HEAD 元ファイル production | HEAD module 合計 production | HEAD 最大ファイル | HEAD inline test | HEAD 外部 test |
|---|--:|--:|--:|--:|---|--:|--:|
| `task-dispatch/src/dispatcher.rs` | 17,013 | 19,166 | 1,868（facade） | 18,205（21） | 1,868 `dispatcher.rs` | 233 | 26,824 |
| `task-core/src/store.rs` | 6,578 | 5,603 | —（`store/` へ） | 7,911（23） | 775 `task_store_impl.rs` | 5 | 5,592 |
| `celeris/src/config.rs` | 4,100 | 3,165 | —（`config/` へ） | 4,545（18） | 698 `harness.rs` | 0 | 3,223 |
| `task-api/src/handlers.rs` | 3,125 | 284 | 334 | 3,351（11） | 459 `accounts.rs` | 0 | 280 |
| `celeris/src/lib.rs` | 2,608 | 1,303 | 83 | 起動配線は `daemon/` 2,683（10） | 467 `daemon/tick_loop.rs` | 0 | 1,286 |
| `task-core/src/execution_plan.rs` | 3,229 | 1,933 | 624 | 3,267（3） | 1,739 `validation.rs` | 0 | 1,967 |
| `task-worker/src/claude_code.rs` | 2,557 | 3,273 | 1,071 | 2,593（2） | 1,522 `prompt.rs` | 5 | 3,335 |
| `task-worker/src/paperqa.rs` | 2,267 | 3,902 | 1,948 | 2,280（2） | 1,948 `paperqa.rs` | 0 | 3,887 |
| `task-worker/src/codex.rs` | 1,169 | 2,288 | 1,175 | 1,175（1） | 1,175 | 0 | 2,272 |
| `task-worker/src/local_deep_research.rs` | 931 | 2,745 | 933 | 933（1） | 933 | 0 | 2,741 |
| `task-ops/src/knowledge.rs` | 1,939 | 1,016 | 1,654 | 1,943（2） | 1,654 | 0 | 1,011 |
| `task-ops/src/view.rs` | 1,477 | 1,348 | 1,549 | 1,549（1） | 1,549 | 0 | 1,336 |
| `task-ops/src/replay.rs` | 1,192 | 1,534 | 1,213 | 1,213（1） | 1,213 | 0 | 1,537 |
| `gui/app/routes/tasks.$id.tsx` | 2,935 | — | 527 | 527 + `components/task-detail/` 2,144（8） | 790 `OverviewTab.tsx` | — | — |

全体（884606d → HEAD）: Rust production 142,338 → 148,060 行（269 → 361 ファイル。module 宣言・`use`・main 由来の機能追加を含む）、Rust inline test 100,831 → 6,160 行、Rust 外部 test 46,789 → 143,942 行、TS production 41,086 → 41,142 行。

## 2,000 行を超える production ファイルの分類（HEAD）

| 区分 | ファイル | production 行 | 理由 |
|---|---|--:|---|
| cohesive なので残した | `crates/task-api/src/types.rs` | 2,031 | API の wire 型の平らな定義で、`docs/api/v1/*.schema.json` の schemars の唯一の源。endpoint 別に割っても 1 endpoint を触るエージェントが読む量は減らず、正本が散る。`source-size-report.toml` に理由付きの例外として登録（audit.md §9） |
| 次の候補 | （2,000 行超は無し） | — | 閾値を超えるものは残っていない |

閾値の手前（1,700 行超）で次に割る候補として監視するもの:

| ファイル | production 行 | 見立て |
|---|--:|---|
| `crates/task-worker/src/paperqa.rs` | 1,948 | 描画は `paperqa/render.rs` へ出した。残りは設定型・seed URL と検索 query の組み立て・実行・結果の解釈。次に伸びたら設定型と seed/query 組み立てを子 module に出す候補 |
| `crates/task-dispatch/src/dispatcher.rs` | 1,868 | `Dispatcher` facade（子 module 20 本の宣言）に加え、`DispatchConfig` / `ExecutionConfig` など runtime 設定型と scratch target の割り当てが残る。新しい責務は `dispatcher/` の子 module に置き、設定型の `dispatcher/config.rs` への移動が次の候補 |
| `crates/task-core/src/execution_plan/validation.rs` | 1,739 | 計画検証の規則群。規則は相互参照が多く当面 cohesive。上限系（木の上限・バイト数）と構造系の分離が次の候補 |

## 検査記録（最終 HEAD aaa6a1ca、2026-09-30）

- `cargo test --workspace` → exit 0、2,935 passed / 0 failed / 8 ignored、所要 4 分 57 秒（real）
- `cargo fmt --all -- --check` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0（warning 0）
- test 属性数 `git grep -hE '#\[(tokio::)?test' <rev> -- crates | wc -l` → f34f060 = 2,907、HEAD = 2,907（減少なし）
- `git diff --quiet f34f060 HEAD -- crates/task-core/migrations docs/api/v1 docs/protocol config` → exit 0（差分ゼロ）
- `python3 scripts/dev/source-size-report.py --strict` → exit 0（0 active warning、1 excepted）
