---
title: ops-r1 漏洩検査（grader-private・旧判定・旧 v1 入力が試行の入力と可読 path に無いこと）
tasks: [01M62TJ5WBDD0VH5Z1EM264NWV]
status: done
updated: 2026-10-29
---
# ops-r1 漏洩検査

完了日: 2026-10-29

棚卸しの不足 g-grader-private-isolation（inventory.json の tooling_gaps、freeze/stop.json の blocking_gaps）を解く。
daemon・task-*・web のコードは変えていない（変更は scripts/campaign/・docs/ops/・agent-docs/ だけ）。測定の daemon は release 94f86928dc13 の binary のまま。

## 変更
- `phase 1: ops-r1 の漏洩検査 script と試験を足す`
  - `scripts/campaign/check-ops-r1-leak.py`: private.json の sha256・12 字以上の文字列値（`.schema`・`.case_id`・`.split` と `--allow-string` は除く）・canary・禁止 hash を scan 先（tar/zip/gzip の member、git の全 object を含む）で探し、`--deny-readable` の path が run の uid/gid から読めるかを判定する。hit があれば exit 1、入力の誤りは exit 2。結果 JSON は pass・hit_count・hits・checked・limits。hit には文字列を書かず needle の id・長さ・sha256 だけを書く。
  - 同じ uid の限界: run の uid が path か祖先の持ち主なら limits に same_uid を書き、代わりの検出として canary を求める（canary が無ければ same_uid_without_canary で exit 1）。
  - `scripts/campaign/test_check_ops_r1_leak.py`: 24 件（合格、文字列・JSON escape・hash 文字列・写しの内容 hash・禁止 hash・chunk 境界・tgz・zip in tar・gzip・git 履歴・symlink・結果 file の除外・可読判定 4 種・入力の誤り 2 種）。
  - `docs/ops/ops-r1-leak-check.md`: 呼び方・検出する kind・限界と代わりの検出・結果 JSON。`docs/ops/mlsys-main-4-ops-r1.md` §2 から 1 行で参照。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `python3 -I scripts/campaign/test_check_ops_r1_leak.py` → exit 0（Ran 24 tests, OK）
- 許可 path の検査（受け入れ条件 1 の command）→ exit 0
- 実物での確認: K4 の private.json で `--scan crates/campaign/fixtures/{inputs,materials,grade-cases} --deny-readable crates/campaign/fixtures/grader-private` → exit 1。grade-cases（grader 側）に `expected.input` が 2 件、grader-private のディレクトリと中の 8 file が可読（同じ uid が持ち主）。公開入力の tgz 名は `--allow-string` で外す運用。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` の結果は報告本文に書く。

## 未解決
- 本番の CoS と worker は人と同じ uid で動くので、grader-private を「読めない」ようにはできない。B の試行の証拠は canary の出現検査（試行の前と後の scan）になる。読んだが何も書き出さなかった場合は検出できない。
- 実際の scan 先（公開入力 v2・KB snapshot・memory・CoS 初期会話・operator 材料・run の作業場所）の path と canary の入れ方は、prep（freeze・preflight・runbook）で固定する。
- 可読の判定は host から見た mode・owner だけで、bwrap の mount namespace は数えない（多めに読めると判定する）。

## 提案
- grader-private を別 uid（例: launcher と同じ方式の専用 uid）が持ち、grader だけがその uid で読む構成にすれば、可読の判定が漏洩の防止になる。daemon 側の変更は要らないが、host の設定（人の作業）が要る。
