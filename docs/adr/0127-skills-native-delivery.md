---
tasks: [01M3YBGM6RBZXG39YM2XDRZX0K]
---
# ADR-0127: mount した skill を codex・acp（opencode）にもディレクトリごと届け、段階的に読ませる

- 状態: 実装済み（LLM による付属ファイル読込の実証は環境制約で未完。2026-10-02）
- 日付: 2026-10-02
- 位置づけ: **ADR-0056（D3: skills を run に届ける）の付記**。ADR-0056 Phase 79 / Phase 81 追記の
  「codex は `AGENTS.md` に本文を埋め込む、acp は前置きに本文を埋め込む」「マーカー
  `.celeris/skills.json` は `claude-code` の `.claude/skills/` だけを掃除する」を、この ADR の D1〜D6 で置き換える。
  `claude-code` の届け方（`.claude/skills/<name>/` への丸写し）は変えない（D2・D4 の印だけ足す）。
- 関連: ADR-0059 D5 / ADR-0079 R5b-fix2（`SYNC_ALWAYS_EXCLUDED`・pull の `--delete`）、ADR-0122（ui-ux の 4 skill）

## 1. 文脈

`crates/task-worker/src/skills.rs`（ADR-0056 D3）の現状:

- `claude-code`: `<cwd>/.claude/skills/<name>/` に KB の skill ディレクトリを丸ごと写す。前回書いた名前を
  `<cwd>/.celeris/skills.json`（JSON の文字列配列）に残し、unmount された名前だけ消す。
- `codex`: `<cwd>/AGENTS.md` の `<!-- celeris:skills:start -->`〜`end` 節に、各 skill の `SKILL.md` 本文を全部埋め込む。
- `acp`: 前置き（プロンプト文面）の末尾に同じ節を足す（本文を全部埋め込む）。

このため (1) `SKILL.md` 以外の付属ファイル（`config/skills/shadcn/rules/*.md`、
`config/skills/ui-ux-quality-gate/references/`・`templates/` など）が codex・opencode に届かず、
(2) 大きな skill（`du -sb config/skills/*`: shadcn 100154、ui-ux-quality-gate 93432、web-design 104196、
frontend-design 20335 バイト）を mount すると毎 run の文脈を本文で食う。

## 2. 調べたこと（本機、外部ネットワークに出ずに）

実験はすべて `unshare -rn`（network namespace を空にして外へ出られない状態）で、一時ディレクトリを
`HOME`・`CODEX_HOME`・`XDG_*` に向けて行った（本番の `~/.codex`・`~/.config/opencode` には書いていない）。
LLM は呼んでいない（どちらも「一覧を出す」デバッグ用サブコマンドで、モデルへの要求を送らない）。

実験用の作業場所（`/tmp/skillexp.guB7/ws`、`git init` 済み）に、候補の場所ごとに別名の skill を置いた。
各 skill は `SKILL.md`（front matter に `name`・`description`）と付属ファイル `aux.md` を持つ:

```
.agents/skills/probe-agents/        .codex/skills/probe-codex/      .claude/skills/probe-claude/
.opencode/skill/probe-oc-skill/     .opencode/skills/probe-oc-skills/
.celeris/skills/probe-celeris/      $CODEX_HOME/skills/probe-codexhome/
```

### 2.1 codex-cli 0.160.0（`~/.local/bin/codex` → `~/.codex/packages/standalone/current/bin/codex`）

**結論: ネイティブに読む。場所は `<cwd から git の root までの各階層>/.agents/skills/`・`<cwd>/.codex/skills/`・
`$CODEX_HOME/skills/`。`.claude/skills/` と `.celeris/skills/` は読まない。読み方は段階的（名前・説明・
`SKILL.md` のパスの一覧だけをプロンプトに入れ、本文は必要になったときにモデルが読む）。**

