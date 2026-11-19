---
title: アーキ v4 G rules — after の測定の TSV の列・valid の規則・probe の戻し方・log の置き場
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# rules（G の測定の共通規則）

G の各 leaf（scope・build-check・build-test・ta-*・control-*・fix）は、この file の規則で測り、TSV に 1 回 1 行を足す。
手順と規則は A の baseline.md（A の run の成果物 `artifacts/baseline.md`）の §5・§6・§7 をそのまま使う。
シナリオの file は [scenarios.env](scenarios.env) を `. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env` で読む。

## TSV

- 形式: tab 区切り、1 行目は header、1 回の測定を 1 行に書く。leaf ごとに自分の TSV（例: `scope.tsv`・`build-check.tsv`）を置く。
- 列（この順）: `metric scenario rev run seconds user_s sys_s exit load1_start valid src_tree note`

| 列 | 値 |
|---|---|
| `metric` | `check_cold`・`check_warm`・`test_build_cold`・`test_build_warm`・`health`・`test_affected` のどれか |
| `scenario` | `S1`・`S2`、シナリオに依らない測定（cold）は `-` |
| `rev` | `after`（after の tree）か `before`（A と同じ bc665168 の tree。control の leaf だけ） |
| `run` | valid な回の番号 1・2・3。valid=no の回は `x<試行の番号>`（例 `x2`） |
| `seconds` | `/usr/bin/time -v`（または `-f '%e …'`）の経過秒 |
| `user_s`・`sys_s` | 同じ `/usr/bin/time` の user・sys の秒（子 process を含む） |
| `exit` | 測った command の exit code |
| `load1_start` | 計る直前の `/proc/loadavg` の 1 分値 |
| `valid` | `yes` か `no`（下の規則） |
| `src_tree` | 測った tree の `git rev-parse HEAD:crates`（after は scenarios.env の `AFTER_CRATES_TREE` と一致すること） |
| `note` | 試行の番号・待った秒・load の 3 値・内訳（health の fail の rule、nextest の passed 数など）。tab を入れない |

- valid の規則:
  - `load1_start` が 4 以下で、手順どおり（probe を足した状態から測り、起点が A §7 C8 のとおり）の回だけ `yes`。
  - `test_affected` は `exit` が 0 であることも要る。`health` は const の probe で fail（exit 1）になるのが手順どおりなので、exit 1 でも `yes` にできる。
  - 4 を超えた回も `valid=no` で行を残し、測り直す（A §6 の 9）。中央値は `valid=yes` の 3 回で取る。
- 待ち方（A §7 C8 の waitload）: `s=0; while awk '{exit !($1>4)}' /proc/loadavg && [ $s -lt 600 ]; do sleep 10; s=$((s+10)); done`。
  600 s 待っても 4 を超えていたら、測って `valid=no` を残すか、測らずに次の run に回す。CPU を焼く負荷はかけない。

## probe と戻し方

- const の probe（check warm・test build warm・health。A §7 C8）: 各 file の末尾に
  `printf '\n#[allow(dead_code)]\npub const SCENARIO_PROBE_%s: u64 = %s;\n' <file の番号> $(date +%s%N) >> <file>`。
- comment の probe（test-affected と `--list`。A §5.5）: 各 file の最初の `//` の行の末尾に ` probe-<毎回違う値>` を足す（行数も名前も増えない）。
- 写しの script: scope の run の成果物 `raw/probe-const.sh`・`raw/probe-comment.sh`（引数は file の並び）。
- 戻し方: 測るたびに `git checkout -- $S1_FILES` または `git checkout -- $S2_FILES` で戻す。測り終えたら
  `git status --porcelain -- crates` が空であることを確かめる。branch に source の差分を残さない。
- health は `--head worktree` を測るので、測っている間は repo の中に未追跡の file を置かない（進捗の md も測り終えてから書く）。

## log の置き場

- 生の log（`/usr/bin/time` の出力、health の json と要約、test-affected の log、load の記録）は各 run の成果物 dir
  （`$CELERIS_WU_…/artifacts/raw/`、例: `health-S1-<試行>.json`・`timing-raw.log`）に置く。repo には置かない。
- 要点（各回の値・開始時の load・exit・内訳）と中央値を leaf の md に書き、TSV を repo の `agent-docs/progress/2026-11-19-arch-v4-g/` に置く。
- `CARGO_TARGET_DIR` は Celeris が渡した値のまま使う。`/tmp` に target を置かない。
