---
title: ops-r1 の採点〜集計の経路の形（pipe-design）
tasks: [01M655T6CGPRQ2HSQPYQFXYGZX]
status: done
updated: 2026-10-30
---
# ops-r1 の採点〜集計の経路の形（pipe-design）

完了日: 2026-10-30

## 結果
- verdict: feasible。daemon（crates/campaign 以外）の変更は要らない。設計は `docs/ops/ops-r1-pipeline.md`、判断は ADR `2026-10-30-ops-r1-pipeline-manifest`。
- condition の表し方: ConfigId A・B を流用し、manifest に省略時は直列化しない `conditions`（condition_id・intake `direct_claude_code`|`cos_thread`・trial_ids）を足す。
  `conditions` のある manifest は run・preflight など task API 投入の道具が拒否する。
- human-op: `--condition <condition_id>`（`--arm` と排他、`--manifest` 必須）、experiment_set `ops-r1`、任意欄 `condition_id`。
- 入口: `ops-r1-assemble` が A の raw（`ops-r1-a-raw/1`）と B の intake・trace・attribution から runner と同じ out dir と decision-log 形の records.jsonl を作る。
  後段（grade-out・check・export・human-op report・final-accounting・check-out-consistency.py）は既存の subcommand のまま。
- 各段への写像表（試行数・分母・終了理由・wall・消費・合否）は設計書 §4。
- preflight.json の gaps 5 件・proposals 3 件を読んだ。`stage-grader-inputs は manifest 必須` は誤り（main.rs の分岐で manifest は任意）。
- 調べて分かったこと: checker の (A, B) 組は B の observed run の model を全部 Sonnet と照合し、model 欠測も違反にする。attribution は model を持たないが、
  DB の写しの `runs.model`・`runs.adapter` にある（preflight の DB で worker run の model claude-sonnet-5-5 を確認）。attribution に欄を足す（/2）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `git diff --stat "$CELERIS_WU_BASE" -- crates/` → 出力なし（crates/ に差分なし）
- `grep -n '^verdict:' docs/ops/ops-r1-pipeline.md` → `verdict: feasible`
- コードは書いていないので cargo の試験・clippy・fmt は走らせていない（Rust の変更が無い）。

## 未解決事項
- 設計書 §6 の未確認 3 点（set 宣言の照合が routing の無い B を通すか、自由文の reason を checker が受けるか、check-out-consistency.py が executor を見るか）は pipe-assemble が試験で確かめる。
- 計画の replan で pipe-entry は pipe-raw（A の raw・attribution/2）と pipe-assemble（入口・雛形・既存段の通し）に分かれた。設計書 §7 と ADR の葉の名前をこれに合わせた。
- B の workspace_sha256_initial と成果物 dir の取り方は runbook（固定票の開始前条件 2）で決める。

## 提案
- 無し。
