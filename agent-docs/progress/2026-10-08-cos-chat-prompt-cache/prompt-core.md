---
title: CoS chat 最適化 5 — 固定 Global Core と可変部の分離
tasks: [01M4D3XKE2PZKTNSKHJGBQMEK0]
status: done
updated: 2026-10-08
completed: 2026-10-08
---
# 固定 Core と可変部の分離（H1 / T4）

設計・根拠は [ADR 2026-10-08-cos-chat-prompt-cache 付記 D6](../../adr/2026-10-08-cos-chat-prompt-cache.md)。

## 完了したこと
- branch を main `8cdd96fb` へ fast-forward した（T2 の決定的ベンチ `cos_chat/bench_tests.rs` を含めるため）。
- `cos_chat::build_parts` が Core（`api_base_url` と credential の環境変数名だけが parameter。5,919 B）と可変部（id 行・skill 名・「run 固有の操作パラメータ」・入力・要約・未要約範囲・受信箱の件・添付の一覧）を返す。
- claude-code: Core を `HEADLESS_RUN_NOTE` に続けて 1 つの `--append-system-prompt` 引数に載せ、stdin は可変部だけ。codex: `-c developer_instructions=<TOML 文字列>`（`exec`・`exec resume` で同じ bytes）。acp・pi: 入力の先頭に Core → skill 一覧 → 可変部。
- prompt.txt は claude-code・codex で Core と stdin の 2 区画を残す。
- worker 一般の prompt は変えていない（`preamble::render` は同じ bytes。既存の claude_code・codex・acp・pi・preamble の試験は無変更で通る）。

## 判定（決定的ベンチ、`cargo test -p task-worker cos_chat_bench_`）
before = main `8cdd96fb`（この変更の前）、after = この branch。生の JSON は run の artifacts `bench-before.json`・`bench-after.json`。

| 指標 | before | after |
|---|---|---|
| 固定部 bytes | 4,226 B（run の値で分断） | Core 5,919 B（10 thread・10 turn で byte 一致） |
| 新規 thread 10 件の先頭一致（claude-code: system + stdin） | 1,262 B | 7,182 B |
| 同上（入力 1 本に連結。acp/pi） | 43 B | 5,962 B |
| 同 thread turn 間の先頭一致（claude-code / 連結） | 1,303 B / 84 B | 7,477 B / 6,257 B |
| claude-code の stdin（turn 1 / turn 10） | 4,783 B / 11,097 B | 883 B / 7,117 B |
| 連結した入力の総 bytes | 4,747 B | 6,766 B |

改善あり（固定部全体が prefix に入った）と判定して統合する。連結した総 bytes は 2.0 KB 増えた（pin・relay の規則を常に Core に入れるため）。claude-code では Core は system 側にあり、resume の turn ごとの会話には積まれない。

## 証拠
- `cargo test -p task-worker cos_chat` → exit 0、`test result: ok. 49 passed`（新規 `cos_chat_core_` 6 件を含む）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-parallel.sh` → exit 100、passed 4772 / failed 64 / ignored 14。失敗 64 件はすべて browser・browser_launcher・credentiald・CDP relay・browser_e2e・scratch reflink の試験（worker sandbox の userns・socket path の既知の制約。base の記録でも 64 件）。cos_chat・claude_code・codex・acp・pi・preamble の試験は全部通った。
- `bash scripts/dev/check-doc-links.sh` → ok、`sh scripts/dev/check-adr-numbers.sh` → ok。

## 未解決事項
- live（隔離 daemon・claude_oauth）の cache read / write の変化は測っていない（**不明**）。T8 で `scripts/dev/cos-chat-bench.sh` の S1/S2 を流して比べる。
- codex の `exec resume` で developer message が再挿入されて積まれるかどうかは live 未確認（**不明**）。
- pi の system 経路（CLI flag）はこの host に pi が無く確かめていない。

## 提案
- T8 の live ベンチでは、新規 thread の 2 件目以降の cache write が baseline（21.9k）より減るかを見る。
- Core の作業の規則は `preamble::fixed_notes()` の短縮版。片方を変えたら両方を直す（Core 側の試験で 6,000 B 上限を固定している）。