- `codex --version` → `codex-cli 0.160.0`
- `codex debug --help` → `prompt-input  Render the model-visible prompt input list as JSON`
- 実験:
  ```
  $ cd /tmp/skillexp.guB7/ws
  $ HOME=$X/home CODEX_HOME=$X/home/codexhome unshare -rn codex debug prompt-input "hi" > codex-prompt.json; echo exit=$?
  exit=0
  $ grep -o 'probe-[a-z-]*' codex-prompt.json | sort | uniq -c
        3 probe-agents
        3 probe-codex
        3 probe-codexhome
  ```
  プロンプトの該当部分（抜粋）:
  ```
  <skills_instructions>
  ## Skills
  A skill is a set of local instructions to follow that is stored in a `SKILL.md` file. Below is the list of
  skills that can be used. Each entry includes a name, description, and a short path that can be expanded into
  an absolute path using the skill roots table.
  ### Skill roots
  - `r0` = `/tmp/skillexp.guB7/ws/.codex/skills`
  - `r1` = `/tmp/skillexp.guB7/home/codexhome/skills`
  - `r2` = `/tmp/skillexp.guB7/home/codexhome/skills/.system`
  - `r3` = `/tmp/skillexp.guB7/ws/.agents/skills`
  ### Available skills
  ...
  - probe-agents: probe skill probe-agents located at .agents/skills (file: r3/probe-agents/SKILL.md)
  - probe-codex: probe skill probe-codex located at .codex/skills (file: r0/probe-codex/SKILL.md)
  - probe-codexhome: probe skill probe-codexhome located at .../home/codexhome/skills (file: r1/probe-codexhome/SKILL.md)
  </skills_instructions>
  ```
  本文の印 `BODY-MARKER-…` はプロンプトに無い（`"BODY-MARKER" in prompt` → `False`）＝本文は埋め込まれない。
