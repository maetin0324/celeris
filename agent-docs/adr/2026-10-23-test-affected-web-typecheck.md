# test-affected.sh は web の型に届きうる変更で web の型検査と unit 試験を走らせる

- 日付: 2026-10-23
- 状態: 採用
- 決定者: agent（task 01M5MQ6SYDXFY1QCGN8HWENWMT の依頼に沿う）
- 関連: ADR 2026-10-20 full-tests-at-release-only（全体の試験は release gate だけ）、`scripts/dev/test-affected.sh`、
  `scripts/dev/tests/test-affected-list.sh`、`agent-docs/guides/testing.md`

## 背景

2026-10-23 の release e486d7fb が release gate の `web-pnpm-typecheck` で初めて落ちた。Rust に event kind
`required_checks_evaluated` を足した task は schema と `web/api/generated` を再生成したが、
`web/api/realtime/event-kinds.ts`・`invalidation-map.ts` に登録しなかった（`Record<EventKind, …>` の網羅で tsc が落ちる）。
task の受け入れ検査 `test-affected.sh` は「crates に差分がなく web/ だけ」のときしか web を見ず、crates の差分があると
cargo の試験だけで終わったため、WU・review を通り抜けた。

## 決定

D1. **web の検査を選ぶ条件。** 差分に次のどれかがあれば web の検査を走らせる（網羅を優先し、迷う所は含める）:
    - `web/` 配下の file
    - `docs/api/` 配下（`web/scripts/gen-types.mjs` が読む schema）
    - workspace 共通 file（Cargo.lock・ルート Cargo.toml 等。全 crate 扱いのとき）
    - `task-api` とその通常依存（dev 依存を除く。現在は task-core・task-ops・celeris-credentiald）の crate が直接変わったとき。
      schema の型と event kind はこの範囲から生まれる。逆依存として試験対象に入っただけの crate（例: dev 依存の
      task-worker の変更で task-api が対象になる）や、無関係な crate（celeris・task-dispatch・routing-eval 等）だけの変更では選ばない。
D2. **web の検査の中身は typecheck script（`tsc -b`）と unit 試験（e2e でない test script = vitest＋node --test）。**
    unit 試験も同じ条件で足す（invalidation-map.test.ts が schema と EVENT_KINDS を突き合わせ、同じ漏れを捕まえる。
    合わせて 15 秒前後）。crates の差分があっても web の検査を省かない。web を先に走らせ、落ちたら cargo の前に止まる。
    e2e・本物の browser は従来どおり release gate だけ（ADR 2026-10-20 D1）。
D3. **node_modules。** `web/node_modules/.modules.yaml` が無いか、差分に `web/pnpm-lock.yaml`・`web/package.json` が
    あるときは、先に `pnpm install --offline --frozen-lockfile`（host 共有の pnpm store から。worktree・WU で 1 秒前後）。
    store に無い版（依存を足した直後）で落ちたときだけ `--prefer-offline` で 1 回入れ直す（必要な package だけ network）。
    pnpm が PATH に無ければ `corepack pnpm@<packageManager の版>`。試験と typecheck は従来どおり pnpm を通さず
    `node_modules/.bin` を PATH に足して直に走らせ、各段（install・typecheck・unit）に watchdog
    （`TEST_AFFECTED_WEB_TIMEOUT`、既定 300 秒）を掛ける。
D4. **`--list` の出力。** 対象 crate の行の後に `web: typecheck+unit (理由: …)` を 1 行出す（理由は D1 のどれか。複数なら `; ` 区切り）。
    crate 名は `:` を含まないので区別できる。

## 試験

`scripts/dev/tests/test-affected-list.sh` の fixture（最小 cargo workspace に task-core・task-api を置く）:
(e) task-core・task-api・docs/api・web/ の変更で選ばれ、無関係な crate と task-api の dev 依存だけでは選ばれない。
(f) 偽の pnpm・tsc・vitest で、node_modules が無いと install→typecheck→unit の順、有れば install しない、
task-core だけの変更で tsc が落ちると test-affected.sh が非 0 で終わり unit・cargo へ進まない。
