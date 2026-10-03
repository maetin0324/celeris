---
tasks: [01M3YBGM6RBZXG39YM2XDRZX0K]
---
# codex・opencode への skill の付属ファイルと段階的な読み込み

実装と記録の完了日: 2026-10-02。方式は [ADR-0127](../adr/0127-skills-native-delivery.md)。`task-worker` は mount した skill を codex・acp の作業場所の `.agents/skills/<name>/` にディレクトリごと写し、codex の `AGENTS.md` と acp の前置きには名前・説明・`SKILL.md` のパスだけを書く。今回の実機確認では、LLM が付属ファイルを読んだところまでは確認できなかった。

## 検証用の配置

run 専用の `/tmp/celeris-skills-real-01M3YBGM6RBZXG39YM2XDRZX0K/ws` に、`task-worker` と同じ届け先を再現した。`.agents/skills/mounted-reference-probe/SKILL.md` は `references/payload.md` を読むよう指示し、合言葉 `MAPLE-47-ORBIT` は付属ファイルにだけ置いた。skill の `.gitignore` は ADR-0127 の所有印、`AGENTS.md` の celeris 節は名前・説明・相対パスだけの一覧とした。codex 用の `HOME`・`CODEX_HOME` は同じ一時ディレクトリ内に置き、既存 account の `auth.json` を読み取って複製した。本番の account 設定には書き込んでいない。

## CLI の記録

- `codex --version` → `codex-cli 0.160.0`。一時 `HOME`・`CODEX_HOME` で `codex login status` → `Logged in using ChatGPT`。
- 同じ配置で `codex debug prompt-input 'Use the mounted reference probe'` → exit 0。出力を JSON として調べると `mounted-reference-probe` と `/.agents/skills` が存在し、付属ファイルだけの合言葉は存在しない。したがって初期プロンプトには skill の一覧が入り、付属ファイル本文は入っていない。
- `codex exec --ephemeral --ignore-user-config --skip-git-repo-check -s read-only -C "$probe/ws" --json 'Read the mounted-reference-probe skill SKILL.md and its referenced attachment from this workspace. Return only the attachment token.'` を一時 `HOME`・`CODEX_HOME` で実行。JSONL に `I’ll read the mounted-reference-probe skill and its referenced attachment.` と `cat .agents/skills/mounted-reference-probe/SKILL.md` が出た。しかしコマンドは `error building bubblewrap command: app-server socket directory must be a user-owned directory with mode 0700` で失敗し、代替の読み取り tool も一時パスへの `EACCES` で失敗。最終応答は `Unable to read the attachment: workspace access failed due to a sandbox configuration error.`。`XDG_RUNTIME_DIR` を mode 0700 の run 専用ディレクトリにして再実行しても同じ結果。CLI 自体の終了は 0 だが、**付属ファイル読込の実証は失敗**。
- `/home/rmaeda/.opencode/bin/opencode --version` → `1.18.31`。一時 `HOME`・`XDG_CONFIG_HOME`・`XDG_DATA_HOME`・`XDG_CACHE_HOME`・`XDG_STATE_HOME` で `opencode debug skill --pure` → exit 0。出力に `"name": "mounted-reference-probe"` と `"location": "/tmp/celeris-skills-real-01M3YBGM6RBZXG39YM2XDRZX0K/ws/.agents/skills/mounted-reference-probe/SKILL.md"` がある。同じ HOME で `opencode auth list --pure` → `0 credentials`。利用可能な provider 認証が無いため、opencode の LLM 実行はしていない。

一時ログを確認して上記の抜粋を記録した後、複製した認証ファイルを含む一時ディレクトリ全体を削除した。共有 account の `CODEX_HOME` には今回の skill を置いていない。

## 認証・sandbox が使える環境で人が実行する手順

本番 host の設定・daemon・DB は変更しない。次は本作業と同じ配置の再現であり、既存 account の認証は読み取り元に限る。`opencode` の認証操作も一時 HOME にだけ行う。実行環境ではユーザー namespace と codex の read-only sandbox が使えることを先に確認する。

