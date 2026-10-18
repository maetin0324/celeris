---
title: P1-3 cap-runbook（人が host で実行する能力 probe・実資産検証・接続到達確認）
tasks: [01M56HCNTPN0H2VGP4JE0JYRM9]
status: 一部実走済み（real-evidence.md）。残りは §2 の再実走
updated: 2026-10-18
---
# cap-runbook

worker sandbox は userns が使えないため、実 A sandbox 内の実走と実 build は人が host で行う。
**起動してよいのは無 LLM の確認だけ**。有料ジョブ・本測定・LLM 呼出し・Sirius への job 投入はしない。
本番 host（systemd・本番 config・release・本番 DB/knowledge）は変更しない。鍵・認証材料の値は出力・記録に貼らない。
前提: 最新の本ブランチを build 済み（`CARGO_TARGET_DIR` はローカル scratch。NFS に target を作らない）。

## 0. cap env の用意（承認が要る値）
`$WORK/cap-env.json`（`$WORK` は一時 dir。git に入れない）。`a` と `worker` は同じ集合でなければ読み込み拒否される。
```json
{"schema":"celeris.campaign-cap-env/1",
 "a":     {"ro_mounts":["<UCX prefix>"],"sockets":["/run/celeris-cap/sirius-ssh.sock"],"env":[["PKG_CONFIG_PATH","<UCX prefix>/lib/pkgconfig"]]},
 "worker":{"ro_mounts":["<UCX prefix>"],"sockets":["/run/celeris-cap/sirius-ssh.sock"],"env":[["PKG_CONFIG_PATH","<UCX prefix>/lib/pkgconfig"]]}}
```
- `/`・`/home`・HOME を覆う path・HOME の上書きは拒否される（ホーム全公開・sandbox 無効化の経路は無い）。
- `/run/celeris-cap/sirius-ssh.sock` は**承認済みの管理接続**（ControlMaster の socket）。人が事前に確立する。鍵・`~/.ssh` は mount しない。UCX prefix・socket path の実値は承認が要る。

## 1. cap-probe を実 A sandbox で実走（A/B/C）
```sh
cd <worktree>
for arm in a b c; do
  cargo run -p celeris-campaign -- cap-probe --case k2-bf-xattr-restart-campaign --arm $arm --cap-env $WORK/cap-env.json > $WORK/cap-$arm.json; echo "arm=$arm exit=$?"
done
cargo run -p celeris-campaign -- cap-probe --case k4-cmp4-validity-analysis --arm a --cap-env $WORK/cap-env.json
# argv だけ確認（実行しない）: --dry-argv
```
期待: 各 arm exit 0、JSON `ok: true`、K2 の 10 能力（cc, c++, cmake, git, ucx-pkg, min-build-ucx, min-build-cmake, autotools, libclang, sirius-link）が全て ok。K4（python3 と stdlib）も ok。
A と B/C の probe argv（`--dry-argv`）が同一であること（B/C は A と同じ能力を与える設計）。
不足時: exit 非 0、`failure_class: env_failure`。この場合は LLM 呼出し前に停止（preflight の `capability` check）。ro_mounts / 接続 socket を足して再実行し、sandbox を緩めて通さない。

## 2. K2 実 asset の組立・ビルド・正負例（verify-real-assets.sh）
asset store の k2 dir（`SHA256SUMS`・`benchfs-*.bundle`・`submodules/*.tar`）と、known-good 用 `k2-fix.patch`（k2-known-good-v2、sha256=6fc5d09a880ce405f4f81cbbf559df2b2bcf191946e14040c05705de63d22773。p0-final.patch 単独は古く known-good が落ちる。最新結論は 2026-10-18-p1-3-known-good-fix.md）、K4 原票 `main-v3-n2-43706.tgz` を用意する。
```sh
export CAMPAIGN_K2_FIX_PATCH=/local/celeris/data/campaign-assets/k2/k2-fix.patch> CAMPAIGN_K4_TGZ=<main-v3-n2-43706.tgz>
sh crates/campaign/scripts/verify-real-assets.sh <asset_dir>          # 組立・lock・patch・K4 再計算（build なし）
sh crates/campaign/scripts/verify-real-assets.sh <asset_dir> --build  # 実 cargo build/test（UCX 等が要る）
```
期待（build なし）: SHA256SUMS ok、lock 一致、known-good patch 適用、正負の印、K4 raw の hash と再計算が ok、exit 0。
期待（`--build`）: known-bad は公開/隠しテストで落ち、known-good は通る。いずれも exit 0。
既知の不足（real-evidence.md）: 入れ子 snapshot は async-ucx と `ucx1-sys/ucx`（`submodules/ucx-938ffcd1.tar`）の 2 つが lock にある。ucx 側を asset store に置く。host に autoconf・automake・libtool・libclang-dev が要る（無いと env_failure）。asset/grader を変えたら新しい測定 ID で lock を作り直し、旧 pilot の試行と混ぜない。

## 3. Sirius 接続の到達確認のみ
```sh
ssh -F /dev/null -S /run/celeris-cap/sirius-ssh.sock -O check sirius
```
期待: `Master running (pid=...)`、exit 0。確認のみで、sbatch/qsub・job・コマンド実行はしない（cap-probe の `sirius-link` と同じ経路）。TOTP が要る場合は人が通常手順で master を確立する。

## 4. 結果の記録
各手順の exit code と出力要点（鍵・値は除く）を `README.md` の区分表の該当行に追記する（実経路確認済みに上げるのはここ）。

## CAP_ENV の固定（env-lock/2）
CAP_ENV は env-lock に正規化して固定される。変更すると同じ測定の watch は外部実行前に `env-lock mismatch` で止まる。旧 `/1` の lock は拒否され、試行 data が無い場合のみ `env-lock.json` を消して再作成する。詳細は ADR 2026-10-17-campaign-lock 付記。
