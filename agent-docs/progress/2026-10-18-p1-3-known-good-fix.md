---
tasks: [01M56HCNTPN0H2VGP4JE0JYRM9]
wu: known-good-fix
status: done
completed: 2026-10-18
title: P1-3 known-good-fix: K2 known-good を Sirius で ok だった tree にする
updated: 2026-10-18
---
# known-good-fix: K2 known-good を Sirius で ok だった tree と同一内容にする

## 1. 原因（特定済み）
host 実走で known-good（086c4618 + p0-final.patch）が `lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks` の `recovered.is_file()` で落ちた。
- Sirius の P0 workspace（01M3QKJZ8Y8TMSQ6HXQNFDFD9Z/artifacts）の `p0-final.patch`・`p0-run6.patch` を読んだ。`src/rpc/metadata_ops.rs` の前像 `f833fe3` → 後像 `18cf7ee` は両者で同一、086c4618 の同 file の blob も `f833fe3`。**metadata_ops.rs は Sirius の e4c8e77d と同一内容**（e4c8e77d の追加分は aiori-BENCHFS.c と sirius-p0-build.sh だけで、この試験に無関係）。
- 差は別 file。Sirius の系列 086c4618 → … → 60d19a16 には `p0-xattr-path.patch`（535f72d2、`src/rpc/handlers.rs` の `chunk0_xattr_size`/`persist_chunk0_xattr_size` が by-ID synthetic path `/benchfs/<hash>` も見る）が入っている。k2 初期 tree（086c4618）と known-good にはこれが無い。
- 再現（本 run、無 LLM）: 086c4618 + p0-final.patch の `cargo test --release --features transport-locusta --lib` で、by_id=false の反復は `Ok(Some(4))`、by_id=true の反復は `chunk0_xattr_size` が `Ok(None)` → `is_file()` が偽。xattr は host で正常（/tmp・/local とも set/get 成功）。**環境差ではなく tree の欠落**。
- 未検証: 60d19a16 そのものの tree（bundle は親 commit 欠落で clone 不能、`git bundle verify` 不可）。086c4618 → 60d19a16 の間に handlers.rs 以外の差があるかは確認していない。手順: Sirius 側で `git diff 086c4618 60d19a16 --stat`（read-only）。本 known-good は「Sirius ok の lookup 試験が通る最小の同一内容」で、全 file の同一性は主張しない。

## 2. 直し（測定 ID `k2-known-good-v2`）
- known-good = 086c4618 + `p0-xattr-path.patch`（handlers.rs のみ）+ `p0-final.patch`（metadata_ops.rs のみ）の合成 **k2-fix.patch**（2 file、7079 byte）。
  - sha256 `6fc5d09a880ce405f4f81cbbf559df2b2bcf191946e14040c05705de63d22773`（p0-xattr-path.patch 9f2c996b…、p0-final.patch 7226b7fb…）。
  - 実物は本 run の artifacts/k2-fix.patch（`git apply` で 086c4618 に clean に当たる）。
- `private.json` に `expected.known_good`（測定 ID・旧 ID `k2-known-good-v1`・base・patch の sha256・Sirius 同一性の説明）を追加。`case.json` の `private_ref.sha256` を `2264fec5…` → `d0212607…` に更新。**旧 lock と不一致になり、旧試行の結果に混ざらない**（grade-case 本体・hidden test・initial.lock.json は不変）。
- `real_asset_grade.rs`: known-good patch の sha256 を `k2-known-good-v2` に固定、handlers.rs の by-ID 経路の印を bad=無/good=有で確認。テストは緩めていない。
- known-good が Pass にならない場合は env_failure ではなく `failure_class` と tree（bad/good）を出す既存経路のまま（診断に tree 名を含む）。

## 3. 無 LLM 検証（本 run、実 cargo、host build）
- `CAMPAIGN_ASSET_DIR=/local/celeris/data/campaign-assets/k2 CAMPAIGN_REAL_BUILD=1 CAMPAIGN_K2_FIX_PATCH=<k2-fix.patch> cargo test -p celeris-campaign --test real_asset_grade -- real_k2_assets_build_and_grade_known_bad_and_known_good`
  → **ok**（84.7 秒）。known-bad（初期 tree）= implementation Fail、known-good = Pass、どちらも env_failure ではない。
- 修正前（同環境、patch=p0-final.patch のみ）は同試験が FAILED（295 秒）で operator の結果を再現した。
- 実 LLM・有料 job は呼んでいない。

## 4. operator 手順（asset store は書いていない）
```sh
A=/local/celeris/data/campaign-assets/k2
# 1) artifacts の k2-fix.patch（本 task の成果物）を置く
cp <task artifacts>/k2-fix.patch $A/k2-fix.patch
(cd $A && sha256sum k2-fix.patch)   # 6fc5d09a880ce405f4f81cbbf559df2b2bcf191946e14040c05705de63d22773 と一致を確認
cp $A/SHA256SUMS $A/SHA256SUMS.bak-20261018-k2fix
(cd $A && sha256sum k2-fix.patch >> SHA256SUMS)
# 2) 再実走（branch を統合した commit で）
export CAMPAIGN_K2_FIX_PATCH=$A/k2-fix.patch
sh crates/campaign/scripts/verify-real-assets.sh $A --build
```
期待: exit 0（snapshot 完全性・K2 known-bad Fail / known-good Pass・K4 再計算）。落ちたら出力の `failure_class` と tree（bad/good）を記録する。
k2-fix.patch が無い場合の取得元は P0 workspace の `p0-xattr-path.patch` と `p0-final.patch`（`git apply` を順に）。

