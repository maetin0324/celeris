---
tasks: [01M4VAAKFJHRSVKZ3996NFKSKS]
title: eval-run manifest-tmp — 一時 DB で manifest 生成・recheck の手順を dry-run（本番 schema 77 待ち）
status: done
completed: 2026-10-13
---
# manifest-tmp: 一時 DB での manifest 手順の dry-run

**本番 schema 77 待ち。** 本番 DB は schema 70 で、runner（`celeris-routing-eval`）は 77 を要る。人の決定（eval-deploy = main-first）により、77 は AQ が main に入った後の通常 release で本番に入る。それまで本番の manifest 作成と評価 run は止める（unit manifest-prod が担う）。この unit は本番 DB・本番 config・systemd・releases に触れていない（`--db` に本番 path を渡さず、写しも作っていない）。

中間物（一時 DB・manifest・合成スクリプト）は repo の外の `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/tmp-dryrun/` にある。コードは変えていない。

## 実行したこと（一時 DB）

以下 `E=/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval`、`BIN=$CARGO_TARGET_DIR/debug/celeris-routing-eval`。

1. `cargo build -p routing-eval` — exit 0、約 50 秒（cold target）。
2. `python3 -I $E/tmp-dryrun/make_fixture.py <worktree> $E/tmp-dryrun/fixture.sqlite3` — worktree の migration 0001〜0077 を（RESERVED_VERSIONS 38・39・40・68・69 を飛ばして）空 DB に当て、`schema_migrations` に記録し、合成の task・work_unit を 44 行入れる（coding 36 件 = kind implement/test/fix 各 12、非 coding 8 件 = research/docs。形は `crates/routing-eval/src/manifest.rs` の試験 fixture と同じ。created_at は 2026-09 と 2026-10-05 以降を交互）。出力 `schema=77 tasks=44 work_units=44`、約 9 秒。
3. `$BIN manifest --db $E/tmp-dryrun/fixture.sqlite3 --out $E/tmp-dryrun/manifest` — exit 0、0.07 秒。`problems=30 sha256=3698e574…`。同じ command の再実行で同じ sha（凍結の安定性）。
4. manifest の中身の確認（python）: 30 問全部に空でない `prompt`、`prompt` に採点 command の文字列が入らない（`[check command redacted]` に伏せられる）、stratum は `coding:implement`・`coding:test`・`coding:fix` 各 10、split は dev 14・held-out 16（cutoff 2026-10-05）、`subset_12` は 12 問、`features` あり。
5. recheck（写しに対して）: `$BIN recheck --manifest $E/tmp-dryrun/recheck-copy/manifest.json --repo <worktree の絶対 path> --out $E/tmp-dryrun/recheck-copy/recheck.json --worktree-root $E/tmp-dryrun/recheck-wt` — exit 0、0.1 秒。合成行の base/head は実在 commit ではないので `adopted=0 excluded=30 commit_not_in_repo=30`。**合成問題では check の再実行（base で失敗・head で成功）に意味が無いので、本当の判定はしていない**。確かめたのは CLI の引数・出力の形・manifest への `recheck` 鍵の書き戻しだけ。

## 出力の形

- `<out>/manifest.json`（約 80 KB / 30 問）: `schema`（`celeris.routing-eval-manifest/1`）・`version`・`cutoff`・`seed`・`sha256`・`problems[]`。各問は `id`・`task_id`・`work_unit_id`・`created_at`・`genre`・`kind`・`command`・`prompt`・`split`・`stratum`・`subset_12`・`head_commit`・`base_commit`・`features`・`features_sha256`・`sha256`。
- recheck は manifest に `recheck` 鍵を足して書き戻す（問題集合と sha256 は変えない）。`--out` を付けると同じ記録を別 file にも書く。標準出力は 1 行の要約と除外の問題ごとの行。
- held-out の問題文・check command が入るので、manifest と recheck の出力は repo・標準出力の記録・進捗に写さない。

## 本番で使う command の並び（schema 77 到達後、unit manifest-prod）

