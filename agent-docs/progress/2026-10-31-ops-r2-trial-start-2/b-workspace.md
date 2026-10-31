---
title: ops-r2 B の案件 repo・preflight の --ssh-config・種の展開から daemon 起動前までの試験
tasks: [01M69YZVW2BBP82R6ZY45BMW3J]
status: done
started: 2026-10-31
completed: 2026-10-31
updated: 2026-10-31
---

# B の案件 repo・preflight の --ssh-config・B の daemon の起動（親 task 01M69YXTFWZT56XCZFY2PCPZMB の項目 4・5・6）

完了日: 2026-10-31

方針は ADR [2026-10-31-ops-r2-preflight-fixes](../../adr/2026-10-31-ops-r2-preflight-fixes.md) の付記 D-bwork・D-bssh・D-bstart に書いた（実装の前）。

## 変更
- 項目 4（B の案件 repo）: `scripts/campaign/ops-r1-measure-config.sh` が `setup/project.json` に
  `"workspace": {"kind": "local", "path": "<state>/work/benchfs"}` を書く（JSON は python3 で書くので path の `"`・`\` も壊れない）。
  path は A（`ops-r2-trial-a-command.sh`）と同じ `<state>/work/benchfs` に固定し、新しい引数は足していない。
  `POST /api/v1/projects` はこれを primary repo（名前 `benchfs`）にし、`.git` が無いので種類は `dir`（run には木への symlink）になる。
- 項目 5（preflight の ssh config）: `ScriptSandbox.ssh_config` と CLI の `--b-ssh-config`・`--operator-ssh-config` を足し、
  `ops-r2-sandbox.sh` に `--ssh-config` として渡す（`--print-args` と probe の起動の両方。許可表は従来どおり `--print-args` から作る）。
- 項目 6（B の起動）: 新しい `scripts/campaign/ops-r2-b-daemon-command.sh` が B の daemon の起動 command を 1 行出す（起動はしない）。
  包みは `--ro <claude の dir> --ssh-config --socket`、中身は `ops-r2-seed.sh unpack --into <S>/work/benchfs && exec <R>/bin/celeris --config <S>/daemon.toml --release <sha12>`。
  出す前に種・seed script・daemon.toml・release の celeris と sha12・release-store の形・project.json の workspace・作業ツリーが無いか空かを確かめ、欠ければ exit 1。
- 試験:
  - 新しい `crates/campaign/tests/ops_r2_b_start.rs`（3 件）。手順書の順に実物の measure-config（stub）・`ops-r2-seed.sh make|place`・A と B の command script を走らせ、
    出た 1 行を sh で評価する。偽の celeris・claude が起動時の作業ツリーの一覧を記録するので、daemon の起動前に展開が済むこと、submodule の中身があり `.git` が無いこと、
    A と B の木が同じこと（相対 path・中身）、project.json の workspace がその木を指すことを確かめる。path は空白・`'`、socket は `@`・`:` を含む。
    前提の欠け（release-store の不一致・workspace 無し・種無し・2 回目）は exit 1、引数の誤りは exit 2。
    実 bwrap の包み（実物の `ops-r2-sandbox.sh`）で展開から exec までを通す試験は `CELERIS_USERNS_TESTS=1` のときだけ走る。
  - `crates/campaign/tests/ops_r2_ssh_config.rs` に、preflight の `run` が `--ssh-config` を `--print-args` と probe の起動の両方に渡す試験（偽の sandbox script）。
  - `crates/celeris/tests/ops_r1_measure.rs`: 展開済みの木の代わりを置いて daemon を起動し、`GET /projects/{id}/repos` が `benchfs`・`dir`・primary・その path を返すことを足した。
    木が無いと worker の起動が `No such file or directory` で失敗した（この run で再現）。B の起動 command が展開を daemon の前に置く理由。
  - `scripts/campaign/test_ops_r1_measure_config.py`: project.json の workspace を確かめる。`releases_dir` の期待値は付記 D-store の後も旧い値のままで落ちていたので、`<state>/release-store/releases` に直した。
- 文書: `docs/ops/ops-r2-k2-preflight.md` §3（`--b-ssh-config`。`--b-ro "$SSHCFG_DIR"` をやめ daemon と同じ形）・§4（probe の包みも `--ssh-config`）・§4.1・§6（新しい script、案件の repo の確かめ方）・§6.1、
  `docs/ops/ops-r1-measure-daemon.md`（project.json の workspace）、`docs/ops/ops-r2-isolation.md`（preflight の ssh config）。

commit（件名）:
- ops-r2: B の案件 repo を種の作業ツリーにし、preflight に --ssh-config を足し、B の daemon の起動 command と試験を足す
- 進捗: b-workspace の提出時の検査結果

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `cargo test -p celeris-campaign --test ops_r2_b_start` → exit 0、3 passed（実 bwrap の試験は skip を出して通過）。
- `CELERIS_USERNS_TESTS=1 cargo test -p celeris-campaign --test ops_r2_b_start real_sandbox -- --nocapture` → exit 0、1 passed（skip の行なし。実 bwrap で展開から exec まで通った）。
- `cargo test -p celeris-campaign --test ops_r2_ssh_config` → exit 0、4 passed。
- `cargo test -p celeris --test ops_r1_measure` → exit 0、1 passed。
- `python3 -m unittest scripts/campaign/test_ops_r1_measure_config.py` → OK、16 tests。
- `bash scripts/dev/test-affected.sh` → 下の「提出時の検査」。
- `cargo clippy --workspace -- -D warnings` → 下の「提出時の検査」。
- `cargo fmt --all -- --check` → 下の「提出時の検査」。

## 未解決事項
- 本物の release の `bin/celeris` が B の包みの中で `--config` を読んで起動することは実機で通していない（試験の daemon は偽物）。
- B の CoS が実機でこの `dir` の repo を選んで起票すること、`dir` の repo の task で delivery（取り込み）が何をするかは確かめていない（試行の成果は作業ツリーの状態で採点する）。
- §4.1 の専用 ssh config の `UserKnownHostsFile` は `$SSHCFG_DIR` の下の file を指す。B の包みには config の file だけを見せるので、known_hosts は包みの中から読めない。
  既存の master の socket（`ControlPath`）経由の接続は host key を照合し直さないので通る見込みだが、実機では未確認。通らなければ §4.1 で止まる。

## 本番への移行（operator が実行する）
1. B の state を新しく作り直し、`ops-r1-measure-config.sh` を今の版で走らせる（`setup/project.json` に workspace が入る）。
2. §3 の preflight を `--b-ssh-config "$SSHCFG_DIR/config"` 付き（`--b-ro "$SSHCFG_DIR"` なし）でやり直す。§4 の B の probe も `--ssh-config` の形でやり直す。
3. §6 は `ops-r2-b-daemon-command.sh` が出す 1 行で起動し、案件を作った後に `GET /api/v1/projects/<id>/repos` で `dir`・`$S/work/benchfs` を確かめる。

本番 config・DB・release store・sirius はこの task では変えていない。

## 提案
- なし