## 5. 未解決・提案
- cap-runbook・README・real-evidence は旧 `p0-final.patch` 表記のまま（並列 WU の担当 file）。統合後に `k2-fix.patch`（k2-known-good-v2）へ置換する（record-rerun で）。
- `k_cases_grade.rs` の `real_benchfs_trees_from_the_bundle…` は旧 p0-final.patch のみで印確認（metadata_ops の印のみで今も通る）。
- 60d19a16 全体との同一性は上記の read-only 手順で確認できる（未検証）。

### k2-fix.patch の再生成（artifacts が見えない場合）
```sh
P=/local/celeris/data/workspaces/01M3QKJZ8Y8TMSQ6HXQNFDFD9Z/artifacts
d=$(mktemp -d) && git clone -q /local/celeris/data/campaign-assets/k2/benchfs-086c4618.bundle $d/b 2>/dev/null
cd $d/b && git checkout -q 086c4618 && git apply $P/p0-xattr-path.patch --include=src/rpc/handlers.rs && git apply $P/p0-final.patch
git diff -- src/rpc/handlers.rs src/rpc/metadata_ops.rs > k2-fix.patch && sha256sum k2-fix.patch   # 6fc5d09a… と一致
```

## 6. host 再実走（operator、無 LLM・job 投入なし）
区別: **実経路確認済み** = host 実走で exit 0 を確認。**未成立** = 未実走または失敗。認証材料の値は書かない。

| # | commit | cap-probe（K2 a/b/c・K4 a） | `verify-real-assets.sh --build` | 区別 |
|---|---|---|---|---|
| 1 | ad80c920 | すべて exit 0（sirius-link・autotools・libclang を含む全能力 ok）。host-cap2-*.json | exit 101。snapshot 完全性 ok・K4 再計算 ok。K2 は初期 tree が compile error（`locusta_transport_ref` ほか 8 errors、host-build2.log） | cap-probe は実経路確認済み。--build は未成立（build-diag-fix で解消） |
| 2 | 82d15134 | すべて exit 0。K2 arm a は 11 能力（rdma-verbs 含む）すべて ok。host-cap3-*.json | exit 101。snapshot 完全性 ok・K4 再計算 ok。初期 tree は transport-locusta で build 成功（Finished release 28.81s）。`real_asset_grade.rs:770` で known-good が Pass にならず（left Fail / right Pass、env_failure=false）。`lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks` が known-good の 2 試験（hidden と tree 自身）で `recovered.is_file()` 失敗。host-build3.log | cap-probe は実経路確認済み。--build は未成立（原因は tree の欠落 = §1、known-good-fix で k2-fix.patch に置換） |

- 失敗した tree は known-good（086c4618 + p0-final.patch）。known-bad 側は期待どおり Fail。env_failure は 2 回とも false で、環境不良ではなく tree 側の原因だった。
- **known-good-fix（§2・§3）後の operator 再実走（k2-fix.patch 配置 + `--build`）の出力は、この WU の時点でまだ回答に届いていない。** よって K2 known-good Pass / known-bad model_failure を host で確認した、とは記録しない。K2 実 build 正負例の host 判定は **未成立（未実走）**。本 run の worktree 検証（§3、84.7 秒 ok）は host の `--build` 実走の代わりにならない。
- 次の手順: §4 の手順で operator が `--build` を再実走し、exit・known-bad/known-good の結果（落ちたら failure_class と tree）をこの節の表の行 3 に追記する。exit 0 なら実経路確認済みに格上げする。

## 7. 対象環境（host, 作業 3 の task 01M57B1FTG…）での再確認
- 配置済み `/local/celeris/data/campaign-assets/k2/k2-fix.patch` の sha256 = `6fc5d09a…2773`（期待と一致）。`SHA256SUMS` にも同 hash の行があり `sha256sum -c` 相当の snapshot 完全性は全 6 件 OK。
- 実行 commit: aaee85d8（main）上の本 branch。コマンド: `CAMPAIGN_K2_FIX_PATCH=$A/k2-fix.patch CAMPAIGN_K4_TGZ=…/cmp4/raw/main-v3-n2-43706.tgz sh crates/campaign/scripts/verify-real-assets.sh $A --build` → **exit 0**。
  - real_k2_snapshots_complete_before_build ok、real_k4_raw_recompute_matches_the_grader ok（K4 実 raw 再計算）、real_k2_assets_build_and_grade_known_bad_and_known_good ok（known-bad Fail / known-good Pass、207 秒）。
- 区別: 実経路確認済み（host 実走 exit 0）。grader は緩めていない（コード変更なし）。実 LLM 未使用。
- 正解 patch は asset store（grader 私有）にあり、試行への公開入力へは配送されない（変更なし）。
