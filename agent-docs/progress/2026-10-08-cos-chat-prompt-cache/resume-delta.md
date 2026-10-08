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
- 試験 `cos_chat_resume_delta_` 12 件（task-dispatch 10・task-core 1・task-worker 1。merge-main で経路切替の回帰試験を追加）。
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

## 試験の修正（Core 分離後）

**原因**: main の Core 分離（ddc6a895/8ce73e81）で checkpoint の JSON テンプレートが Core へ移り、可変部では「run 固有の操作パラメータ」節に `` `<through_seq>` = 11、`<expected>` = 2（checkpoint） `` の形で出るようになった。`cos_chat_resume_delta_prompt_omits_summary_and_names_the_cursor`（task-worker cos_chat/tests.rs）が Core 分離前の JSON 文字列 `\"expected_summary_through_seq\":2` を prompt 内で探していたため exit 101 で落ちた（同 file の `cos_chat_run_proto_prompt_puts_interrupt_first_and_forbids_actions` は 426 行で同形の可変部断言を使い済み）。

**修正**: 製品コード（prompt 組み立て）は不変。assert を `` p.contains("`<through_seq>` = 11、`<expected>` = 2") `` に置き換え、試験の意図（resume の差分配送でも checkpoint 水位 `summary_through_seq=2` が cursor（`delivered_through_seq`）と独立に渡る）は維持。

**検証**（2026-10-08、修正後）:
- `cargo test -p task-dispatch -p task-core -p task-worker cos_chat_resume_delta`: exit 0 — task-core 1 passed、task-dispatch 9 passed、task-worker 1 passed（全 11 件・失敗 0）。
- `cargo test -p task-worker cos_chat`: exit 0 — 51 passed。
- `cargo clippy --workspace -- -D warnings`: exit 0。

**WU check 不合格の再確認と修正**（2026-10-08、retry）:
- 前 run の事後 check `sh scripts/dev/check-doc-links.sh && sh scripts/dev/progress-index.sh --check` が exit 1 で不合格（stdout: `check-doc-links: ok`、stderr: `agent-docs/progress/2026-10-08-work-unit-scope-snapshot.md:1: front matter（--- で囲んだ title, tasks, status, updated）が無い`）。
- 原因: 本 WU 由来ではない。main の bb03f5a0（WU 開始時 snapshot task）が追加した `agent-docs/progress/2026-10-08-work-unit-scope-snapshot.md` が `# title` 行を `---` fence の前に置き、`title`・`status`・`updated` 欄も無く、`progress-index.sh --check` の front matter 規則（1 行目が `---` で始まり `title, tasks, status, updated` を含む fence）に違反していた。check は全 progress file を見るため、本 WU の成果とは無関係に不合格になる。
- 修正: 当該 file の front matter を同ディレクトリの他 file と同じ fenced 形（`title`・`tasks`・`status: done`・`updated: 2026-10-08`）に整形し title 行を fence 後に移した（本文・リンクは不変、commit 02f25686）。
- 再検証: `sh scripts/dev/check-doc-links.sh && sh scripts/dev/progress-index.sh --check`: exit 0（`check-doc-links: ok`・`progress-index --check: ok`）。
- 上記の 3 crate の試験件数・exit code は再 run でも不変（`cargo test -p task-dispatch -p task-core -p task-worker cos_chat_resume_delta` exit 0、全 11 件 / `cargo test -p task-worker cos_chat` exit 0 / `cargo clippy --workspace -- -D warnings` exit 0）。

## 未解決事項
- 非 cache input（`Usage.input_tokens`）では改善が無い（claude-code は prompt が cache write に入るので構造上ほぼ一定）。判定指標の扱いは 2026-10-08 の人の決定（上の「人の決定」節）で (a) に決まった。
- 計測中、CoS は checkpoint を書かず summary は空だった（既知の不具合 4）。summary がある thread の効果は未計測。
- codex・acp・pi の live は未計測（不明）。

## 提案
- 差分配送の判定指標は非 cache input でなく cache write と prompt bytes にする（baseline の提案と同じ）。この基準なら改善あり（−13.6%・−21.1%）。（2026-10-08 の人の決定で採用。）


## main 取り込みと代替経路の併存（2026-10-08、merge-main）

- task branch へ `git merge --no-ff main` を実施。取り込み対象は `10566e5bc7666a18c0850fabb1c177aef4472483`（CoS 代替経路と Claude 利用枠の予約）。`merge-tree` と実 merge の衝突は `crates/task-dispatch/src/dispatcher/cos_chat/launch.rs` のみ。
- 衝突解消後、通常の resume は `history_for_run(..., delta_from)` で summary を省き、cursor より後を渡す。`route_index > 0` は常に別 session を作り、cursor に関係なく summary と未要約履歴の全文を渡す。main の適用済み操作 receipts の summary 追記、source・credential の切替、残り実行時間での再試行を保持した。同じ session key の代替経路でも失敗した session を再利用しない。
- cursor 更新は既存の `advance_delivery_cursor` を保持。経路再試行を enqueue している間は更新せず、completed 終端の run に記録された session 行だけを進める。resume 拒否後の fresh 再試行も、その最終 session 行を使う。
- `cos_chat_resume_delta_fallback_route_gets_full_history` を追加。summary と既配送履歴を持つ thread の resume 中に利用上限を返す fake adapter で再現し、同じ run/input のまま新 session に全文と receipts が配送され、旧 cursor は不変・完走 session の cursor だけが進むことを断言する。LLM は使用しない。
- main の経路開始通知も永続 message の seq を消費するため、既存 delta 試験の期待 seq を通知込みに更新。経路通知を欠落させず配送する動作を確認する。
- main の `routes.rs`、`cos_source_fallback.rs`、`provider_select.rs`、`config/cos.rs` と試験、`bootstrap.rs`、task-worker の `provider.rs` と試験、fallback ADR/progress、`celeris.example.toml` は main と差分ゼロ。
- 本 WU では本番 config/DB/release/systemd の操作も LLM 計測も行っていない。既存の before/after 計測は lab account provenance 未確認であり、その補完は後続 `lab-bench` WU の対象。上の旧計測結果だけでは lab 使用の証明にならない。

検証（統合後 tree）:
- `cargo test -p task-dispatch -p task-core -p task-worker cos_chat_resume_delta`: exit 0、task-dispatch 10・task-core 1・task-worker 1、計 12 件成功。追加した `cos_chat_resume_delta_fallback_route_gets_full_history` も成功。
- `cargo test -p task-dispatch cos_source_fallback`: exit 0、9 件成功。main の同試験 file は不変。

- `cargo test -p task-dispatch -p task-core cos_chat`: exit 0、task-dispatch lib 113・結合 11・task-core 28 件成功（失敗 0）。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/progress-index.sh --check`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
