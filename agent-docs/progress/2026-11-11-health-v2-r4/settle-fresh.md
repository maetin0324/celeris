---
title: "健全性v2 R4 settle-fresh: continuation-resume の Resume か Fresh の照合を stored_session_fresh_reason の 1 つにする"
tasks: [01M7B2KBNJ72C5F75RAEWSFECG]
status: done
updated: 2026-11-13
completed: 2026-11-13
---
# 健全性v2 R4 settle-fresh

完了日: 2026-11-13

## 結論

人の決定 fresh-contract の答え a に従い、共有の範囲を「保存 session の id の妥当性・context の枯渇・測った token と上限」に絞った。
絞り込みと理由は [ADR 2026-11-13-continuation-resume-shared-inputs](../../adr/2026-11-13-continuation-resume-shared-inputs.md) に書き、
実装より先に commit した。ADR 2026-11-11-dispatcher-state-ownership の本文は変えていない。

- `sessions::stored_session_fresh_reason`（`crates/task-dispatch/src/sessions.rs`）を 1 つだけ定義した。入力は `StoredSessionFacts`
  （adapter・id・context の枯渇・測った token・上限）、返す値は `Option<StoredSessionFreshReason>`（`InvalidId`・`ContextExhausted`・
  `TokenRollover`。`None` なら Resume）。順は id → context → token。
- `decide_continuation` は `InvalidId` を `SessionMissing`、`ContextExhausted` と `TokenRollover` を `ContextRollover` に写す。
- `decide_cos_chat_session` は cache の消失を今のまま先に見て `CacheMissing`、その後に共有の照合を呼び、`InvalidId` → `CacheMissing`、
  `ContextExhausted` → `ContextExhausted`、`TokenRollover` → `TokenRollover` に写す。
- 2 つの関数の中にあった id の検査と token・context の比較の写しは消した。照合 key・前回の拒否・cache の有無・cwd、
  token の測り方（`approx_tokens` と `rollover_measure`）、理由ごとの session の作成と引退、`resolve_continuation_session` は変えていない。

以下の「確かめたこと」は 1・2 回目の run の分析（照合の順の違い）で、ADR 2026-11-13 の背景の根拠として残す。

## 確かめたこと（adr-align の未解決）

今の 2 つの判断の順序（`crates/task-dispatch/src/sessions.rs`、`crates/task-dispatch/src/sessions/cos_chat.rs`）:

- task・WU: 役割 → adapter・container → 別 WU → WU の最初の run → **前回の拒否** → continuation でない → 設定の fresh 指定 →
  保存 session が無い → **key（adapter → account・provider → cwd）** → **id** → **token の上限か context の枯渇** → Resume
- CoS chat: 保存 session が無い → **key（方針だけ → それ以外）** → **前回の拒否** → cache 消失か **id** → **context の枯渇** → **token の上限** → Resume

等価に共有できない理由は 2 つある。

1. 前回の拒否と key の不一致の順が逆である。
   - CoS chat は key を先に見る。試験 `cos_chat_run_session_reason_precedence_is_fixed` は key の変化と拒否が重なると `key_changed` を求める。
   - task・WU は拒否を先に見る。拒否で選ばれる `ResumeRejected` だけが `continuation_session.rs` で直近の checkpoint の文脈
     （`latest_checkpoint_context`）を前置きに入れる。key を先にすると、拒否と adapter の変化が重なった run は `AdapterChanged` になり、
     checkpoint の文脈を失う（session の解決が変わる）。
   - 順を引数で切り替えると共通の関数に呼び出し元ごとの分岐を足すことになる（規約 3・5）。
2. task・WU では、拒否の判定と key・id・token の判定の間に、呼び出し元固有の判定（continuation でない・設定の fresh 指定・保存 session が無い）がある。
   1 回の呼び出しではどちらかの位置に寄せるしかなく、どちらでも結果が変わる。
   - 拒否を後ろへ寄せる: 試験 `session_resume_session_reuse_rejected_resume_falls_back_to_checkpoint`（直前が `Failed` で拒否 → `ResumeRejected`、
     保存 session が無くて拒否 → `ResumeRejected`）が落ちる。
   - key・id・token を前へ寄せる: 設定の fresh 指定（`continuation_resume = false`）と adapter の変化が重なると、`FreshRequested`
     （session を作らない）が `AdapterChanged`（session を作る）に変わる。壊れた id と重なると `SessionMissing`（session を作る）に変わる。
     既存の試験はこの組み合わせを持たないので通るが、理由ごとの session の作成が入力によって変わる（等価でない）。
3. cwd の違い（task・WU で `SurfaceUnsupported`、session を作らない）は、key の不一致の種類を呼び出し元が自分の enum に写せば保てる。
   これ単独は共有の妨げではない。

## 変更

- `phase 1: settle-fresh: continuation-resume を等価に共有できない理由を進捗に書く` — 1 回目の run。この進捗ファイルだけ。
- `phase 1: settle-fresh: 再提出の経緯（result.json の summary が yield より先に読まれた）を進捗に書く` — 2 回目の run。この進捗ファイルだけ。
- `phase 1: settle-fresh: continuation-resume で共有する照合を id・context・token に絞る ADR を足す` — 3 回目の run。ADR だけを先に commit した。
- `phase 1: settle-fresh: stored_session_fresh_reason を sessions に 1 つだけ定義し、decide_continuation と decide_cos_chat_session から呼ぶ` —
  `sessions.rs`・`sessions/cos_chat.rs`・試験 2 file・`docs/architecture-map.md`（sessions と CoS chat の行に共有の関数と ADR 2026-11-13 を足した）・この進捗。

