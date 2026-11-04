---
tasks: [01M6HW7NFHTFSG78DHWQNHBTHS]
---
# T1-cap-background（TS の経路）の試行の記録

- 結果: trial は 1 回で ok（trial.jsonl は 1 行、ok=true、first_failed_stage=null、production_touched=false）。
- 試した回数: 1 回。落ちた段: 無し。直したこと: 無し。
- 変更（candidate.patch、+43/-1）: `control-ts/src/domains/cos_digest.ts` に `capBackground`（300 code point を超えたら先頭 300 code point ＋ `…`）を足し、
  `digestMessage` の `- 背景: ` の行だけに当てた。`cos_digest.test.ts` に試験 3 件（300・301 字、BMP 外の文字で code point と UTF-16 の差、
  cards と他の行が変わらないこと）を足した。
- patch を作る前の手元の確認: `pnpm exec tsc --noEmit` exit 0、`pnpm exec vitest run src/domains/cos_digest.test.ts` 55 passed。
- trial の段（記録の値）: scope exit 0（0.022 秒）、eval exit 0（6.952 秒。旧 fixture 2066 件の差分 0、期待差分 16 件で explained 9・equal 7・wrong 0、
  cargo 0 回）、pilot exit 0（86.898 秒。ts-verify 7.899 秒、edit→activate 8.158 秒、rollback 1.019 秒、canary の再起動の復元 0.316 秒、
  cargo 0 回）。全体 93.929 秒。
- 採点の file（spec.json・spec.cases.jsonl・inputs.jsonl）は読まなかった。読んだのは task.md と `cos_digest.ts`・`cos_digest.test.ts`・trial.sh の冒頭（使い方と検査）。
- 本番の host・DB・mode・release・guard には触れていない（trial.sh の一時 DB の daemon だけ）。subagent・別の LLM の CLI・API は起動していない。
