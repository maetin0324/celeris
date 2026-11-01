---
title: K2 ops-r2 の開始前チェック修正 — WU docs（手順書の更新と本番への移行）
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
status: done
updated: 2026-10-31
---
# WU docs

完了日: 2026-10-31

## 変更
- `docs/ops/ops-r2-k2-preflight.md`: 実装済みの引数名に合わせて §0・§2〜§4・§6〜§8 を書き直した。旧い手順・旧引数は残していない。
  - §2.1 local の claude copy（`--version` と sha256 を承認票と照合。NFS の claude は使わない）。§2.2 `--policy-version`（必須。K2 ops-r2 は 10）・`--b-account-dir`（`$S/pool` に b だけの symlink）・`--claude-bin`。
  - §3 `celeris-campaign preflight` の引数を ops-r2 の形にした（K2 ops-r2 manifest `ops-r2-k2.json`、`--deny-list` の file、`--claude`・`--claude-sha256`、`--b-ro`・`--operator-ro`）。api-url・token・task-template・canaries・cap-env は要らない。`measure-launcher.sh preflight` は `--claude` を渡せないので使わない。
  - §4 B の probe に `--allow` を入れ、`--deny-readable-file`・`--eval-root` を使う形にした。L1 の `mounts_outside_allow` は hit にしない。same_uid は canary なしで 4 条件（runtime probe・L1 probe の合格）により外す。bwrap の中から評価器の sudo が効かないことは 2026-10-31 の実機で確認済み（exit 1）。
  - §4.1 sirius への接続: B の包みの中から ssh・qsub・qstat が使えるかを確かめる手順。通らなければ試行・daemon 起動を止めて人に返す。
  - §6 測定 daemon を包みの中で起動し、health の方針版を確かめる手順。§7 の停止規則に 7（試行と無関係な CoS run）・8（sirius 不達、claude copy 不一致）を足した。
- `docs/ops/ops-r2-isolation.md`: §2.1〜§2.4・§4.7 を `--deny-readable-file`・`--allow`・`--claude`/`--claude-sha256`・`--eval-root`・same_uid 規則・L1 mount 非 hit・pool に合わせた。
- `docs/ops/ops-r1-measure-daemon.md`: 旧い B account 引数を `--b-account-dir`、固定の方針版 6 を `--policy-version` に直した。stagnation 等の off を注記した。
- `docs/ops/ops-r1-leak-check.md` は leakdoc unit の担当なので触っていない。
- commit 件名: `docs: ops-r2 の手順書を実装済みの引数に合わせ、本番への移行を書く`

## 本番への移行（operator が §2〜§6 をやり直す手順）

前提: 人の準備（`ops-r2-k2-preflight.md` §0）が済み、承認票の欄が埋まっている。この変更が入った release（または repo の clone）の script と固定した `celeris-campaign` を使う。
本番 daemon・本番 DB・`~/.config/celeris` は変えない。変数は `ops-r2-k2-preflight.md` 冒頭のとおり。2026-10-31 の旧 state `/local/celeris/data/measure/ops-r2-k2-p1/{b,operator}` と旧 `daemon.toml`・旧 `preflight/` の証跡は消さず、使わない。

1. **新しい state dir と証跡 dir を作る。** `S=/local/celeris/data/measure/ops-r2-k2-p2/b`、`O=/local/celeris/data/measure/ops-r2-k2-p2/operator`、`P=<新しい証跡 dir>`（旧 dir を再利用しない。`--state` は試行ごとに新しい dir）。
2. **§1 を再実行する**（private の sha256、release dir に private 系 0 件）。release が変わったなら新しい `R` で。
3. **local の claude copy を確かめる**（§2.1）。`$CLAUDE --version` が 2.1.303、`sha256sum` が manifest の `claude_code_sha256` と承認票に一致。違えば止めて人に返す。
4. **config を作る**（§2.2）。`--policy-version 10 --b-account-dir /local/celeris/measure-accounts/b --claude-bin "$CLAUDE" --out "$S/daemon.toml"`。
   `policy_version = "10"`、`ls -A "$S/pool"` が `b` だけ、`principal = "human:` が 0、stagnation 4 閾値が 0 を確かめる。
5. **全件の deny list を作る**（§3）。承認票の deny list を実在の path に展開して `$P/deny.list` に 1 行 1 path で書く（7129 件でも、9000 件を超えても可）。`sha256sum` を steps に残す。
6. **隔離 preflight**（§3）。`celeris-campaign preflight --manifest crates/campaign/fixtures/frozen-manifests/ops-r2-k2.json ... --deny-list "$P/deny.list" --claude "$CLAUDE" --claude-sha256 <値>` を直接実行し、exit 0・`missing` 0 件を確かめる。
   `--repo-src`・`--inputs-src` の実物を渡した実行は未確認。`graders` や `input-manifest` の missing が出たら、出力を steps に残して止め、人に返す。
7. **漏洩検査**（§4）。B の probe（`--allow` 付き、`--deny-readable-file`）と L1 の probe を取り、`--eval-root` 付きで `check-ops-r1-leak.py` を評価器の uid で走らせる。exit 0・`pass: true`、`l1_mounts_outside_allow` は報告のみ。
8. **sirius への接続**（§4.1）。B の包みの中から `ssh -o BatchMode=yes <sirius> 'echo ok; command -v qsub qstat'`。通らなければ試行・daemon 起動を止め、exit code と stderr の 1 行目を添えて人に返す（ssh の鍵・config・socket を許可表に足すかは人が決める）。
9. **ここまでが全部通ったとき**、manifest の `campaign_sha256` 等を承認票に結んだ人の確認を取り、**測定 daemon を B の包みの中で起動**する（§6）。health の `cos_policy_version` が `"10"` であることを確かめる。違えば §7-6 で止める。
10. **operator principal と出典**（§5）、**session prep**（§6、`--claude-bin "$CLAUDE"`、`--policy-version 10`、`--api-url` は daemon の port）。`SHA256SUMS` が全 OK。承認票の実験専用 session の id が `ticket.json` と一致。
11. §8 のとおり証跡と sha256 を進捗に残す。

実機で確認できていない点（提出時点）: ops-r2 の preflight を実物の `--repo-src` 等で通すこと、B の probe の `--allow` を手書きで組む snippet、包みの中での `bin/celeris` の起動、sirius への接続、`transcript_expected` の実物照合。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 文書だけの変更（手順書 3 件・進捗 1 件）。
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし、check-doc-links self-test ok）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-doc-links.sh` → ok、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok、`python3 -I scripts/dev/check-human-text.py` → 0 findings、`sh scripts/dev/progress-index.sh --check` → ok
- 範囲 check（`CELERIS_WU_SCOPE_PATHS`）→ 変更 path は手順書 3 件と自分の進捗だけ
- 引数名は `crates/campaign/src/main.rs`・`runtime_probe.rs`・`preflight_isolation.rs`、`scripts/campaign/ops-r1-measure-config.sh`・`check-ops-r1-leak.py`・`ops-r2-session-prep.sh` の実装を読んで合わせた

## 未解決
- `scripts/campaign/measure-launcher.sh` に `--claude`・`--claude-sha256` の pass-through が無い。手順書は `celeris-campaign preflight` の直接実行にした。launcher を使いたければ別 task で足す（probe unit の提案どおり）。
- `ops-r1-leak-check.md` は leakdoc unit が直す（blocked）。そこに旧引数が残っていれば統合段で確認する。

## 提案
- B の probe の `--allow` を `ops-r2-sandbox.sh --print-args` と host の mountinfo から作る script を足す（今は手書きの snippet）。
