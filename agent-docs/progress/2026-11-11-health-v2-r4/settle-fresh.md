---
title: "健全性v2 R4 settle-fresh: continuation-resume の Resume か Fresh の照合を stored_session_fresh_reason の 1 つにする"
tasks: [01M7B2KBNJ72C5F75RAEWSFECG]
status: blocked
updated: 2026-11-13
---
# 健全性v2 R4 settle-fresh

## 結論

ADR 2026-11-11-dispatcher-state-ownership D4 の契約（入力は照合 key の一致・前回の拒否・id の妥当性・context の枯渇・測った token と上限）の
`sessions::stored_session_fresh_reason` は、今の `sessions::decide_continuation`（task・WU）と
`sessions::cos_chat::decide_cos_chat_session`（CoS chat）の両方から等価に呼べない。Objective の指示どおり実装せず、replan を申告した
（yield plan_issue）。repo のコードは変えていない。

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

- `phase 1: settle-fresh: continuation-resume を等価に共有できない理由を進捗に書く` — この進捗ファイルだけ。コードと ADR は変えていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `sh "$CELERIS_WU_SCOPE_PATHS"` → この進捗ファイルだけ
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）、`sh scripts/dev/check-doc-links.sh` → exit 0
- コードを変えていないので test-affected・clippy・fmt はこの unit では走らせていない

## 未解決

- D4 の continuation-resume の行と `stored_session_fresh_reason` の契約を、等価に共有できる形に改める決定（ADR の変更）が要る。
  この unit は ADR を変えられない（範囲の check が拒む）。

## 提案

推奨は案 A。

- 案 A（推奨）: 共有の範囲を「保存 session の id の妥当性・context の枯渇・測った token と上限」に絞る。key の照合と前回の拒否は各関数に残す。
  - 順は両方で id → context の枯渇 → token の上限になり、等価に共有できる（task・WU は context の枯渇と token の上限を同じ `ContextRollover` に写すので、
    両者の順は結果を変えない。CoS chat の cache 消失は呼び出し元の判定のまま id より前に見る。どちらも `CacheMissing` なので順は結果を変えない）。
  - 関数名・module・2 か所から呼ぶ・写しを消す、という task の受け入れ条件 1 はそのまま満たせる。
  - ADR D4 の契約の入力から「照合 key の一致」と「前回の拒否」を外す付記が要る（docs の unit で先に ADR を直し、その後この unit をやり直す）。
- 案 B: 順を 1 つにそろえ、どちらかの振る舞いを変える（CoS chat を拒否優先にする、または task・WU を key 優先にする）。既存の試験の assertion を変える
  ことになり、この unit の受け入れ条件 1 に反する。人の決定が要る。
- 案 C: continuation-resume を D4 の共有から外す（「共有しない」の列に移す）。判断はすでに経路ごとに 1 つずつで、id の妥当性は
  `session_id_is_valid_for_adapter` で既に共有している。task の受け入れ条件 1 の D4 の行の数が変わる。
