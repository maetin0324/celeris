---
title: "測定環境 launcher: 接続先 daemon の同一性照合と固定記録の設計（identity-design）"
tasks: [01M56HCNS9SYAS7TAEA7Z3E4CZ]
unit: identity-design
status: done
updated: 2026-10-18
---
# identity-design（調査と設計のみ。コード変更なし）

完了日: 2026-10-18。結論は ADR `agent-docs/adr/2026-10-18-measure-env-launcher.md`。

## 結論の要点
- (1) 設定源: `measure-env.toml` 1 つ。`daemon_config`（測定 daemon の celeris config）から api_url（`[api] listen`）・
  token（`[api] token_file` を要求直前に file から読む）・state（db・workspace_root・knowledge root・memory dir）を、
  `[assets]` から sandbox 資産を導出。API を使う campaign subcommand は `--env` だけ。`--api-url`・`--token-env`・
  `--measure-config` 等の重複欄は削除（渡すと非 0）。測定 daemon は CLI 上書きなしで起動。
- (2) 同一性: **task-api への追加は要る**。`GET /api/v1/config` に `identity`（instance_id・release・pid・started_at・
  config_sha256・overrides・秘密なしの effective）を 1 つ足す。照合は 1 回の GET の値と、`/proc/<pid>/{stat,exe,cmdline}`
  （launcher.json の pid・starttime・exe sha256）、`/proc/net/tcp{,6}` の LISTEN inode の所有者が全部その pid（SO_REUSEPORT
  相乗りの拒否）。`/health` は変えない。新 endpoint は作らない。
- (3) 固定記録: `<out>/env-lock.json`。A（path・sha256・版・account）、B/C（release・写した bin の hash・daemon の exe/config/
  effective/state hash・worker CLI の path/sha256/版・arm ごとの routing 宣言＋カタログ tier 対応＋根拠表 hash）。
  再開時は instance_id/pid 以外を全部照合。A の実体は試行ごとに起動直前に hash し直す。
- (4) launcher: `scripts/campaign/measure.sh {prepare,start,preflight,run,grade,finalize,status,stop} --env`。
  release の `bin/` を state_dir に写して起動（本番 releases の prune と昇格の認可から切り離す）。停止は launcher.json の
  pid 1 つだけ、starttime・exe・cmdline 照合後に SIGTERM→上限で SIGKILL。systemctl・pkill は使わない。
  account は作らない・消さない（作業 1 の対応表を env ファイルに置く形）。

## 調べた事実（cae27fc073a5）
- `ConfigView` は起動時に 1 回作る静的値（`crates/celeris/src/daemon/api.rs:24` `config_view`）。knowledge・memory・
  selfdeploy・llm_proxy・overrides・config hash・pid は無い。
- `celeris` の CLI 上書き `--db/--listen/--workspace-root/--token-file`（`crates/celeris/src/main.rs`）は config を読んでも見えない。
- `[llm_proxy] listen` には既定値があり（`crates/llm-proxy/src/config.rs`）、測定 config で省略すると本番と同じ port に
  相乗りする（ADR-0139 の型）。`[selfdeploy]` の既定は本番の releases_dir・checkout・push=true。
- 昇格の認可は releases_dir の外の release 名を「unmanaged」で判定外にする（`crates/celeris/src/instance.rs` `decide_promotion`）。
- `ResolvedProgram` は `{path, version}` だけで sha256 を持たない（`crates/campaign/src/preflight.rs`）。
- `CurlTransport` は token を環境変数から読み curl の stdin config に載せる（argv に出ない）。

## 証拠コマンドと結果
- 調査のみ。`git diff --stat $CELERIS_WU_BASE` で変更は ADR と本ファイルの 2 件だけ（範囲 check を下に記録）。
- 試験・clippy・fmt は Rust コードを変えていないため、この WU では走らせていない（Phase 完了時の検査は後続 WU と統合段で行う）。

## 未解決事項
- 作業 1（試行 identity 別 account 状態）の最終形が未確定。env ファイルの account 節と env-lock の `a.account` は
  作業 1 の対応表に合わせて配列化する前提で、launcher 側は account を変更しない。
- 写す release の中身が `bin/` だけで足りるか（task-worker・helper scripts の探索 path）を launcher WU が実起動で確認する。
- `[accounts]` dir は本番と共有し得る（A の account を使うため）。共有の範囲は作業 1 の決定に従う。
- e2e で実 daemon を起こす試験が worker sandbox 内で走るか（userns 不要だが bwrap・port の制約）を launcher WU が確認する。

## 提案
- 本番 runbook（run-final.md）で本番 daemon に接続して測定する手順は、runbook WU で launcher の手順に置き換えて消す。
