---
title: 統括 CoS の定期見直し・人に回す件の背景・決定回答後の計画再判定
tasks: [01M66BSZJECRFA8KBT8T3048FY]
status: running
updated: 2026-10-30
---

# 統括 CoS の定期見直し・人に回す件の背景・決定回答後の計画再判定

## 実装（ADR D1〜D4）
- D1: 定期見直しの入力（`crates/task-dispatch/src/dispatcher/cos_chat/review.rs` の `review_text`）に「いま開いている人待ち」（`cos_triage_open_items`）と「前回から閉じた人待ち（報告しない）」（新設 `cos_triage_closed_since`）を DB から決定的に入れる。各行は種類・source key（task id を含む）・状態・開いた時刻・題名。
- D2: `config/skills/cos-operator/SKILL.md` の定期見直しの節に、報告に書く人待ちは入力か報告直前の API で開いていると確かめたものだけ、session の記憶で書かない、done・回答済み・クラスタ job 待ちを人待ちと書かない、閉じた件と報告済みの件を「増えた」と書かない、の規則。
- D3: escalation packet に任意欄 `background`（どの依頼・task から、なぜ今）と `on_answer`（答えると何が起きるか）。daemon の件ごとの発言（`digest.rs`）に「- 背景:」「- 答えると:」、Discord 本文（`crates/celeris/src/notify/triage.rs`）に「背景:」「答えると:」。`config/skills/cos-inbox-triage/SKILL.md` に欄の書き方と task 内部の語へ言い換えを添える規則。CoS への指示文（`task-worker/src/cos_chat.rs`、`cos_chat/fallback.rs`）も同じ書式に揃えた。API schema（`docs/api/v1/api-v1.schema.json`）を再生成。
- D4: 人が task の決定にすべて答えると、未回答の決定を理由に人へ回した計画の承認（`plan_gate-<task>-v<n>`）を案件 CoS の一次対応に 1 回だけ再び渡す（`crates/task-core/src/chat/plan_rejudge.rs`、dispatcher pass `CosChatLaunch::plan_rejudge`）。入力の 2 行目は `決定の回答による計画の再判断: [...]`。判定は skill §3 (f) のまま。決定は人に残す規則・人へ回す範囲は変えていない。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-dispatch review_text_lists` — 1 passed（回答済み決定は閉じた側だけ、開いた側は 2 件、同じ入力で同じ文面）
- `cargo nextest run -p task-core cos_triage_closed_since` 系と `plan_rejudge` — plan_rejudge 2 passed（固定時刻: 決定 open では候補なし→回答→候補 1→claim→answered で閉じる／判定前の回答では渡さない）
- `cargo nextest run -p task-dispatch answered_decision_rejudges` — 1 passed（決定 open では run なし→回答→再判断 run 1 件→再 tick で増えない）
- `cargo nextest run -p task-dispatch cos_chat_triage_digest` — 7 passed（「- 背景:」「- 答えると:」を確認）
- `UPDATE_SCHEMA=1 cargo nextest run -p task-api committed_schema_matches_generated` — 1 passed（schema に 14 行追加）
- 受け入れの検査（test-affected / clippy / fmt）の結果は下の「検査」節。

12:00Z の 3 つの誤りとの対応: (i) 回答済みの決定は「前回から閉じた人待ち（報告しない）」にだけ出る（`review_text_lists_open_waits_apart_from_closed_ones`）。(ii) BenchFS（waiting_for_cluster_jobs）と (iii) done の K4 は受信箱の項目を持たないので「いま開いている人待ち」に出ない（同試験で受信箱の項目だけが開いた側に並ぶことを確認）。skill D2 で記憶からの補完を禁じた。

## 検査
（下に追記）

## 未検証（実 LLM）
- CoS が `background`・`on_answer` を実際に書くか、言い換えが人に数十秒で読める分量になるか、再判断 run で §3 (f) を満たす計画を approve するかは、実 LLM で確かめていない。daemon 側の入力・整形・配線だけを試験した。
- Discord 本文の「背景:」「答えると:」は単体試験が無い（既存の notify 試験 252 件が通ることだけ確認）。

## 本番への移行
- policy_version は上げない（10 のまま）。理由: 追加は任意欄と新しい入力種別で、既存の判定基準（人に回す範囲）を変えていない。版を上げると稼働中の CoS session が全て fresh になり、未回答の op が 422 になる。
- skills の import（operator が実施）: 配備後に `config/skills/cos-inbox-triage/` と `config/skills/cos-operator/` を本番 KB の skill に差し替える（KB `projects/agent-platform/cos-triage-policy-skill-version.md` の「KB skill 差し替え」手順）。確認: 次の受信箱一次対応 run の作業場所の SKILL.md に「background」の節があること。
- config の変更は無い。

## 提案
- `background`・`on_answer` を必須欄にするかは、実 LLM で数日運用して空欄率を見てから決める（必須にするなら policy_version を上げる）。