足した試験（守る不変条件と既存の試験で検出できない理由は各試験の doc comment に書いた）:

- `sessions/tests.rs` の `stored_session_fresh_reason_checks_id_then_context_then_tokens` — 共有の関数の単体試験。理由が重なると
  id → context → token の順で 1 つ選び、token は上限と等しければ上限に達したと数える。
- `sessions/tests.rs` の `session_resume_broken_id_wins_over_context_exhaustion` — task・WU で壊れた id と context の枯渇が重なると `session_missing`。
- `sessions/cos_chat/tests.rs` の `cos_chat_run_bad_id_wins_over_context_and_tokens` — CoS chat で cache があっても id が不正なら、
  context の枯渇・token の上限と重なっても `cache_missing`。

既存の試験の assertion は変えていない（`session_resume_session_reuse_rejected_resume_falls_back_to_checkpoint`・
`cos_chat_run_session_reason_precedence_is_fixed` を含む）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

コードの commit の後に実行した（その後の commit はこの進捗ファイルだけ）。

- `bash scripts/dev/test-affected.sh`（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）→ exit 0
  （nextest 3878 passed・11 skipped、script bundle SKIP 0 件、doc test は失敗 0）
- `cargo nextest run -p task-dispatch -E 'test(/session/) | test(/cos_chat/) | test(/continuation/)'` → exit 0（296 passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict=pass、new=0・worsened=0）
- `sh scripts/dev/progress-index.sh --check` → exit 0、`sh scripts/dev/check-doc-links.sh` → exit 0、`sh scripts/dev/check-adr-numbers.sh` → exit 0
- `grep -rn "fn stored_session_fresh_reason" crates/task-dispatch/src` → `sessions.rs` の 1 件。呼ぶのは `sessions.rs` の `decide_continuation` と
  `sessions/cos_chat.rs` の `decide_cos_chat_session` の 2 か所
- `git diff "$CELERIS_WU_BASE" -- agent-docs/adr/2026-11-11-dispatcher-state-ownership.md` → 差分無し
- `sh "$CELERIS_WU_SCOPE_PATHS"` → ADR 2026-11-13・この進捗・`sessions.rs`・`sessions/cos_chat.rs`・試験 2 file・`docs/architecture-map.md` の 7 件

## 再提出の経緯（1・2 回目の run）

- 1 回目の run の `result.json` は `summary` と `yield`（`plan_issue`）を両方持っていた。worker は `summary` を `yield` より先に読む
  （`crates/task-worker/src/result_report.rs` の `interpret_result_text`: question > wait > summary > yield）ので、done として扱われ、
  WU の check（`stored_session_fresh_reason` の定義の数）が落ちて retry になった。
- 2 回目の run は上の分析を試験で確かめ直した（`sessions/tests.rs` の `session_resume_session_reuse_rejected_resume_falls_back_to_checkpoint`
  は保存 session が無くても `ResumeRejected` を求め、`cos_chat/tests.rs` の `cos_chat_run_session_reason_precedence_is_fixed` は
  key の変化と拒否が重なると `key_changed` を求める）。結論は変わらない。`result.json` は `yield` だけにして replan を申告した。

## 未解決

- D4 の表と実装の差: ADR 2026-11-11 D4 の continuation-resume の行は「拒否」を共有すると書くが、実装は拒否と照合 key を共有しない
  （ADR 2026-11-13 D4 が正本の付記）。
- 拒否と照合 key を見る順を task・WU と CoS chat で 1 つにそろえるか（前回の選択肢 c に当たる。どちらかの振る舞いと試験の assertion を変える）は、
  後で人が決める問いとして残す。
- node 単位の session の `sessions::decide`（ADR-0054 D1、`dispatcher/run_context.rs` が呼ぶ）にも id の検査と token の比較があるが、
  D4 の表に無く順も id → account → token なので、この unit では変えていない（ADR 2026-11-13 D3）。

## 提案

（1・2 回目の run の提案。人は案 A を選び、この run で実装した。）

- 案 A（推奨）: 共有の範囲を「保存 session の id の妥当性・context の枯渇・測った token と上限」に絞る。key の照合と前回の拒否は各関数に残す。
  - 順は両方で id → context の枯渇 → token の上限になり、等価に共有できる（task・WU は context の枯渇と token の上限を同じ `ContextRollover` に写すので、
    両者の順は結果を変えない。CoS chat の cache 消失は呼び出し元の判定のまま id より前に見る。どちらも `CacheMissing` なので順は結果を変えない）。
  - 関数名・module・2 か所から呼ぶ・写しを消す、という task の受け入れ条件 1 はそのまま満たせる。
  - ADR D4 の契約の入力から「照合 key の一致」と「前回の拒否」を外す付記が要る（docs の unit で先に ADR を直し、その後この unit をやり直す）。
- 案 B: 順を 1 つにそろえ、どちらかの振る舞いを変える（CoS chat を拒否優先にする、または task・WU を key 優先にする）。既存の試験の assertion を変える
  ことになり、この unit の受け入れ条件 1 に反する。人の決定が要る。
- 案 C: continuation-resume を D4 の共有から外す（「共有しない」の列に移す）。判断はすでに経路ごとに 1 つずつで、id の妥当性は
  `session_id_is_valid_for_adapter` で既に共有している。task の受け入れ条件 1 の D4 の行の数が変わる。
