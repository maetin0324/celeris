---
title: CoS chat 最適化 7 — resume 時の差分配送
tasks: [01M4DE3G78D16NJ80SEMWVAVAK]
status: done
updated: 2026-10-08
---
# resume 時の差分配送（T6 / H3）

設計・決定は [ADR 2026-10-05-cos-chat-home 付記](../../adr/2026-10-05-cos-chat-home.md)「resume 時の差分配送」。

## 完了したこと
- migration `0063_cos_chat_delivered_through`（schema 63）: `node_sessions.delivered_through_seq`（配送 cursor）。completed の run の終端でだけ上げる。
- resume で cursor が分かれば summary を省き cursor 以後の差分だけを渡す。new・fresh（4 理由）・resume 拒否後の fresh 再試行・再起動後の回収・cursor 0 は全文。
- worker protocol `CosChatContext.delivered_through_seq`（schema 再生成）と prompt の差分節。
- 試験 `cos_chat_resume_delta_` 11 件（task-dispatch 9・task-core 1・task-worker 1）。
- `scripts/dev/cos-chat-bench.sh` に mode `same-thread`（同一 thread 10 turn だけ）を追加。

## 証拠
- `cargo test -p task-dispatch -p task-core cos_chat`: exit 0（task-dispatch lib 103 passed など、失敗 0）。
- `cargo test -p task-dispatch cos_chat_resume_delta`: 9 passed。`cargo test -p task-worker cos_chat`: 44 passed。
- `cargo nextest run -p task-core -p task-dispatch -p task-worker -p celerisctl --no-fail-fast`: 2967 run、2929 passed、38 failed（全て browser・launcher の sandbox 既知失敗。cos_chat は 0 件）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- live（claude_oauth、隔離 data dir・一時 DB・port 17957）: before = base 8cdd96fb のバイナリ、after = この変更。各 2 回。t2〜t10 合計の平均: 非 cache input 50 → 50（±0%）、cache write 51,647 → 44,630（−13.6%）、cache read −5.6%、prompt bytes 55,064 → 43,457（−21.1%）。turn 別の値は task の artifacts `resume-delta-bench/compare.md`（runs.json 一式あり）。after の DB で session_mode は 1 turn 目 new・以後 resumed、cursor は 10 turn 後に 21。

## 人の決定（2026-10-08）

**質問**: 差分配送（commit b9446a53、試験 11 件・clippy 合格）は実装済みだが、統合条件の非 cache input は 50→50 で改善なし。どれにしますか: (a) 判定指標を cache write・prompt bytes に変えて統合する（推奨）、(b) 統合せず task を閉じる、(c) summary がある thread での追加計測を別 task で行い結果で決める。

**人の回答**（2026-10-08）:

> 受け入れ条件 3 の末尾を「非 cache input・cache write・prompt bytes の before/after が記録され、cache write と prompt bytes のどちらかが改善している」に書き換え、目的の「改善が無ければ統合せず yield」も同じ趣旨に直し、(a)の方向性で進めてください

- **選んだ選択肢: (a)** — 判定指標を cache write・prompt bytes に変えて統合する。
- **理由**: claude-code では非 cache input は構造上動かない（prompt が cache write に入り turn ごとに 4〜6 で一定）ため、この変更の判定指標として不適。新しい基準では改善あり（cache write −13.6%、prompt bytes −21.1%）。
- task の受け入れ条件 3 と目的の文言更新は**人が API で行う**。
- 本決定記録の run では製品コード（crates/・migration・schema）は一切変更していない。統合は delivery の流れの後続段で実施する。

## 未解決事項
- 非 cache input（`Usage.input_tokens`）では改善が無い（claude-code は prompt が cache write に入るので構造上ほぼ一定）。判定指標の扱いは 2026-10-08 の人の決定（上の「人の決定」節）で (a) に決まった。
- 計測中、CoS は checkpoint を書かず summary は空だった（既知の不具合 4）。summary がある thread の効果は未計測。
- codex・acp・pi の live は未計測（不明）。

## 提案
- 差分配送の判定指標は非 cache input でなく cache write と prompt bytes にする（baseline の提案と同じ）。この基準なら改善あり（−13.6%・−21.1%）。（2026-10-08 の人の決定で採用。）