- 付属ファイルの扱い（バイナリの `strings` より、モデルへの指示の文面）:
  ```
  1) After deciding to use a skill, the main agent must expand the listed short `path` with the matching alias
     from `### Skill roots`, then open and read its `SKILL.md` completely before taking task actions. ...
  2) When `SKILL.md` references relative paths (e.g., `scripts/foo.py`), resolve them relative to the directory
     containing that expanded `SKILL.md` first, and only consider other paths if needed.
  3) If `SKILL.md` points to extra folders such as `references/`, use its routing instructions to identify the
     files required for the task. ...
  ```
- 探す範囲: git 管理下の `ws/sub` から起動しても `r3` = `.../ws/.agents/skills`（root まで遡る）。git 管理外の
  `/tmp/skillexp-nogit.tcQX/inner` から起動すると親の `/tmp/skillexp-nogit.tcQX/.agents/skills` を見つけた。
- `$CODEX_HOME/skills` も読むが、codex は起動時にそこへ自前の `.system/`（imagegen ほか 5 件）を書き込む
  （実験後の `ls $X/home/codexhome/skills/.system` → `imagegen openai-docs review-agent skill-creator skill-installer`）。
  CODEX_HOME は account ごとに共有なので、celeris はここに書かない（D1）。
- 設定キー・環境変数で skill の場所を足す仕組みは見つけられなかった（`strings` に `skills.config` 等は無い。
  `codex features list | grep -i skill` → `skill_search stable true`、`skip_host_skill_discovery under development false` など。
  場所を変える flag ではない）。

### 2.2 opencode 1.18.31（`~/.opencode/bin/opencode`、acp の既定コマンド `opencode acp`）

**結論: ネイティブに読む。場所は `<cwd から worktree root まで>/.claude/skills/`・`.agents/skills/`・
`.opencode/skill/`・`.opencode/skills/`、全体設定 `~/.config/opencode/skill(s)/`、`~/.claude/skills/`・`~/.agents/skills/`、
加えて設定 `skills.paths`（`OPENCODE_CONFIG_CONTENT` でも渡せる）。読み方は段階的（一覧を system prompt に置き、
`skill` tool で本文を読む）。`OPENCODE_DISABLE_EXTERNAL_SKILLS=1` だと作業場所の `.claude/`・`.agents/` も読まなくなる。**

- `opencode --version` → `1.18.31`。`opencode debug --help` → `opencode debug skill   list all available skills`
- 実験（`env -i PATH=/usr/bin:/bin HOME=$X/ochome XDG_CONFIG_HOME=… XDG_DATA_HOME=… XDG_CACHE_HOME=… XDG_STATE_HOME=… timeout 120 unshare -rn ~/.opencode/bin/opencode debug skill`、exit 0。name | location を抜粋）:
  ```
  customize-opencode | <built-in>
  probe-claude    | /tmp/skillexp.guB7/ws/.claude/skills/probe-claude/SKILL.md
  probe-agents    | /tmp/skillexp.guB7/ws/.agents/skills/probe-agents/SKILL.md
  probe-oc-skill  | /tmp/skillexp.guB7/ws/.opencode/skill/probe-oc-skill/SKILL.md
  probe-oc-skills | /tmp/skillexp.guB7/ws/.opencode/skills/probe-oc-skills/SKILL.md
  ```
  （`.codex/skills`・`.celeris/skills`・`$CODEX_HOME/skills` は出ない。`ws/sub` から起動しても同じ 4 件。）
- 環境変数・設定での変化:
  ```
  == OPENCODE_DISABLE_EXTERNAL_SKILLS=1     → customize-opencode, probe-oc-skill, probe-oc-skills
  == OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1  → customize-opencode, probe-agents, probe-oc-skill, probe-oc-skills
  == OPENCODE_CONFIG_CONTENT={"skills":{"paths":[".celeris/skills"]}}
                                            → 上の 4 件 + probe-celeris | .../ws/.celeris/skills/probe-celeris/SKILL.md
  ```
- 同梱文書（組み込み skill `customize-opencode` の本文。`opencode debug skill` の出力から抜粋）:
  ```
  | Project skills                | `.opencode/skill(s)/<name>/SKILL.md`
  | Global skills                 | `~/.config/opencode/skill(s)/<name>/SKILL.md`
  | External skills (auto-loaded) | `~/.claude/skills/<name>/SKILL.md`, `~/.agents/skills/<name>/SKILL.md`
  Register skills from non-default locations via `skills.paths` (scanned recursively for `**/SKILL.md`) ...
  - `description` is effectively required: skills without one are filtered out and never surfaced to the model.
  - `OPENCODE_DISABLE_EXTERNAL_SKILLS=1`, `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1`: skip the external skill scans under `~/.claude/` and `~/.agents/`.
  ```
  （文書は `~/` 配下だけと書くが、実験では作業場所の `.claude/`・`.agents/` もこの変数で外れる。ADR は実験の結果に従う。）
- 段階的な読み込み（バイナリの `strings` より）: system prompt に
  `"Skills provide specialized instructions and workflows for specific tasks." "Use the skill tool to load a skill when a task matches its description."`
  と一覧を置き、`skill` tool（`Load a specialized skill when the task at hand matches one of the skills listed in the system prompt.`）が
  `<skill_content name="…">` と `Base directory for this skill: …` を返す（付属ファイルは base directory からの相対で読める）。
- `opencode acp` は同じ設定・skill の読み込みを使う（acp は同じ binary の server。`debug skill` と別の経路だという根拠は無い）。
  ただし acp アダプタは opencode 専用ではない（`[adapters.acp] command` で任意の ACP エージェントを起こせる）ので、
  ネイティブ読込に頼り切らない（D3）。実機での確認は試験方針（D6）の real-run で行う。

### 2.3 git と worktree

- 写した skill ディレクトリに `*` だけの `.gitignore` を置くと、そのディレクトリは git から見えなくなり、
  codex・opencode のネイティブ読込は変わらない:
  ```
  $ printf '*\n' > .agents/skills/probe-agents/.gitignore   # .opencode/skills/probe-oc-skills, .celeris にも同じ
  $ git status --porcelain --untracked-files=all | grep -E 'probe-agents|probe-oc-skills|\.celeris'; echo $?
  1
  $ git add -A -n | grep -E 'probe-agents|probe-oc-skills|\.celeris'; echo $?
  1
  $ codex debug prompt-input hi | grep -o 'probe-agents: [^(]*(file: [^)]*)'
  probe-agents: probe skill probe-agents located at .agents/skills (file: r3/probe-agents/SKILL.md)
  $ opencode debug skill   → probe-agents | /tmp/skillexp.guB7/ws/.agents/skills/probe-agents/SKILL.md（ほか同じ）
  ```
- linked worktree の `info/exclude` は共通 git dir のものになる:
  ```
  $ git worktree add ../wt2; cd ../wt2
  $ git rev-parse --git-path info/exclude   → /tmp/skillexp.guB7/ws/.git/info/exclude
  $ git rev-parse --git-common-dir          → /tmp/skillexp.guB7/ws/.git
  ```
  celeris の WU は同じリポジトリの worktree を並列に作るので、`info/exclude` は全 worktree（人の checkout を含む）で共有される。

## 3. 決定

### D1. 届け先

| アダプタ | 届け先（丸写し） | ネイティブに読むか | 補う文面 |
|---|---|---|---|
| `claude-code` | `<cwd>/.claude/skills/<name>/`（変えない） | 読む（従来） | なし（従来どおり） |
| `codex` | `<cwd>/.agents/skills/<name>/` | 読む（2.1） | `AGENTS.md` の celeris:skills 節を一覧に（D3） |
| `acp` | `<cwd>/.agents/skills/<name>/` | opencode は読む（2.2）。他の ACP エージェントは不明 | 前置きの節を一覧に（D3） |

- `.agents/skills/` を codex と acp の共通の届け先にする。両方がネイティブに読む唯一の作業場所内の場所で（2.1・2.2）、
  特定の CLI 名（`.codex/`・`.opencode/`）に依らない。`.opencode/skills/` は `OPENCODE_DISABLE_EXTERNAL_SKILLS=1` でも
  読まれるが、opencode 以外の ACP エージェントに opencode の設定ディレクトリを作るのは避ける（その場合も D3 の一覧で届く）。
- codex は `$CODEX_HOME/skills/` に**書かない**（account 共有。codex 自身が `.system/` を書く場所でもある）。
  run 専用の CODEX_HOME も作らない（認証・規則が CODEX_HOME にあり、ADR-0095 の規則注入とも衝突する）。
- `.celeris/skills/` への写し + `skills.paths` 指定は採らない（codex は読めない。opencode には `OPENCODE_CONFIG_CONTENT` を
  人の設定と合成する必要が出る）。
- 丸写しは `claude-code` と同じ `copy_dir`（サブディレクトリを含む全ファイル）。届け先の `<name>/` だけを置き換え、
  `.agents/` の他の内容（人が置いた `.agents/skills/<別名>/` など）には触れない。

### D2. 「celeris が書いたもの」の印と掃除（marker の共有）

- **ディレクトリ内の印を正とする**: celeris が写した各 `<root>/<name>/` に、次の 2 行だけの `.gitignore` を置く
  （KB 側に `.gitignore` があっても上書きする）:
  ```
  # celeris:skill-copy (ADR-0127). このディレクトリは run ごとに celeris が置き換える
  *
  ```
  印は git から隠す役（D4）と所有の印を兼ねる。人が置いたディレクトリには無いので、人のものには触れない。
- `.celeris/skills.json` は届け先ごとの記録に広げる:
  `{"version":2,"roots":{".claude/skills":["a"],".agents/skills":["b","c"]}}`。
  旧形式（文字列の配列）は `{".claude/skills": [...]}` として読む（Phase 81 の worktree をそのまま引き継ぐ）。
  壊れていれば空として扱う（従来どおり、消しすぎない側に倒す）。
- 掃除の規則（どのアダプタの届けでも同じ関数）: 既知の届け先（`.claude/skills`・`.agents/skills`）の各 `<name>/` のうち、
  「マーカーに載っている、または印の `.gitignore` を持つ」もので、「今回のアダプタの届け先かつ今回 mount されている」に
  当たらないものを消す。つまり **作業場所に残る celeris の写しは常に今回のアダプタの分だけ**になる
  （attempt ごとにアダプタが変わっても、opencode が `.claude/skills` の古い写しを二重に拾わない）。
  マーカーは今回の届け先の分だけを書き、他の届け先の欄は消す。
- 何も記録が無く今回も空なら、何も作らない（`.agents` も `.celeris` も作らない。従来の `.claude` と同じ）。

### D3. 段階的な一覧（`AGENTS.md` の節と acp の前置き）と埋め込みの閾値

- 本文の全埋め込みは**やめる（閾値 0 バイト。どの大きさの skill も埋め込まない）**。両 CLI とも一覧→必要時に本文、の
  仕組みを持ち（2.1・2.2）、閾値を設けると「小さい skill は付属ファイル無しで本文だけ」という第 2 の経路が残るため。
- codex の `AGENTS.md`（`<!-- celeris:skills:start -->`〜`end` の節、区切りの外は従来どおり触れない）と acp の前置きの末尾に、
  同じ関数が作る次の形を置く:
  ```
  ## Skills（celeris）
  次の skill を作業場所に置いてある。仕事が skill の説明に当てはまるときだけ、その SKILL.md を最後まで読み、
  SKILL.md が参照する付属ファイル（SKILL.md のあるディレクトリからの相対パス）も必要に応じて読むこと。
  当てはまらない skill は読まなくてよい。
  - `<name>` — <description>（`.agents/skills/<name>/SKILL.md`）
  ```
  - パスは cwd からの相対（コンテナ実行・リモートの写しでも同じ相対で読める）。
  - 説明は `SkillMount.description`、空なら `SKILL.md` の front matter の `description`、それも無ければ省く。
    1 件 1024 文字で切る（Agent Skills 形式の description の上限。一覧が大きくなりすぎないため）。
- codex で一覧が `<skills_instructions>`（ネイティブ）と二重になるのは許す（1 件 1 行。ネイティブ読込が無効化されたときの
  保険で、受け入れ条件でもある）。
- `skills` が空の run では、`AGENTS.md` に以前の celeris:skills 節があれば取り除く（従来は何もしなかったので、unmount 後も
  古い本文が残った）。節が無ければ `AGENTS.md` に触れない（ファイルが無いのに作らない）。

### D4. git の差分・commit に混ざらない方法

- D2 の印の `.gitignore`（`*`）を各写しに置く。`.celeris/` にも `*` だけの `.gitignore` を置く（このリポジトリは
  `.gitignore` に `.celeris/` があるが、他の対象リポジトリには無い）。どちらも 2.3 の実験で `git status`・`git add -A` から
  隠れることを確かめた。worktree ごと・ディレクトリごとに閉じており、写しを消せば印も消える。
- `.git/info/exclude` は使わない。linked worktree では共通 git dir の 1 ファイル（2.3）で、並列の WU・人の checkout と共有され、
  同時書き込みの競合と、worktree を消した後の残骸が出るため。
- 既に追跡されているファイル（リポジトリが `.agents/skills/<同名>/` を commit している場合）は `.gitignore` では隠れない。
  その場合の置き換えは差分として見える（従来の `.claude/skills` と同じ扱い。人が同名を commit した場合だけ起きる）。
- codex の `AGENTS.md` の節は従来どおり作業場所の `AGENTS.md` に書く（追跡ファイルなら差分に出るのは従来と同じ。
  一覧化で節は数百バイトに縮む）。

### D5. ssh 同期（`SYNC_ALWAYS_EXCLUDED`）との整合

- `SYNC_ALWAYS_EXCLUDED`（`crates/task-worker/src/ssh.rs`: `.taskd/`・`.celeris/`・`runs/`・`inputs/`）は**変えない**。
  `.celeris/skills.json` は手元だけの帳簿のまま、`.agents/skills/` と `.claude/skills/` は同期される（従来の `.claude/skills` と同じ）。
- 問題: push は既定で `--delete` 無し、pull は `--delete` 付き。unmount で手元から消した写しがクラスタに残り、次の pull で
  手元に戻ると、マーカー（手元の `.celeris/`）には載っていないので旧方式では消せない。
  → D2 で印（ディレクトリ内の `.gitignore`）を所有の正にしたので、戻ってきた写しも印で見分けて次の届けで消せる。
  印は同期でディレクトリと一緒に運ばれ、クラスタ側の git からも隠れる。
- `.agents/` を `SYNC_ALWAYS_EXCLUDED` に足すことは採らない（人がリポジトリに置いた `.agents/skills` まで同期から外れるため）。

### D6. 試験と実機確認

- 単体（`cargo test -p task-worker skills`、一時ディレクトリだけ、外部ネットワーク・LLM なし）:
  - codex・acp それぞれで、付属ファイル（`rules/x.md`・`references/y.md` 等、入れ子を含む）を持つ skill を mount すると
    `.agents/skills/<name>/` に全ファイルが届き、印の `.gitignore` があること。
  - `AGENTS.md` の節・前置きが一覧の形で、`SKILL.md` の本文（印の文字列）を含まないこと。区切りの外の内容は保たれること。
  - unmount（`skills` 空）で写しと `AGENTS.md` の節が消え、マーカーが空になること。
  - 人が置いた `.agents/skills/<別名>/`・`.claude/skills/<別名>/`・`.agents/` の他のファイルに触れないこと。
  - 印だけがあってマーカーに無い写し（pull で戻った写し）が消えること。旧形式のマーカー（配列）を読めること。
    アダプタが替わったとき（claude-code → codex、codex → claude-code）に他方の写しが消えること。
  - `git init` した一時リポジトリと、その linked worktree で、届けた後の `git status --porcelain` が空であること
    （`git` コマンドは手元の binary。ネットワーク不要）。
- 結合（ADR-0122 の ui-ux の 4 skill、`config/skills/{frontend-design,shadcn,ui-ux-quality-gate,web-design}`）:
  codex・acp の届けで 4 件の全ファイルが写ること（例: `shadcn/rules/*.md`、`ui-ux-quality-gate/references/`・`templates/`）、
  一覧の節が 4 行で、本文を埋め込んでいた従来より十分小さい（各 skill の `SKILL.md` の大きさの和より小さい）こと。
- CLI のネイティブ読込の確認（LLM なし・本機の binary に依存するので `cargo test` には入れず、real-run の手順として記録）:
  届けた作業場所で `unshare -rn codex debug prompt-input hi` の `### Skill roots` に `<cwd>/.agents/skills` と 4 件が出ること、
  `unshare -rn opencode debug skill` に 4 件の location が出ること。どちらも `HOME`・`CODEX_HOME`・`XDG_*` を一時ディレクトリに向ける。
- LLM を呼ぶ実機確認（real-run。ADR-0009 P-34、使える環境で codex・opencode 各 1 回）: 付属ファイルにだけ書いた合言葉を
  持つ skill を mount し、合言葉を答えさせて付属ファイルまで読まれたことを確かめる。codex の CODEX_HOME は run 専用の一時
  ディレクトリにし（人の回答 2026-10-02）、本番 account の設定に書き込まない（認証ファイルは読むだけ）。証跡と
  環境が使える場所での再実行手順は `docs/progress/phase-skills-progressive.md`、要約は `docs/PROGRESS.md` に記録する。

## 4. 採らなかった案

- codex の `$CODEX_HOME/skills/` に写す: account 共有で他の run・人の対話 session にも見える。
- `.git/info/exclude`: D4 のとおり worktree 間で共有される。
- 小さい skill だけ本文を埋め込む閾値（例 4KB）: 付属ファイルを読まない経路が残り、届け方が 2 通りになる。
- `.celeris/skills/` に写して `skills.paths` / `AGENTS.md` の一覧で指す: codex はネイティブに読めず、opencode には設定の合成が要る。

## 5. 影響

- 変更は `crates/task-worker/src/skills.rs`（共通: 丸写し・印・マーカー v2・掃除・一覧）、`codex.rs`・`acp.rs`（呼び出しの差し替え）、
  `skills/tests.rs` と結合試験。`protocol.rs` の `SkillMount` の形は変えない（schema 生成物は動かない見込み）。
- `claude-code` の挙動は印の `.gitignore` が増えることと、他の届け先の写しを掃除することだけが変わる。