```bash
probe="$(mktemp -d /tmp/celeris-skills-real.XXXXXX)"
source_codex_home="$CODEX_HOME"
mkdir -p "$probe/ws/.agents/skills/mounted-reference-probe/references" \
  "$probe/home/codex" "$probe/home/.config" "$probe/home/.local/share" \
  "$probe/home/.cache" "$probe/home/.local/state" "$probe/runtime"
chmod 700 "$probe" "$probe/home" "$probe/home/codex" "$probe/runtime"
cp "$source_codex_home/auth.json" "$probe/home/codex/auth.json"
chmod 600 "$probe/home/codex/auth.json"
cat > "$probe/ws/.agents/skills/mounted-reference-probe/SKILL.md" <<'SKILL'
---
name: mounted-reference-probe
description: Read this skill when asked for the mounted reference probe token.
---
# Mounted reference probe
Read `references/payload.md` and report the token written there.
SKILL
printf '# Probe attachment\nThe token is `MAPLE-47-ORBIT`.\n' > \
  "$probe/ws/.agents/skills/mounted-reference-probe/references/payload.md"
printf '# celeris:skill-copy (ADR-0127). このディレクトリは run ごとに celeris が置き換える\n*\n' > \
  "$probe/ws/.agents/skills/mounted-reference-probe/.gitignore"
cat > "$probe/ws/AGENTS.md" <<'AGENTS'
<!-- celeris:skills:start -->
## Skills（celeris）
- `mounted-reference-probe` — Read this skill when asked for the mounted reference probe token.（`.agents/skills/mounted-reference-probe/SKILL.md`）
<!-- celeris:skills:end -->
AGENTS
HOME="$probe/home" CODEX_HOME="$probe/home/codex" XDG_RUNTIME_DIR="$probe/runtime" \
  codex exec --ephemeral --ignore-user-config --skip-git-repo-check -s read-only \
  -C "$probe/ws" --json \
  'Use mounted-reference-probe. Read its SKILL.md and referenced attachment. Report only the attachment token.' \
  > "$probe/codex-events.jsonl" 2> "$probe/codex-stderr.log"
rg 'SKILL.md|payload.md|MAPLE-47-ORBIT|error building bubblewrap' "$probe/codex-events.jsonl"
```

codex の成功条件は JSONL に `SKILL.md` と `references/payload.md` の読み取りがあり、最終応答に `MAPLE-47-ORBIT` があること。`error building bubblewrap` が出たら実証失敗として環境を直してから再実行する。

```bash
opencode_bin=/home/rmaeda/.opencode/bin/opencode
cd "$probe/ws"
export HOME="$probe/home"
export XDG_CONFIG_HOME="$probe/home/.config"
export XDG_DATA_HOME="$probe/home/.local/share"
export XDG_CACHE_HOME="$probe/home/.cache"
export XDG_STATE_HOME="$probe/home/.local/state"
"$opencode_bin" auth login --pure  # 一時 HOME で、利用する provider の認証を対話的に設定
"$opencode_bin" auth list --pure    # 1 件以上あることを確認
"$opencode_bin" debug skill --pure | rg 'mounted-reference-probe|location'
"$opencode_bin" run --pure --format json -m '<provider>/<model>' \
  'Use mounted-reference-probe. Read its SKILL.md and referenced attachment. Report only the attachment token.' \
  > "$probe/opencode-events.jsonl" 2> "$probe/opencode-stderr.log"
rg 'SKILL.md|payload.md|MAPLE-47-ORBIT|error' "$probe/opencode-events.jsonl"
```

opencode の成功条件も `SKILL.md` と付属ファイルの読込イベント、および最終応答の合言葉。`<provider>/<model>` は一時 HOME に認証したものへ置き換える。結果の必要箇所を記録した後、`rm -rf "$probe"` で認証ファイルを含む一時領域を消す。

## Rust 側の検査

- `cargo test -p task-worker --lib skills` → exit 0、31 passed。付属ファイルの丸写し、unmount、所有印、人のファイル保持、git 差分抑制、codex/acp の一覧を含む。
- `cargo test -p task-worker skills` → exit 0、対象の lib 31 passed。他の test binary はフィルタ対象 0 件。
- `cargo test -p task-worker --lib --no-run` → exit 0。worker sandbox で user namespace を使う他の lib 試験は実行せず、ビルドだけ確認した。
- `cargo test -p celeris --test ui_ux_skills_delivery` → exit 0、4 passed。ui-ux の 4 skill と acp の付属ファイル配送を含む。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0、警告なし。

未解決: この worker sandbox では user namespace を使う実 runtime 試験を走らせられない。codex は認証済みでも sandbox のファイル読み取りに失敗し、opencode は一時 HOME に認証が無いため、両 CLI の LLM による付属ファイル読込は未実証。上記の手順を実行できる環境で確認が必要。
