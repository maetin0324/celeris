# T2-cap-item-title（TS の経路）の記録

- trial の回数: 1 回（trial.jsonl は 1 行、ok=true、first_failed_stage=null）。落ちた段は無く、直し直しも無い。
- 変えた file（candidate.patch）: `control-ts/src/domains/cos_digest.ts`（`title` が `capTitle` を通す。`Array.from` で code point を数え、80 を超えたら先頭 80 + `…`）と `control-ts/src/domains/cos_digest.test.ts`（境界の試験 5 件）。追加 51 行・削除 2 行。
- trial 前の手元の確認: `pnpm install --offline --frozen-lockfile`、`tsc --noEmit` exit 0、`vitest run src/domains/cos_digest.test.ts` 57 passed。
- trial の段の結果: scope exit 0（0.022 秒）、eval exit 0（7.246 秒。旧 fixture 2066 件の差分 0、期待差分 12 件で explained_ok 8・equal_ok 4・wrong 0、cargo 0 回）、pilot exit 0（86.892 秒。ts-verify 7.645 秒、rollback 1.02 秒、canary の再起動の復元 0.304 秒）。合計 94.216 秒。
- 採点の file（spec.json・spec.cases.jsonl・inputs.jsonl）と oracle の dir は読んでいない。
- source は commit していない（patch を作った後に `git checkout -- control-ts/src` で戻した）。本番・LLM の CLI・subagent は使っていない。
