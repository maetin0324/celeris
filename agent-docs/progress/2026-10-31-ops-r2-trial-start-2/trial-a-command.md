---
title: ops-r2 試行 A の起動 command — K4 の A と同じ権限の形と @・: を含む socket path
tasks: [01M69YZVW2AA2QPYS5KKAN20SR]
status: done
started: 2026-10-31
completed: 2026-10-31
updated: 2026-10-31
---

# ops-r2 試行 A の起動 command（親 task 01M69YXTFWZT56XCZFY2PCPZMB の項目 3）

完了日: 2026-10-31

## やったこと
- ADR 付記 D-trial-a2 を `agent-docs/adr/2026-10-31-ops-r2-preflight-fixes.md` に足した（実装の前）。
- `scripts/campaign/ops-r2-trial-a-command.sh`:
  - claude の引数を K4 ops-r1 の A（K4 runbook §3.1、成果物 `01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1/runbook.md`）と同じ形にした:
    `-p --model claude-sonnet-5-5 --session-id <uuid> --output-format stream-json --verbose --permission-mode bypassPermissions --disallowedTools Task`。
  - `--session-id <小文字の UUID>` を必須にした（UUID 以外・空は exit 2）。script は生成しない。
  - path の文字種の制限（`[A-Za-z0-9._/+-]`）をやめ、改行だけを拒む。内側の command の各 path と外側の各引数を sh の単一引用符で 2 段に quote する。
    `/run/user/1001/ssh-mux-rmaeda@130.158.241.2:22` の形を受ける。
- `crates/campaign/tests/ops_r2_trial_a_command.rs`: script の写しの隣に argv を記録する偽の sandbox を置き、偽の seed script・claude で
  出した 1 行を sh で評価する。sandbox・seed・claude の argv、`CLAUDE_CONFIG_DIR`、cwd、stdin を照合する。path に空白・`'`・`@`・`:` を含める。
  実 sirius・LLM・bwrap には出ない。
- `docs/ops/ops-r2-k2-preflight.md` §6.2・§8（session id の決め方と `$P/a-session-id`、権限の形、quoting）、
  `docs/ops/ops-r2-operator-session.md` §1.1（A の session と operator の session の区別）。

commit（件名）:
- ops-r2: 試行 A の起動 command に K4 の A と同じ権限の形と session id を出し、@・: を含む path を quote して受ける

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `cargo test -p celeris-campaign --test ops_r2_trial_a_command` → exit 0、2 passed。
- `bash scripts/dev/test-affected.sh` → exit 0（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch 等。nextest 2920 tests run: 2920 passed, 9 skipped）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決事項
- `--disallowedTools Task` は K4 と同じ値にした。claude 2.1.303 では subagent の道具名が `Agent`（旧名 `Task`、ADR 2026-10-07-worker-no-subagents-no-llm-cli）なので、
  `Task` だけで subagent が止まるかは未確認（K4 でも同じ）。K4 との比較を優先した。変えるなら人の決定が要る。
- 実際の claude 2.1.303 がこれらの引数を受けるかは、worker が LLM CLI を起動しないため確かめていない。人が起動前に `claude-2.1.303 --help` で確かめる。

## 本番への移行
なし（本番 config・DB・release store・sirius は変えていない）。

## 提案
- B の worker と同じ `Agent,Task,Workflow` に揃えるかを、K2 の承認票で決める。