```sh
DB=/local/celeris/data/db/celeris.sqlite3
E=/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval
BIN=$CARGO_TARGET_DIR/debug/celeris-routing-eval
# 0. 本番 schema を読み取り専用で確かめる（77 以上でなければ止める）
sqlite3 "file:$DB?mode=ro" "SELECT MAX(version) FROM schema_migrations;"
# 1. 候補の数（coding で done の task に属する done の WU のうち check を持つもの。30 未満なら manifest は失敗する）
sqlite3 "file:$DB?mode=ro" "SELECT COUNT(*) FROM work_units w JOIN tasks t ON t.id=w.task_id WHERE w.status='done' AND t.status='done' AND COALESCE(json_extract(t.json,'\$.genre'),'coding')='coding' AND json_array_length(json_extract(w.json,'\$.checks'))>0;"
# 2. build と凍結（manifest は内部で mode=ro の URI で開く）
cargo build -p routing-eval
$BIN manifest --db "$DB" --out "$E/manifest" --cutoff 2026-10-05 --seed 0 --version 1
# 3. 凍結した manifest を保全してから recheck（recheck は manifest に recheck 鍵を書き戻す）
cp "$E/manifest/manifest.json" "$E/manifest/manifest.frozen.json"
out=$($BIN recheck --manifest "$E/manifest/manifest.json" --repo /home/rmaeda/workspace/agent-platform --out "$E/manifest/recheck.json" --worktree-root "$TMPDIR/recheck-wt" 2>&1); echo "exit=$?"
```

所要時間の目安: manifest は一時 DB で 0.07 秒。本番 DB は行数が多いが、走査は done の WU 全件と選んだ 30 問の features 抽出だけなので秒の桁と見込む（未計測）。recheck は 30 問 × base/head の 2 回 check を実行し、各回の上限は `--timeout` 900 秒。check が cargo test 等なら build を含めて 1 問あたり数分、全体で数十分〜1 時間超になりうる。`--limit` で先に数問だけ流して所要を測るのがよい。

## 気付いた問題（コードは変えていない）

1. **`recheck` は終了時に `--worktree-root` の dir を丸ごと消す**（`Worktrees` の Drop が `remove_dir_all(root)`）。最初の試行で manifest の写しと同じ dir を `--worktree-root` に渡し、写しごと消えた。本番では manifest の dir と別の、消えてよい dir（`$TMPDIR/recheck-wt` など）を渡す。直すなら「自分で作った dir だけ消す」にする提案。
2. **`recheck` の標準出力を `| head` 等で途中で閉じると exit 101（println の broken pipe で panic）**。出力は変数か file に全部取る。
3. **manifest は genre=coding だけを採る**。一時 DB の research/docs 8 行は黙って除かれた。task の受け入れ条件「genre ごとに区間・n・費用」は、今の runner では coding の 1 genre（kind ごとの stratum）にしかならない。他の genre を評価するなら runner の変更が要る（collect/closeout の前に人が判断）。
4. **check は WU の `checks` の最初の `cmd` 1 本だけ**を採点に使う。複数 check の WU は残りが採点されない。
5. **同じ `--version` で再生成すると、DB が増えて sha が変わり拒否される**（`choose a new --version`）。本番 DB は動き続けるので、凍結は 1 回で済ませ、作り直すときは版を上げる。
6. 最初の recheck の失敗（`No such file or directory`）は問題 1 による: 書き戻し前に manifest の dir が消えていた。別 root で再実行して通った。
7. 一時 DB の base/head は合成なので、recheck の判定（noop・head で失敗）の挙動は本番まで確かめられない。

## 証拠

- `cargo build -p routing-eval`: exit 0。
- `python3 -I make_fixture.py`: exit 0、`schema=77 tasks=44 work_units=44`。
- `celeris-routing-eval manifest …fixture.sqlite3 …`: exit 0、`problems=30`。再実行で同じ sha256。
- manifest 検査（python）: `30 True False`（問題数・全問 prompt あり・prompt に command なし）。
- `celeris-routing-eval recheck …`（写し・別 root）: exit 0、`adopted=0 excluded=30 commit_not_in_repo=30`。
- 本番 DB: この unit の command に `/local/celeris/data/db/celeris.sqlite3` を渡していない（上の本番手順は文書として書いただけで、実行していない）。

## 未解決

- 本番 schema 77 の到達（AQ の main 取り込みと通常 release）。到達後に manifest-prod が上の並びを流す。
- 問題 1〜4 の扱い（runner を直すか、運用で避けるか）。

## 提案

- `recheck` の Drop は `--worktree-root` を渡されたときは `wt/` と `log/` だけを消す。
- manifest の genre 絞り込みを外すか、受け入れ条件の「genre ごと」を coding の kind ごとと読み替えるかを、collect の前に人が決める。
