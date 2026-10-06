---
title: shared CDP relay の idle event pump（F1）・F4・確認台本 F2/F3 の統合検証
tasks: [01M48DGY90GJAW2T8C95267HV3]
status: done
updated: 2026-10-06
completed: 2026-10-06
---

# shared CDP relay の idle event pump（F1）・F4・確認台本 F2/F3 の統合検証

段 fix の統合 HEAD `85f59d11`（`integrate wu/session-home: 確認台本の衝突を解消…`）で検査だけを行い記録する。実装の修正は行わない。4 回目の実機確認（`884c7baf`、Result: FAIL）と原因調査（`4cc2b535`）に基づく F1・F4・F2/F3 の修正が、各葉の作業 commit（`e3201862`、`dfe9636a`、`3d544c3a`）と統合 commit（`dab4e9a7`、`8f4ea60b`、`85f59d11`）で tree に入っていることを前提に検証した。

## 各 unit 記録

- [cdp-pump（F1）](2026-10-06-browser-cdp-fix/cdp-pump.md) — shared CDP relay の idle event pump。設計は [ADR](../adr/2026-10-06-shared-cdp-idle-event-pump.md)。
- [session-home（F4）](2026-10-06-browser-cdp-fix/session-home.md) — bwrap の `HOME=/session/home` と launcher の session dir cleanup。
- [check-script（F2/F3）](2026-10-06-browser-cdp-fix/check-script.md) — 確認台本の順序（agent 操作と egress 拒否記録の収集を pause/takeover/release/decision より前に）と `ip_literal` 拒否理由の受け入れ。

衝突解決: 統合 `85f59d11` で check-script 側（pause 前の拒否収集・ip_literal）を採り、session-home 側の後段の重複した収集（private_address 要求）は捨てた。session-home の R7（launcher の `Started` 応答の session id を daemon.log から拾う）は台本の session dir 探索に組み込んだ。

## 検証結果（統合 HEAD 85f59d11）

- `cargo test --workspace --no-run` — exit 0。
- `cargo test -p task-worker` — exit 0、888 passed、0 failed、10 ignored（userns 試験は opt-in 外のため skip/ignore）。
- `cargo nextest run -p task-worker --test browser_shared_cdp_events` — exit 0、1 test run: 1 passed（0 件実行でなく、idle pump の決定的試験が通る）。
- `cargo clippy --workspace -- -D warnings` — exit 0。
- 文書検査 3 本 — `sh scripts/dev/progress-index.sh --check` exit 0、`sh scripts/dev/check-adr-numbers.sh` exit 0（148 files）、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` exit 0。
- `bash -n scripts/dev/browser-web-live-check.sh` — exit 0。`git diff --check 4cc2b535..HEAD` — clean。

## 未解決事項

- 実機 5 回目の確認は Fable が行う（本 run は検査のみ）。sandbox 内では launcher と Chrome の実機確認ができないため、F1/F4 の効きは試験（scripted browser）と compile/clippy でしか確認していない。
- F1/F4 の修正が本番に効くには、host の `/usr/local/libexec/celeris/celeris-browser-launcher`（と egress binary）を人が入れ替える必要がある。root 作業であり、この検証では daemon・launcher の本番差し替え・再起動は行っていない。人の手順: 新 release の binary を host の `/usr/local/libexec/celeris/` に配置して `systemctl --user restart celeris@<instance>`（該当 daemon）を行い、launcher の version/protocol 応答で新 binary が効いていることを確認する。
