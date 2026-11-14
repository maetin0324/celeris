---
title: 自動取り込みを案件ごとからリポジトリごとの切り替えにする
tasks: [01M7BSKR87T3ERB9EJVXBZQTFB]
status: done
updated: 2026-11-14
---
# 自動取り込みを案件ごとからリポジトリごとの切り替えにする

完了日: 2026-11-14（ADR 2026-11-14-repo-auto-integrate。旧 ADR 2026-10-16-project-auto-integrate を置き換え）

## 変更
commit は件名で挙げる（hash は書かない。基準は `$CELERIS_WU_BASE` / `git merge-base HEAD main`）。WorkUnit ごとの詳細は次のファイル。
- 保存層・API: [2026-11-14-repo-auto-integrate/store.md](2026-11-14-repo-auto-integrate/store.md)、[api.md](2026-11-14-repo-auto-integrate/api.md)、[rest.md](2026-11-14-repo-auto-integrate/rest.md)
  - 件名: `store: repo の auto_integrate 欄（migration 0126）と案件の欄の削除`、`api: repo の auto_integrate 欄・422・案件の欄の削除・docs/api の生成物`、`rest: celeris の取り込み判定を repo の欄へ・fixture 置き換え・celerisctl projects show に repo の欄・進捗`
- 消費側: [consumers.md](2026-11-14-repo-auto-integrate/consumers.md)
  - 件名: `consumers: 取り込み判定を repo の auto_integrate 欄から読む 1 関数へ置き換える`
- web: `web: 案件の自動取り込みの切替を消し、リポジトリの設定に移す`
- docs: `docs: 取り込みの説明を repo の欄の判定にする`。`config/celeris.example.toml` の説明コメントは leftovers WU で直した（提案を参照）。
- `docs/SPEC.md`・`docs/architecture-map.md` に案件の `auto_integrate` の記述は残っていない（`grep -n "auto_integrate\|自動取り込み"` が 0 件）。

## 証拠
- 各 WorkUnit の提出時の HEAD での検査結果は上の各ファイルに書いてある（`cargo fmt --all -- --check` exit 0、`cargo clippy --workspace -- -D warnings` exit 0、`bash scripts/dev/test-affected.sh` exit 0）。
- この close-out は文書とコメントだけの変更で、`grep -n auto_integrate docs/SPEC.md docs/architecture-map.md` は 0 件。最終の検査（test-affected・clippy・fmt・progress-index --check）は統合の後の task の review で再実行する。

## 未解決
- `cargo clippy --workspace --all-targets` は task-dispatch の lib test などで、この task が触れていないファイル由来の指摘で落ちる（store WU・consumers WU の報告）。base 側から既にある不備と見ており、未確認。

## 本番への移行
- config 変更: 無し。`~/.config/celeris` の config に `auto_integrate` の欄は無く（`config/celeris.example.toml` を確認。欄は無く、コメントは leftovers WU でリポジトリ単位に直した）、読み込みエラーになる欄は消していない。
- DB: release の起動時に migration 0126 が自動で適用される。`auto_integrate = 1` だった案件の local の git リポジトリは自動で有効になり、取り込みの動作は変わらない。remote・dir のリポジトリは移行後に自動取り込みの対象にならない。事前に確認する SQL は rest.md の「本番への移行」の 1 を参照。
- 運用者の手順（release 昇格の後。昇格は人が押す）:
  1. benchfs-paper（repo id 01M7A3NAAKPM9ECBE2CR14V0BC）を有効にする。
     `PATCH /api/v1/repos/01M7A3NAAKPM9ECBE2CR14V0BC`、body `{"auto_integrate":true}`。
  2. 確認: `celerisctl projects show <BenchFS の案件 id>` の repos 欄で benchfs-paper が `auto_integrate=true`、`benchfs`（remote）が `auto_integrate=false`。
  3. 案件への `auto_integrate` の書き込み（`PATCH /projects/{id}`）は 422 になる。使っている script があれば repo の API に直す。

## 提案
- `config/celeris.example.toml` の `[selfdeploy]` の説明コメントは、リポジトリの欄（`PATCH /api/v1/repos/{id}`、ADR 2026-11-14-repo-auto-integrate）に直した（leftovers WU）。control-ts の `ProjectSnapshot.project.auto_integrate` の型と試験の値も消した。
