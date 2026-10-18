---
task: 01M56HCNTPN0H2VGP4JE0JYRM9
wu: build-diag-fix
status: done
completed: 2026-10-18
---
# build-diag-fix: benchfs lib test の compile error（host-build2）の原因と修正

> 注（履歴）: 本書の実走は旧 known-good（p0-final.patch 単独）で、原因は by-ID 経路の欠落（tree 側）だった。最新結論は `../2026-10-18-p1-3-known-good-fix.md`（k2-fix.patch, k2-known-good-v2）。

## 原因
- **K2 の評価 command（`cargo test --release --lib -- <隠しテスト>`）が `--features transport-locusta` を有効にしていなかった。**
  初期 commit 086c4618 の benchfs は、optional な依存 `rrrpc`（`lib/locusta/rrrpc`、`transport-locusta = ["dep:rrrpc", "dep:mlx5"]`）を
  cfg なしで使う module を持つ（`src/ffi/io_buffer.rs:13` の `use rrrpc::relay::client::DmaBuffer`、`src/rpc/registry_handshake.rs`・
  `udp_handshake.rs`・`ucx_relay.rs`・`locusta_call.rs` 等）。さらに `io_buffer.rs:45` が呼ぶ `ConnectionPool::locusta_transport_ref`
  は `src/rpc/connection.rs:415` で `#[cfg(feature = "transport-locusta")]`。feature 無しでは lib 自体が compile できない
  （E0433 = 未解決の crate `rrrpc`、E0599 = `locusta_transport_ref` が無い。host-build2 の 8 errors）。
- 根拠: known-good patch（`p0-final.patch`、P0 commit 54f2ad2b）の commit message の検証 command が
  `cargo test --release --features transport-locusta --lib`。benchfs の `install.sh` も `BENCHFS_FEATURES=transport-locusta` を前提にする。
- lock の locusta 68f8b210 / pluvio 9ecd51ee の組合せ、known-good patch の当て方（`git apply --check` は 086c4618 に clean に当たる）、
  隠しテストの差し込み（`.grader/run.sh` は `src/rpc/metadata_ops.rs` に子 module を足すだけで、compile error の場所ではない）、
  asset 組立（lock と一致、snapshot 完全性 ok）は原因ではない。pluvio 9ecd51ee には `pluvio_uring::file::DmaFile` が在る。

## どの tree で落ちたか
- **known-bad と known-good の両方。** 落ちたのは初期 tree 由来の lib の compile（`benchfs (lib test)`）で、patch が触る
  `src/rpc/metadata_ops.rs` とは無関係なので、両 tree で同じ error が出る。
- host-build2 の assert（`left: Fail, right: Pass`）は known-good の assert（`real_asset_grade.rs:574`）。その前の known-bad の
  `Fail` の assert は通っていたが、それは同じ compile error で落ちただけで、**正しい理由で落ちたのではない**（偽の負例合格）。
  旧 grader はこの `error[E` をモデル失敗に数えていた。

## 修正
- 評価 command: `private.json` の `hidden_command` を `cargo test --release --features transport-locusta --lib -- lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks`
  にした。試行に見せる `case.json` の `visible_command` も `cargo test --release --features transport-locusta --lib`。成功条件（must_pass の試験名・隠しテスト本文）は変えていない。
- 初期 tree 由来の compile error を env_failure にする（`crates/campaign/src/grader.rs`）:
  - grade-case の `implementation.baseline_command`（任意。K2 は `cargo test --release --features transport-locusta --lib --no-run`。隠しテストは挿入しない）と、
    `GradeOptions.initial_tree`（grader 側で lock どおりに組んだ初期 tree）を足した。
  - 評価が rustc の compile error（`error[E` / `error: could not compile`）で落ち、case が baseline を持つとき、初期 tree で baseline を走らせる。
    落ちれば `initial tree does not build with the case's build settings … case/asset defect, not a model failure` で env_failure。
    通れば従来どおりモデル失敗。初期 tree が渡されていなければ出どころを決められないので env_failure（初期 tree を渡して採点し直す）。
  - CLI: `celeris-campaign grade … --initial-tree <dir>`、`grade-out … --initial-trees <dir>`（`<dir>/<problem_id>`）。
  - `CasePrivate.baseline_command` → `grade_case_gen` が grade-case に写す。baseline の無い case（K1・K4 ほか）の grade-case は不変（hash も不変）。
- 設計判断: `agent-docs/adr/2026-10-18-grade-initial-tree-baseline.md`。
- 能力: `case.json` の `required_capabilities` に `rdma-verbs`（`/usr/include/infiniband/verbs.h`・`mlx5dv.h` と `libibverbs.so`・`libmlx5.so`）を足した。
  `transport-locusta` で入る `lib/locusta/mlx5` の build.rs がこれらの header を assert し、`mlx5_sys` が `-lmlx5 -libverbs` を link するため。
- lock: grader 変更なので作り直した。K2 grade-case sha256 は `d944a6c1…` → `f84089bc…`（`grade_case_ref`）、private は `be22701d…` → `2264fec5…`（`private_ref`）。
  旧 lock とは `verify_grade_case` が不一致で拒否する（既存試験 `changed_grade_case_or_hidden_test_no_longer_matches_the_old_lock`）。旧試行とは混ざらない。
  `initial.lock.json`（初期 tree・submodule の固定）は変えていない（asset は正しかった）。

