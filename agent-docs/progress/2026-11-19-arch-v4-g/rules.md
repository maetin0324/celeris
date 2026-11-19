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

## TA

test-affected（`metric=test_affected`）の測り方。ta-s1・ta-mix・ta-s2・control-s1・control-s2 が使う。A の §5.5 と §7 C9 の手順に、
G で決めた準備・確認・待ち方・記録の仕方を足したもの。TSV の列・valid の規則・log の置き場は上の「## TSV」と「## log の置き場」に従う。

1. 準備（計らない）: worktree に `web/node_modules`・`control-ts/node_modules` が無ければ、各 dir で `pnpm install --offline --frozen-lockfile`
   を走らせる（test-affected.sh が中で走らせるのと同じ command）。どの回も install を含めずに測るため。A の S1 run 1 は test-affected の中の
   `web install` を含み、A の S1 run 2・3 は含まない（A の log で確かめた）。
2. `--list` の確認（計らない）: comment の probe（上の「## probe と戻し方」）を入れた状態で `bash scripts/dev/test-affected.sh --list` を走らせ、
   `git checkout -- <file>` で戻す。対象が [scope.md](scope.md) §2 の表の after と同じこと（S1: 14 crate ＋ `web: typecheck+unit` ＋
   `ts-verify: test-ts-verify`、S2: 5 crate）を確かめる。違えば測らず、違いを md に書く。
3. 各回: waitload → comment の probe → `git diff --stat -- crates`（4 file・4 行の差分であること）→ `/proc/loadavg` を記録 →
   `/usr/bin/time -v -o <raw>/ta-<S>-<回>.time bash scripts/dev/test-affected.sh > <raw>/ta-<S>-<回>.log 2>&1` → exit code を受ける →
   `git checkout -- <file>` → `git status --porcelain -- crates` が空であることを記録。続けて測るときも各回の前に waitload する
   （直前の回の負荷で 1 分 load が 4 を超えるため、2 分前後待つ）。
4. 1 回の長さは S1 で約 900 s、S2 で約 650 s で、Bash の timeout（1200 s）に 2 回は入らない。上の 1〜3 を行う driver を nohup で起動し、
   foreground の `sleep`（600 s 以下）で done の file を待つ:
   `RAW=<成果物 dir>/raw nohup bash <成果物 dir>/raw/ta-driver.sh S1 1 2 > <成果物 dir>/raw/driver-S1.out 2>&1 < /dev/null &`
   （引数はシナリオと回の番号の並び。終わると `raw/driver-S1.done` ができ、各回の `番号 exit 待った秒 load の 5 値` が `raw/ta-S1.runs` に、
   時刻つきの記録が `raw/timing-raw.log` に入る）。driver と内訳の parser の写しは ta-s1 の run の成果物
   `/local/celeris/data/workspaces/01M7MZTKBKXKEVBWS87PS5J9NK/wu/ta-s1/artifacts/raw/` の `ta-driver.sh`・`ta-summary.py`・`probe-comment.sh`。
   自分の成果物 dir の `raw/` に写して使う。測っている間は repo の中の file を変えず、未追跡の file も置かない（health と test-affected の
   変更 file の集合が変わる）。md・TSV は測り終えてから書く。
5. 値と内訳: `python3 -I <raw>/ta-summary.py <raw>/ta-<S>-<回>.log <raw>/ta-<S>-<回>.time` が次を出す。
   - `seconds`・`user_s`・`sys_s`・`exit`: `/usr/bin/time -v` の Elapsed・User time・System time・Exit status。
   - health: log の `repo-health: verdict=… exit=… findings=… new=… worsened=…` の行と `repo_health_gate: all checks passed`。
   - nextest: `Starting N tests across M binaries (… skipped)` と `Summary [ T s] N tests run: P passed (S slow), K skipped` の行。
   - doc test: `test result:` の行の数と passed・failed・ignored の和。
   - web（S1 だけ）: vitest の `Test Files`・`Tests` の行と node --test の `ℹ pass`・`ℹ fail`。`web install` の行の有無（1 の準備をしていれば無い）。
   - ts-verify（S1 だけ）: `test-ts-verify: N passed, F failed` の行。script bundle の `SKIP: 0 件`。
6. TSV の行: `test_affected <S> <after|before> <回> <seconds> <user_s> <sys_s> <exit> <開始時の 1 分 load> <valid> <git rev-parse HEAD:crates> <note>`。
   note に試行の番号・待った秒・開始時の load の 3 値・nextest の passed/skipped/秒・web・ts-verify・health の verdict を書く。
   valid は exit 0 かつ開始時の 1 分 load が 4 以下（上の「## TSV」）。比べるのは wall の中央値で、user の中央値も並べる。
7. control の leaf（before）は bc665168 の tree で同じ 1〜6 を行う。`src_tree` は `git rev-parse bc665168:crates`、file は A の §7 C8 の S1 の 3 file・S2 の 4 file。
