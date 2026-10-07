---
tasks: [01M49X6CW5KE5HY0EZRQR0S045]
---
# coding の既定ハーネス（Claude 系は Claude Code、非 Claude 系は Pi + Hashline）の導入・確認手順

設計は [ADR 2026-10-07](../../agent-docs/adr/2026-10-07-coding-harness-default-pi-hashline.md)。設定の全体例は
`config/celeris.pi-hashline.example.toml`。以下の host 操作は**人が実行する**手順で、agent は実行しない。
`adapter_policy = "model_family"` を書くまで挙動は今のまま（有効化スイッチを兼ねる）。

## 0. 前提と未確認点

- 2026-10-07 時点で本番 host の PATH に `pi` は無く、**Hashline はどこにも無い**。Hashline の配布元・path・tool 名は人が決める
  （repo 側では未確認。ADR「未確認点」1）。
- `pi` は openclaw の依存に `@earendil-works/pi-coding-agent` 0.75.4 が同梱されており、
  `node /usr/lib/node_modules/openclaw/node_modules/@earendil-works/pi-coding-agent/dist/cli.js` で動く。
  別に入れる場合は npm（既存の node 運用）: 下記 1。
- Pi は現状 **opencode-go pool**（鍵を `OPENCODE_API_KEY` で渡す）と **openai_compatible / llm-proxy 行**で使える。
  codex / Claude の OAuth は Pi に渡せない（config 読み込みで拒否）ので、それらの行は今の adapter のまま fallback で使われる。

## 1. Pi の導入（どちらか一方）

```sh
# A. openclaw 同梱版を使う（導入不要）
node /usr/lib/node_modules/openclaw/node_modules/@earendil-works/pi-coding-agent/dist/cli.js --version   # 0.75.4

# B. 別に入れる（root 不要の prefix）
npm install -g --prefix ~/.local/celeris/npm @earendil-works/pi-coding-agent
~/.local/celeris/npm/bin/pi --version
```

確認: `--version` が版数を出し exit 0。`--help` で `--mode`・`--tools`・`--no-extensions`・`-e` があること。

## 2. Hashline の導入

Hashline は Pi の extension（hash 付き行参照で read/edit する）。Celeris は自動発見を止め（`--no-extensions`）、
config に書いた path だけを `-e` で読む。

```sh
mkdir -p ~/.local/celeris/pi-extensions
# <Hashline の配布物> を ~/.local/celeris/pi-extensions/hashline/ に置く（取得元は人が決める）
ls ~/.local/celeris/pi-extensions/hashline/
```

確認: path が存在すること（無いと Celeris は run を始めずに設定誤りで失敗する）。Hashline の read/edit の
**tool 名**を extension の README か `pi -e <path> --help` 等で確かめ、次節の `tools` に書く。
tool 集合は read/edit（Hashline）+ bash/grep/find/ls 程度に絞る。subagent・planner を名前に含む tool は config 検証が拒否する。

## 3. config の追記

`~/.config/celeris/config.toml`（または providers.d）へ `config/celeris.pi-hashline.example.toml` の次を写す。
先に `cp -a ~/.config/celeris/config.toml ~/.config/celeris/config.toml.bak-pi-YYYYMMDD`。

- `[[providers]]` の `adapter = "pi"` 行: `model`（`provider/id`）、`extensions`（Hashline path）、`tools`（allowlist）、
  必要なら `command`/`args`（B で入れた場合は `command = "~/.local/celeris/npm/bin/pi"`、A なら
  `command = "node"` と `args = ["<cli.js の path>"]`）。
- `[[harnesses]] id = "coding"` に `adapter_policy = "model_family"`。

反映後の確認（人が実行）:

extensions/tools の未指定・OAuth pool 指定は daemon 起動時の config 読み込みで設定エラーになる（起動しない）。
そのため本番へ入れる前に、同じ config を使う verify（`docs/ops/selfdeploy.md`）で読めることを確かめる。通常の release 手順（`docs/ops/selfdeploy.md`）で daemon を入れ替える。daemon の再起動は人が行う。

## 4. 他のハーネスへ戻す・明示する

- **既定ごと戻す**: `[[harnesses]] coding` の `adapter_policy` 行を消す（または `"provider_order"`）。設定の再読み込みだけで、データの移行は無い。
- **Pi 行だけ止める**: `[[providers]]` の pi 行を消す。非 Claude の task は fallback で今の codex / acp 行に倒れる。
- **task ごとに明示する**: task の `adapter`（作成 API・PATCH・role・assignee の役割）に `claude-code` / `codex` / `acp` / `pi` を指定する。
  明示 adapter は既定解決より常に優先され、この仕組みは触らない。
- Claude 系 model の判定は provider 名の文字列ではなく、LLM source・account pool・catalog の `family`
  （`[[model_routing.models]] family = …`）で決まる。proxy（`celeris` source）経由の行は family 不明＝非 Claude 扱いなので、
  Claude を proxy 経由で使うなら catalog に family を書くか adapter を明示する。

## 5. (model, harness) 別の metrics の見方

- run 単位: `GET /api/v1/tasks/{id}/routing`（[gui-api](../api/v1/gui-api.md)）の run ごとの監査に `adapter`（`pi` / `claude-code` …）と
  `model` が別欄で出る。`harness` 欄は genre（`coding`）なので比較には `adapter` 欄を使う。
- 選ばれ方: 監査の `adapter_choice`（explicit / preferred / fallback と理由 / provider_order）で、既定解決で選ばれたか
  fallback に倒れたかが分かる。fallback が多ければ Pi 行の枠・cooldown を疑う。
- 比較: `celerisctl routing export --db <backup の写し>` → `celerisctl routing evaluate`（`docs/ops/model-routing-migration.md` §9。読み取り専用）。
  success・token・cache-hit・cost は run の usage（Pi は `message_end` の `usage` を合算）から (model, adapter) で集計する。
- 注意: proxy 経由 run の `model` は lane 名（`celeris/<tier>`）で実 model ではないので別群に分ける。task 単位の
  `GET /metrics/execution` は adapter で group しない（ADR「metrics 整合」）。

## 6. 確認の流れ

1. §1〜3 を実行し verify で config が読め。
2. 非 Claude の cheap/standard task を 1 件流し、routing 監査の `adapter = pi`、`adapter_choice = preferred` を確かめる。
3. Claude 系 task が `claude-code` のままであること。
4. 問題があれば §4 で戻す。
