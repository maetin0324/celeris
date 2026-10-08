---
title: CoS chat 最適化 0 — 現行実装の確認・仮説表・計測設計（ADR 草案）
tasks: [01M4D3XKDS8DK9QEK081QKA8VE]
status: done
updated: 2026-10-08
completed: 2026-10-08
---
# CoS chat 最適化 0（ADR 2026-10-08-cos-chat-prompt-cache 草案）

## 完了したこと

- main `9d2a73141f0e`（skill 配送の hash 照合 9d2a7314 を含む）で、CoS chat の起動（`dispatcher/cos_chat/launch.rs`・`rollover.rs`）、prompt の組み立て（`task-worker/src/cos_chat.rs`・`claude_code.rs`・`codex.rs`・`acp.rs`・`pi.rs`・`skills.rs`）、chat run の保存（`0050_cos_chat.sql`・`chat/session.rs`）、`/cos/operations` の `ALLOWED` を読み、[ADR 草案](../adr/2026-10-08-cos-chat-prompt-cache.md)の D1 に書いた。
- LLM を呼ばない決定的な計測（$TMPDIR の scratch crate から `task_worker::claude_code::build_prompt` を呼んだ。repo には入れていない）: 新規 thread の 1 turn 目は 4,927 B（固定文は約 4.2 KB）。先頭一致は別 thread どうしで 29 B、同じ thread の turn 間で 67 B。summary 3 KB と履歴 18 件の turn では 10,692 B。
- skill の大きさ: repo は cos-operator 20,158 B・cos-inbox-triage 11,386 B。本番 KB の写し（`celerisctl knowledge get`、読み取りだけ）は 17,801 / 8,633 B。依頼文の数値と一致した。
- 仮説 H1〜H6 の検証方法と判定基準、指標・台本・2 種類のベンチ・測れない指標、後続 T1〜T8 の受け入れ条件案を ADR に書いた。

## 証拠

- `test -s agent-docs/adr/2026-10-08-cos-chat-prompt-cache.md` → exit 0
- `bash scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0（新しい file を `git add` した後に実行した。この検査は追跡された .md だけを見る）
- `sh scripts/dev/check-adr-numbers.sh` → ok（166 files）。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok
- `cargo clippy --workspace -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 100（passed 4734 / failed 64 / ignored 14）。失敗した 64 件はすべて browser・launcher・credentiald・CDP relay・scratch の試験だった（worker sandbox の userns・継承環境の制約で、KB に既知として記録がある）。この task の差分は文書だけなので、コードの試験の結果には影響しない

## 未解決事項

- live の計測（隔離 daemon・claude_oauth）はしていない。baseline は T1（usage の保存）の後の T2 で取る。
- `python3 scripts/dev/check-architecture-map.py` は base の時点から NG（`docs/architecture-map.md` の 3 path）。この task では触っていない。

## 提案

- ADR D4 の T1〜T8。とくに T1（chat run の usage の保存）が、残りすべての計測の前提になる。
- cos-operator §3 の表に、`/cos/operations` に登録されていない操作（PUT execution-plan・pause/resume・standing-rules）が載っている。T5 で是正する（登録を足すかどうかは人の決定）。
- 本番 KB の cos skill の写しが repo より古い。同期の手順は人に任せる。
