---
task: 01M56HCNTPN0H2VGP4JE0JYRM9
wu: record-real
title: P1-3 実資産・host 実走の証拠（K2 実 build 正負例・cap-probe）
status: 一部成立（K4 再計算・組立・cap-probe 7/8 は実経路確認済み。K2 実 build の正負例は host の build 道具不足で未成立）
updated: 2026-10-18
---
# real-evidence

人が決定 h-real で提供した asset と host 実走の結果（commit 7fd505cb、無 LLM、job 投入なし）を記録する。認証値・socket 実体は書かない。
有料ジョブ・本測定・LLM 呼出しは起動していない。旧 pilot の結果は使っていない。

## 1. verify-real-assets.sh（build なし）
- 実行: `CAMPAIGN_K2_FIX_PATCH=<p0-final.patch> CAMPAIGN_K4_TGZ=<main-v3-n2-43706.tgz> sh crates/campaign/scripts/verify-real-assets.sh /local/celeris/data/campaign-assets/k2`
- 結果: exit 0。SHA256SUMS 5 件 OK（bundle・ior・locusta・pluvio・async-ucx）、`real_k2_snapshots_complete_before_build` ok、
  `real_k2_assets_build_and_grade_known_bad_and_known_good`（build なし: 正負 tree の印と patch 適用）ok、`real_k4_raw_recompute_matches_the_grader` ok。
- 本 WU の修正後（lock 5 件）に同じ command を再実行して exit 0（上と同じ 3 test ok）。

## 2. K2 実 build の正負例（`--build`）— 未成立（env_failure）
host の初回（人の実走）: exit 101、`lib/pluvio/pluvio_ucx/lib/async-ucx/Cargo.toml` が無い。原因は lock が入れ子 submodule を載せていなかったこと。
本 WU の修正と、その後に分かったこと:
- lock（`initial.lock.json`）と case.json の submodules に入れ子を足した（commit は async-ucx 自身の gitlink から確定）:
  `lib/pluvio/pluvio_ucx/lib/async-ucx@cffa01eec85fa34e970721e735794a21c77c0be3`、
  `lib/pluvio/pluvio_ucx/lib/async-ucx/ucx1-sys/ucx@938ffcd10122742d0f46a4f609e7395d1648c969`。後者は async-ucx の `ucx1-sys/build.rs` が UCX を source から build するために要る
  （`ucx` が空だと `autogen.sh: No such file`）。組立は両方を展開し lock の内容 hash と照合する。
- 依存は network が使える環境では crates.io から解決でき（`Updating crates.io index` は build の正常動作。offline では `no matching package named async-backtrace`）、
  offline 化（vendor）は asset store に vendor 一式を置いて grader が source replacement を使う設計の追加になる。新機構を足さず、network が使える host で build する前提にした。
  offline のまま解決できない失敗は env_failure に分類する（下記）。
- 組立後の build（この worker host、network あり）: 入れ子の欠落は解消し、`ucx1-sys` の build script まで進んだ。止まった理由は host の道具不足:
  `autoreconf: command not found`（autoconf/automake/libtool）と `Unable to find libclang`（bindgen）。
  known-bad・known-good ともに `env_failure: true`（モデル失敗に数えない）。**known-bad が fail・known-good が pass の実 build は未実証**。
- 分類の修正（`crates/campaign/src/grader.rs`）: 依存 crate の build script の panic（`failed to run custom build command` を伴う `panicked at`）が
  モデル失敗に誤分類され、`--build` が「known-good が Fail」と出していた。env_failure として扱う。`offline mode (via --offline)`、`Unable to find libclang` も環境不良の印に追加。
  試験は `real_asset_grade` の `build_environment_defects_are_env_failure_not_model_failure` に 2 例追加。
- 能力 probe に `autotools`（autoreconf/automake/libtoolize）と `libclang` を追加した。これらが無いと K2 の build は通らないのに、従来の 8 能力は全部 ok になり得た
  （LLM 呼出しの前に止められなかった）。

## 3. cap-probe（人の実走。scratch の cap-{a,b,c,k4}.{json,err}）
cap env: ro_mounts なし、sockets=[/run/celeris-cap/sirius-ssh.sock]、PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig（libucx-dev 1.18.1 は /usr に apt で導入）。
| arm | cc | c++ | cmake | git | ucx-pkg | min-build-ucx | min-build-cmake | sirius-link | 全体 |
|---|---|---|---|---|---|---|---|---|---|
| K2 A | ok | ok | ok | ok | ok | ok | ok | exit 255 | env_failure、paid_calls_made 0 |
| K2 B | ok | ok | ok | ok | ok | ok | ok | exit 255 | 同上 |
| K2 C | ok | ok | ok | ok | ok | ok | ok | exit 255 | 同上 |
| K4 A | python3 ok、python3-stdlib ok | | | | | | | | ok（exit 0） |
sirius-link の原因: sandbox 内で `/etc/ssh/ssh_config.d/20-systemd-ssh-proxy.conf` が userns で nobody 所有に見え ssh が `Bad owner or permissions` で拒否。
修正: case.json の sirius-link を `ssh -F /dev/null -S <socket> -O check sirius` にした。この host で同じ引数を直接実行して `Master running`・exit 0（A の実 sandbox での再実走は人）。
autotools・libclang は上の実走の後に足した能力なので、実走はまだ無い（この worker host には両方無いことを上で確認）。

## 4. 人が次にやること
1. asset store に `submodules/ucx-938ffcd1.tar`（本 run の artifacts/ucx-938ffcd1.tar。`git archive` した UCX 938ffcd1、sha256 8cab367e…413a2）を置き SHA256SUMS に追記。旧 SHA256SUMS は残す。
2. host に `autoconf automake libtool libclang-dev`（と UCX を source build する時間）を入れる。入れずに `--build` を流すと env_failure で止まる（正しい動作）。
3. `verify-real-assets.sh <k2 dir> --build` と cap-probe（A/B/C）を再実走。
4. grader/lock の入力が変わったので、測定 lock は新しい測定 ID で作り直す（旧 lock・旧試行に混ぜない）。測定 ID は本測定の承認時に人が決める。

## 5. 全体検査
- `bash scripts/dev/test-parallel.sh`: exit 0（passed 5754 / failed 0 / ignored 15）。`cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- lock が 5 件になったので `campaign_lock`・`k2_pin`・`k_cases_grade` の実 asset 試験を入れ子対応にした（本体 gitlink は最上位 3 件、snapshot 名は `asset_assembly::snapshot_name`）。
  asset store に ucx snapshot が未配置の間は、実 store を使う 2 試験が `SKIPPED(asset 未配置)` で戻る（合格扱いではない。配置後に走る。組立の照合は verify-real-assets.sh が env_failure で止める）。
