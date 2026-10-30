---
title: ops-r2 runtime probe
tasks: [01M66JYQ249SZHNXK0AEVTZNEC]
status: completed
updated: 2026-10-30
---
# ops-r2 runtime probe

完了日: 2026-10-30

## 変更

- commit 件名: `phase 1: add runtime read probe and shared A sandbox arguments`
- ADR 2026-10-30-ops-r2-isolation-principal D3 の probe-read を追加。
- deny path の open と 1 byte read（EOF も成功）、親 dir の readdir を行う。
- mountinfo の source 欄を octal decode して許可表と照合。絶対 path は配下、pseudo filesystem の source 名は完全一致で許可する。重複は除去する。
- source 欄は bind mount 元の host path を示すとは限らない。mountinfo の root 欄や mount した内容全体の安全性を証明するものではない。deny path の実アクセスも必須。
- Sandbox::args と公開 Sandbox::runtime_args が同じ実装を使う。probe binary は workspace に stage して実行でき、A の実行ファイル mount を保持できる。
- probe は stdout に JSON を出し、--out 指定時は同じ JSON を file にも出す。可読・一覧可能・許可外 source のいずれかで exit 1。mountinfo 読み取り失敗・不正引数も exit 1。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `bash scripts/dev/test-affected.sh` → exit 0、2,862 passed・9 skipped、対象 doc test も成功。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo nextest run -p celeris-campaign --test runtime_probe --success-output immediate` → exit 0、5 passed。4 件が常時試験、1 件は userns opt-in 未指定のため本体を実行せず、stderr に skip 理由を出した（nextest は early return を passed と数える）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、変更は campaign の指定 5 file とこの WU 進捗のみ。
- `git diff --check` → exit 0、差分の空白エラーなし。

## 未解決

実 bwrap 内の拒否確認は今回未実施。試験は CELERIS_USERNS_TESTS=1 または require の opt-in。通常は理由を eprintln して skip する。require では namespace が作れなければ失敗する。
本番 host の uid・権限・設定は変更していない。sep-uid 準備手順、preflight 接続、B と operator の起動は別 WU が担当する。

## 提案

なし。
