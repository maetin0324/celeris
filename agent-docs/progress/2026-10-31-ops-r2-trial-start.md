---
title: K2 ops-r2 の試行開始の残り（promotion gate・初期作業ツリー・sandbox の ssh・試行 A の起動）
tasks: [01M69BAM9SMW8YE1Y4GJXV0NFT]
status: running
started: 2026-10-31
updated: 2026-10-31
---

# K2 ops-r2 の試行開始の残り

## やったこと
- 項目 1（promotion gate）: `scripts/campaign/ops-r1-measure-config.sh` が `$STATE/release-store/releases/<sha12>`（→ release dir）と `$STATE/release-store/current`（→ `releases/<sha12>`）を作り、`[selfdeploy] releases_dir` をその写しに向ける。本番の release store は変えない。ADR 付記 D-store。
- 項目 3（sandbox の ssh）: `scripts/campaign/ops-r2-sandbox.sh --ssh-config <file>`。ADR 付記 D-ssh。
- 項目 2（初期作業ツリー）: `scripts/campaign/ops-r2-seed.sh`（make/place/unpack、.git なしの非圧縮 tar、sha256 照合、危ない member の拒否）。ADR 付記 D-seed。
- 項目 6（K4 文言）: 生成器の `setup/project.json` と daemon.toml 冒頭注記を `ops-r2 K2` に。ADR 付記 D-k2text。
- 項目 5（intake の decision_id）: 書式 `approval:<承認票の task id>:<file 名>:<選択肢 key>` を手順書 §5 と ADR 付記 D-decision に書いた（コード変更なし）。

commit（件名）:
- ops-r2: sandbox の --ssh-config（-F なしの ssh sirius）と測定 config 生成の K2 文言
- ops-r2: 測定 config 生成が state の下に release store の写し（current → sha12）を作り releases_dir をそこへ向ける
- ops-r1 measure 試験: release store の写しと releases_dir を確かめる
- ops-r2: intake の decision_id を承認票の参照で書く書式を決める
- ops-r2: A・B の初期作業ツリーの種（.git なしの非圧縮 tar）を作り・置き・展開する ops-r2-seed.sh

## daemon.toml の差分（main の生成器との比較）
同じ入力（合成 release `0e2e0e2e0e2e`、`--models cos-opus --harness stub --port 18999`、state の path だけ置き換えて比較）で
`git show main:scripts/campaign/ops-r1-measure-config.sh` と現在の生成器の出力を diff した。違いは 2 行だけ:

```
1c1
< # ops-r1 測定 daemon の config（…）
> # ops-r2 K2 測定 daemon の config（…）
33c33
< releases_dir = "<release dir の親>"
> releases_dir = "<state>/release-store/releases"
```

1 行目は注記（コメント）で daemon の挙動に効かない。設定の欄で変わるのは `releases_dir` だけ。

## 証拠（提出時の HEAD で実行）
- `cargo test -p celeris --test ops_r1_measure`: exit 0、1 passed（生成した daemon.toml の releases_dir・current の行き先・元の release store の中身が sha12 1 つのまま、を確かめ、その config で daemon を起動）。
- `cargo test -p celeris-campaign --test ops_r2_seed`: exit 0、3 passed。
- `cargo test -p celeris-campaign --test ops_r2_ssh_config --test ops_r2_socket`: exit 0、6 passed。

## 未解決事項
- 項目 4（試行 A の起動 command）、手順書の反映、test-affected・clippy は未実施。

## 項目 4・手順書の反映（Run #6）
- commit「ops-r2: 試行 A の起動 command を出す ops-r2-trial-a-command.sh と手順書の反映（ssh-config・release-store・種）」
- 試験: `cargo nextest run -p celeris-campaign --test ops_r2_trial_a_command` → 1 passed。`cargo fmt --all -- --check` → exit 0。
- 未実行: `bash scripts/dev/test-affected.sh`（run の壁時計 600 秒に収まらず次の run で実行）。`cargo clippy --workspace --tests` は既存の
  `cap_probe`（duplicate_mod）・`ops_r2_probe.rs:212`（needless_borrow）で落ちる。この task の変更ではない。

## 本番への移行（operator が実行する）
1. 新しい release（この branch を取り込んだもの）の道具で §2.2 をやり直し、`$S/release-store/current` が `releases/1239fab2bc36` を指し、daemon.toml の差分が `releases_dir` だけであることを確かめる。本番の `/local/celeris/state/releases` は変えない。
2. §6.1 で種を A・B の state に置き、§3・§4 をやり直す（漏洩検査が tar の中身を読む）。
3. 専用 ssh config に `ControlPath`・`UserKnownHostsFile` を書き、§4.1（step 8）を `--ssh-config` の形で `-F` なしにやり直す。
4. §6 の形で測定 daemon を起動し、health の `cos_policy_version` が "10" であることを確かめる。
5. §6.2 の command を出し、人が別の端末で試行 A を起動する。