## 試験（`crates/campaign/tests/real_asset_grade.rs`）
- `initial_tree_compile_error_is_env_failure_not_model_failure`: host-build2 と同形の rustc 出力を、評価と baseline の両方が返す実行器で採点 → `failure_class = env_failure`、
  理由に `initial tree does not build`、`details.baseline.initial_tree_builds = false`、2 回目の呼び出しが baseline（隠しテスト挿入なし）であること。初期 tree 無しでは `cannot be attributed` で env_failure。
- `model_defects_stay_model_failure_even_when_noise_mentions_the_environment`（既存）: 初期 tree の baseline が通る状況で、成果物の rustc error（`lookup_metadata` 未定義など）はモデル失敗のまま。
- `k2_build_command_enables_the_transport_feature_the_initial_tree_needs`: 評価 command と baseline に `--features transport-locusta`、`visible_command` も同じ、`rdma-verbs` 能力がある。
- `real_k2_assets_build_and_grade_known_bad_and_known_good`（実物、`CAMPAIGN_ASSET_DIR` 時のみ）: 組んだ初期 tree の `Cargo.toml` の feature 定義と `io_buffer.rs` の `use rrrpc::` を確かめ、
  評価 command が feature を有効にしていることを見る。`--build` 時は先に初期 tree で baseline を走らせ、落ちれば `env_failure: 初期 tree が build できない` で止める。
  正負の採点には初期 tree（= known-bad）を `initial_tree` として渡す。

## 証拠
- 静的特定（worker sandbox、本番 host・network・LLM なし）: asset store `/local/celeris/data/campaign-assets/k2` の bundle を $TMPDIR に clone して 086c4618 を確認。
  `grep` で上記の cfg なし `rrrpc` 利用と cfg 付き `locusta_transport_ref` を確認。`git apply --check p0-final.patch` → clean。
- `cargo test -p celeris-campaign --test real_asset_grade` → 12 passed, 0 failed。
- `CAMPAIGN_ASSET_DIR=/local/celeris/data/campaign-assets/k2 CAMPAIGN_K2_FIX_PATCH=<01M3QKJZ…/artifacts/p0-final.patch> cargo test -p celeris-campaign --test real_asset_grade -- real_k2`
  → 2 passed（実 asset の組立・lock 一致・patch 適用・feature 定義の照合。build はしない）。
- `cargo test -p celeris-campaign` → 全 test binary で failed 0。
- `bash scripts/dev/test-parallel.sh` → exit 0（passed 5756, failed 0, ignored 15）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-adr-numbers.sh`・`check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-doc-links.sh` → exit 0。

## 未検証
- 実 build（`--build`）は未実行。worker sandbox には `/usr/include/infiniband` が無く、mlx5 の build.rs が assert で止まる（ここで走らせても env_failure にしかならない）。
  feature を有効にした build で known-bad が**隠しテストの compile error（`lookup_metadata` 未定義、E0425）でモデル失敗**、known-good が pass になるかは operator の再実走で確かめる。
- host に rdma-core の header/library があるかは未確認（host-cap2 は `rdma-verbs` を見ていない）。

## operator の再実走手順（host、無 LLM・job なし）
1. 必要なら rdma-core の開発 package を入れる（Debian/Ubuntu: `sudo apt install libibverbs-dev`。`verbs.h`・`mlx5dv.h`・`libibverbs.so`・`libmlx5.so` が入る）。
2. cap-probe を K2 の A/B/C で再実走し、`rdma-verbs` を含め exit 0 を確かめる。
3. `CAMPAIGN_K2_FIX_PATCH=<p0-final.patch> CAMPAIGN_K4_TGZ=<main-v3-n2-43706.tgz> sh crates/campaign/scripts/verify-real-assets.sh /local/celeris/data/campaign-assets/k2 --build`
   - 期待: snapshot ok → 初期 tree の baseline build ok → known-bad は `Fail`（model_failure）、known-good は `Pass` → K4 ok、exit 0。
   - baseline で落ちたら panic の文言が `env_failure: 初期 tree が build できない`。stderr 末尾で header/library/依存取得のどれかを見る。
4. 本測定の grade-out では `--initial-trees <dir>`（`<dir>/k2-bf-xattr-restart-campaign` に組んだ初期 tree）を渡す。渡さないと compile error の試行は env_failure（未判定）になる。

## 提案
- K1（dev、`k1-bf-fsync-campaign`）も `cargo test --release --lib` で、初期 commit が同じ系統なら同じ欠陥の可能性がある。採用 case ではないので本 WU では触っていない。
- 隠しテストは `lookup_metadata(&ctx, path, bool)` という known-good の内部関数名・引数に依存する。別の正しい実装（関数名が違う）は compile error でモデル失敗になる。
  成功条件の緩和はしないが、task 文面（materials）に要求 interface を書くかは測定前に人が決める点（`private.json` の `portability_unverified` と同種）。
