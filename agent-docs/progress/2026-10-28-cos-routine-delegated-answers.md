---
title: 承認済みの枠の内側の定型の判断を CoS が根拠つきで代答する
tasks: [01M60AMT8P6FQ5QX64ANRZ588S]
status: done
updated: 2026-10-28
---
# 承認済みの枠の内側の定型の判断を CoS が根拠つきで代答する

完了日: 2026-10-28

ADR: [2026-10-28-cos-routine-delegated-answers](../adr/2026-10-28-cos-routine-delegated-answers.md)
（[2026-10-06-cos-inbox-triage](../adr/2026-10-06-cos-inbox-triage.md)・[2026-10-27-relayed-instructions](../adr/2026-10-27-relayed-instructions.md)・
[2026-10-05-cos-chat-home](../adr/2026-10-05-cos-chat-home.md) に付記）

## 変更

- `crates/task-api/src/cos/basis.rs`（新規）: `DelegatedBasis`（category・refs・frame・failures）と決定的な検査。
  (a) `within_approved_frame`: 人の承認（standing rule・人が答えた decision・認めた approval）の実在、`used + requested <= limit`、usage/ledger の出典。
  (b) `raise_once`: kind=limit の `raise-once` だけ、commit/artifact の出典、前と今の失敗が違う、同じ task で CoS の `raise-once` は 1 回。
  (c) `documented_default`: 推奨 option と一致、adr/doc の path。(d) `work_guidance`: 自由質問に note で答える（`replan` 不可）。
- `POST /cos/inbox/{i}/resolve`: outcome=answer は `basis` 必須。observe・escalate に `basis` は 422 `validation`。
- `POST /cos/operations`: 開いている待ちに答える操作（decision・approval・phase/plan gate・question・inbox answer）で
  `instructed_by` が無ければ `basis` 必須。閉じた待ちは従来どおり domain の 409 等。
- 通った代答は監査の reason が「定型の代答（<category>）: …」、payload に `basis`。
- `[cos.triage]`: `policy_version` の既定 "5"。`human_required` の分類を `objective_change`・`beyond_frame`・`external_publish`・
  `destructive`・`secret_or_security`・`standing_permission`・`explicit_human` に改め、未知・旧名は読み込みエラー。
- `config/skills/cos-inbox-triage`（version 5）: 表の分類名、§3 の定型の代答 4 種類と basis、§5 の本文例。
- 文書: `docs/api/v1/api.md` §3.130・取り次ぎ節、`docs/ops/cos-chat.md`、`config/celeris.example.toml`、
  `docs/ops/mlsys-main-4-ops-r1.md` §8 方針の版の変更記録（"4" → "5"）。API schema と web の生成型を再生成。
- 試験: `crates/task-api/tests/cos_routine_answers.rs`（新規 5 件。fake harness = CoS run の credential で API を呼ぶ試験、LLM なし）、
  `crates/celeris/src/config/cos_tests.rs` の分類名の試験。既存の CoS 試験は代答に basis を付けた
  （phase gate の代答は推奨の無い途中確認なので、人の入力を `instructed_by` にする形に変えた）。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `cargo nextest run -p task-api --test cos_routine_answers`: 5 passed。
- `bash scripts/dev/test-affected.sh`: exit 0（Rust 4702 passed・20 skipped、web の unit 689 passed、文書検査・source-size gate 通過）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `python3 scripts/dev/check-human-text.py`: 0 findings。`sh scripts/dev/progress-index.sh --check`: ok。

## 本番への移行

適用は外部 operator が利用者に告知してから行う。順序は 1 → 2 → 3（新しい release を昇格する前に 1・2 を済ませる）。

1. 本番 config（`~/.config/celeris/celeris.toml`）の `[cos.triage]` を確かめる。
   - `human_required` を書いていれば、旧名を新名へ置き換える（`fundamental_change` → `objective_change`、`resource_overrun` → `beyond_frame`、
     `security` → `secret_or_security`、`standing_permission` を足す）。書いていなければ既定が使われるので変えなくてよい。
     旧名のままだと新しい binary は config の読み込みで止まる（verify も落ちる）。
   - `policy_version` を書いていれば `"5"` にする（skill の `metadata.version` と同じ値）。書いていなければ既定 "5"。
   - 確認: `grep -n -A6 '^\[cos.triage\]' ~/.config/celeris/celeris.toml`。
2. 本番 KB の skill を更新する: `celerisctl skills import config/skills --name cos-inbox-triage --root <kb_root>`
   （リポジトリはこの release の checkout）。確認: `<kb_root>/skills/cos-inbox-triage/SKILL.md` の `version: "5"`。
3. release を昇格し、`GET /api/v1/health` の `cos_policy_version` が "5" であることを確かめる。
   評価の試行は版 "5" から新しい `condition_id` で始める（`docs/ops/mlsys-main-4-ops-r1.md` §8）。

## 未解決事項

- `[cos.triage] human_required` と `min_confidence` は CoS の prompt にも API にも渡っていない（skill の表と同じ語彙を持つだけ）。
  分類名の誤りは読み込みエラーになるが、値を減らしても CoS の振る舞いは変わらない。
- refs の中身（commit が実在するか、ADR の本文が既定を書いているか）は API では確かめない。監査の payload に残るので、
  人の取り消し（override）と評価で見る。

## 提案

- `[cos.triage]` の `human_required`・`min_confidence` を CoS run の prompt（受信箱の run の前置き）に渡し、config と skill の二重管理をやめる。
- `within_approved_frame` の `used` を API が usage・ledger から自分で読む（今は CoS の申告を枠と突き合わせるだけ）。
