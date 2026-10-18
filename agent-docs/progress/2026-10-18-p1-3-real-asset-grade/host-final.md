---
task: 01M56HCNTPN0H2VGP4JE0JYRM9
wu: host-final-record
status: done
completed: 2026-10-18
---
# host-final-record: 修正後 commit の operator host 実走結果（cap-probe・--build）

> 注（履歴）: 本書の実走は旧 known-good（p0-final.patch 単独）で、原因は by-ID 経路の欠落（tree 側）だった。最新結論は `../2026-10-18-p1-3-known-good-fix.md`（k2-fix.patch, k2-known-good-v2）。

決定 host-rerun の回答（operator、commit 82d15134 = 0111c06c 以降、無 LLM・job 投入なし）をそのまま記録する。
コード・grader は変えていない。キー・認証材料は記録していない。**K2 の正負例は成立していない**（下記）。

## 1. cap-probe（K2 arm a/b/c、K4 a）
- host に libibverbs-dev を apt 導入（verbs.h・mlx5dv.h・libibverbs.so・libmlx5.so あり）。
- 4 件すべて exit 0。出力は operator の artifacts/host-cap3-{a,b,c,k4}.json。
- K2 arm a: ok=true。cc・c++・cmake・git・ucx-pkg・min-build-ucx・min-build-cmake・autotools・libclang・**rdma-verbs**・sirius-link の 11 能力すべて ok。
- **rdma-verbs: ok**（不足なし）。前回審査が指摘した「82d15134 で追加された rdma-verbs の実走記録が無い」は、この実走で解消。
- 注意: この実走時点の B/C は A と同じローカル sandbox を probe が走らせた記録で、worker 環境の確認ではなかった。現在の B/C probe は測定専用 daemon の process から環境を取る別経路（ADR 2026-10-18-bc-probe-daemon-env）。この記録を B/C の実経路証拠に使わない。

## 2. verify-real-assets.sh --build
- exit **101**（FAILED）。全文は operator の artifacts/host-build3.log。
- snapshot 完全性: ok。K4 実 raw 再計算: ok。
- 初期 tree（086c4618）の build は `--features transport-locusta` で通った（compile error は解消、Finished release in 28.81s）。build-diag-fix の原因特定（feature 欠落）が実 host で裏づけられた。
- `real_k2_assets_build_and_grade_known_bad_and_known_good` が `real_asset_grade.rs:770` で FAILED。
  - **known-good（落ちた tree）**: Pass にならない。env_failure=false。
    - 隠しテスト `k2_hidden_tests::lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks`（`.grader/k2_metadata_ops_hidden.rs:28`、`assertion failed: recovered.is_file()`）が FAILED。
    - tree 自身の同名試験 `tests::…`（`src/rpc/metadata_ops.rs:1180`、同じ assertion）も FAILED。
  - **known-bad**: failure_class（model_failure 期待）の値は、回答に記載がない。known-good で試験が中断したため、期待どおりかは確認できていない。
- 手がかり（operator 回答より）: (a) host の /tmp（tmpfs）・/local（btrfs）とも user xattr の set/get は成功。(b) 同じ試験は P0 の Sirius 実走で ok（p0-evidence/fixed-tests.log:309、run6-build-test.log:378）。(c) p0-final.patch（54f2ad2b、`src/rpc/metadata_ops.rs` のみ 77 行）は apply-p0-commits.sh で base 60d19a1653f0 → head e4c8e77d の系列の一部で、p0-run6.bundle の head e4c8e77d が known-good の正体の可能性がある。K2 初期 tree は 086c4618。

## 3. 区分
| 項目 | 区分 |
|---|---|
| 必要能力 11 種（rdma-verbs 含む）の cap-probe、A sandbox 同一経路（K2 a/b/c、K4 a） | **実経路確認済み**（A のみ。B/C の worker 環境 probe は ADR 2026-10-18-bc-probe-daemon-env の経路で、測定専用 daemon が起動した host での再実走が要る）。Rust の最小版比較（rustc-min）と transport-locusta 最小ビルド（min-build-locusta）は case に追加済みで、host 実走は未実施。ビルド場所はローカル host、sirius は接続確認のみ |
| 初期 tree の build（transport-locusta）、asset snapshot 完全性、K4 実 raw 再計算 | **実経路確認済み**（host 実走 ok） |
| K2 known-good が Pass、known-bad が model_failure | **未成立 / 未検証**。known-good が FAILED（env_failure=false）。known-bad の failure_class は回答に無く未確認 |
| known-good tree の同一性（086c4618 + p0-final.patch か、Sirius で ok だった e4c8e77d か） | **未検証**（要確認） |
| xattr の環境差か tree の差かの切り分け | **未検証** |
| 有料 job・本測定 | 起動していない |
| 実装（preflight・grader・asset 組立・lock） | 実装済み／無 LLM 検証済み（test-parallel・clippy・fmt は通常経路で exit 0） |

## 4. 未解決事項
- K2 実資産の正負例は**成立と言えない**。次の作業が要る（別 replan）:
  1. known-good を、Sirius で ok だった tree（p0-run6.bundle の e4c8e77d、または 086c4618 + p0-final.patch）と内容比較し、base の違い・patch が系列の最後の 1 commit だけかを確かめる。
  2. 同一内容でも host で落ちるなら xattr/ファイルシステム差と見て、env_failure と model_failure の分離（`recovered.is_file()` の失敗を環境不良に分類できるか）を決める。
- 前回審査が指摘した B/C の実 task-api worker 環境での確認は、本実走では埋まっていない。
