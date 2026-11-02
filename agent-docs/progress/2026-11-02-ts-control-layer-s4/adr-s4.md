---
title: S4 adr-s4 — 境界 ADR に CoS digest 経路（cos_digest）の付記
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 adr-s4 — 境界 ADR に CoS digest 経路の付記

完了日: 2026-11-02（unit adr-s4）。コードは変えていない。

## 結論
[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) に「付記 2026-11-02: S4 の CoS digest 経路」（S4-1〜S4-9）を足した。
読んだ根拠: S0 の計画 §2 S4・inventory §4、S2 の付記と進捗、`cos_chat/digest.rs`、`chat/triage.rs`、`chat/store.rs`。

## 項目と節の対応
| objective の項目 | 節 | 要点 |
|---|---|---|
| (1) 移す経路 | S4-1 | digest pass 全体（`triage_digest`→`digest_one`）。S0 は文面のみだったが requeue の判断も TS の command にする。B・C・D を後に回す理由 |
| (2) 領域と DTO | S4-2 | `cos_digest`、`CosDigestInput`、`post_digest`・`requeue_run`、fixture `{name,input,commands}` |
| (3) 検証規則 | S4-3 | key・thread/run_id・card id・requeue の state・各 command 高々 1 件。code `key_mismatch` ほか 5 種 |
| (4) compare と apply | S4-4 | Rust の command を常に apply、TS を待たない |
| (5) 保存規則 | S4-5 | 本文は残さない。表 `script_compare_log`（events は task_id NOT NULL）に `input_digest`・`run_id`・`thread_id`。replay は store から組み直し、drift は別に数える |
| (6) bridge しない理由 | S4-6 | snapshot は Rust が作る。op・capability は空。S2-7 の context 分離は不要のまま |
| (7) 行為の追跡 | S4-7 | 新 event 不要。chat message の冪等 key、`cos_inbox_items`、`script_compare_log.applied` |
| (8) 試験の表 | S4-8 | 順序・再配送・retry・cancel・再起動・policy_version・generation 切替ほか 10 行 |
| 追加 | S4-9 | Rust の 4 分割（rust-split unit） |

## 後続の unit への注意
- hook unit: migration（`script_compare_log`、`script_domain_modes` の `cos_digest` 行）。`Event` の variant は足さない。
- rust-split unit: 検証関数は参照実装にも試験で当てる。
- 未解決: 20 件の pass で in-flight 上限を超えた run の記録は落ちる（S4-4）。S5 で分母として足りるかを見る。
- 提案: S5 の `ts` mode で TS の command を apply した記録欄を決める（S4-7）。

## 証拠（提出時の HEAD で実行。結果は run の報告に書く）
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ control-ts/ web/`
- `grep -c '^### S4-[1-9] ' agent-docs/adr/2026-11-02-ts-control-layer-boundary.md`（9 を期待）
- `bash scripts/dev/check-doc-links.sh`・`bash scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`
