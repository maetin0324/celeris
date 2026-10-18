---
task: 01M56HCNTPN0H2VGP4JE0JYRM9
wu: nested-asset
status: done
completed: 2026-10-18
---
# nested-asset: 入れ子 submodule snapshot を lock に固定する K2 asset 組立

## 変更
- `crates/campaign/src/lock_tree.rs`: `derive_initial` が入れ子 submodule を再帰で導き、lock に root 相対の全 path で平らに載せる
  （例 `lib/pluvio/pluvio_ucx/lib/async-ucx`）。親の内容 hash は入れ子を含まない。`verify_repo` は本体の gitlink（最上位）だけ、
  `verify_archive` は親の hash から入れ子 path を除いて各 submodule を別に照合する。`top_level_submodules` / `nested_paths` を追加。
- `crates/campaign/src/lock_exec.rs`: workspace-seed 検査も同じ入れ子扱い。
- `crates/campaign/src/asset_assembly.rs`（新）: `check_snapshots` / `assemble_initial`。snapshot は
  `submodules/<末尾名>-<commit 先頭 8 桁>.tar`（既存の `pluvio-9ecd51ee.tar` と同じ規則）。lock の全 submodule 分が在り、
  `SHA256SUMS` に載り、hash が合うことを clone/展開/build の前に確かめる。不備は `EnvFailure`（`env_failure:` 接頭、モデル失敗ではない）。
  SHA256SUMS ごと差し替えても、展開結果が lock の内容 hash と違えば拒否。
- `tests/real_asset_grade.rs` は `assemble_initial` を使い、`real_k2_snapshots_complete_before_build` を追加。
  `scripts/verify-real-assets.sh` は `--build` の前にそれを走らせ、不備なら exit 5（env_failure）。
- lock の版/測定 ID: 入れ子 snapshot の追加は asset の変更なので、既存規則（`check_successor`）どおり新しい測定 ID が要る。
  `lock.rs` に試験を追加（同 ID は後継不可、新 ID は後継可、旧 lock の検証は旧内容のまま）。

## 証拠
- `cargo test -p celeris-campaign --test nested_asset --test real_asset_grade` → 5 + 10 passed。
- nested_asset: 正例（3 段 fixture が lock どおりに組める）/ 入れ子 snapshot 欠落（clone 前に停止）/ SHA256SUMS 未登録 / tar 改変 /
  SHA256SUMS 書換え + 内容差替え（lock で拒否）/ snapshot 名の曖昧さ。
- 実 asset store は読み取りのみ（`sha256sum -c` と script の起動確認）。書き換えていない。

## 未解決（人の作業）
- 実 K2 の `cases/final/k2-bf-xattr-restart-campaign/initial.lock.json` と case.json の submodules には async-ucx がまだ無い。
  pluvio tar は `.git` を持たず async-ucx の commit が分からない。人が (1) pluvio 9ecd51ee の gitlink（`pluvio_ucx/lib/async-ucx`）の
  commit を確認、(2) その snapshot を `submodules/async-ucx-<commit8>.tar` として asset store に置き SHA256SUMS に追記、
  (3) `derive_initial` で lock を再生成し case.json の submodules を更新、(4) 新しい測定 ID で lock を封印する。
  本 WU は実 store を変更しない。
