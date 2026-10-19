# CoS は人が依頼した登録 project のアプリの配備を、project の手順に従って実行できる

- 日付: 2026-10-19
- 状態: 実装済み
- tasks: [01M5A015666V05DRE6CRKY0NCB]

## 背景
CoS は 2026-10-18 に PRA を docker compose で 12 回以上配備していたが、2026-10-19 の run（thread 01M59KWTC9PYEQA6ZW1Q20NJVW）は
「デプロイしてください」に何も試さず「本番の機械（docker compose・本番 DB）は CoS が変えない決まり」と断った。指示は変わっておらず、
WORK_RULES の「本番 host を変えない」と production.md が Celeris 自身の本番を守る規則なのに、登録 project のアプリにも当たると読めたため、run ごとに判断が揺れた。
同じ run で、CoS run の `$TMPDIR` が空で `$TMPDIR/cp.json` が `/cp.json` になる不具合も見つかった。

## 決定
- D1: 「本番 host を変えない」は Celeris 自身の本番（systemctl --user・systemd-run・`~/.config/{systemd,celeris}`・`~/.local/celeris/releases`・
  `/local/celeris/state`・daemon の再起動・Celeris の SQLite と knowledge への直接書き込み）に限る。WORK_RULES（固定 Core）と production.md に明記する。
- D2: 登録 project のアプリの配備は、人が依頼し、その project の手順があるとき、CoS が手順どおりに実行してよい。手順の無い project は従来どおり手順書を書いて人に渡す。
- D3: 手順の置き場所は KB の `projects/<project>/deploy.md` を正本とし、cos-operator skill の `deploy.md` に手順のある project の表（入口）を持つ。
  最初の例として PRA（`/home/rmaeda/workspace/pra-deploy`）: 同期中を避ける → checkout → `pg_dump` を `/local/celeris/data/scratch` へ →
  `docker compose up -d --build` → migrate service exit 0・全 service healthy を確認。project の設定欄は増やさない（config 欄を増やすと migration と画面が要る割に、手順は文章だから）。
- D4: config（`.env` など）の変更を含む配備と DB の破壊的 migration は、実行前に人へ一言確認する。事前条件を満たせないときは止めて報告する。自動の巻き戻しはしない。
- D5: CoS chat run には run 固有の一時 dir（`runs/<run_id>/tmp`、`run_tmpdir::RunTmpDir`）を作り、`TMPDIR`・`TMP`・`TEMP` に向ける。run task の終了で消える。
  作れなければ警告だけで従来の環境のまま走る（build-tmp-hygiene D2 と同じ）。

## 検証
- task-worker: CoS の固定 Core に D1 の範囲と配備の記述がある試験、skill（SKILL.md・deploy.md・production.md）に範囲の記述がある試験。
- task-dispatch: `cos_run_tmp_env` が `TMPDIR` 等を run dir に向ける試験。

## 本番への移行
KB の `skills/cos-operator/` に `deploy.md` と更新した `SKILL.md`・`production.md` を置く（人が行う。docs/ops/cos-chat.md）。
KB の `projects/pra/deploy.md` に PRA の手順（deploy.md の「PRA」節）を置く。

## 付記（2026-10-19 人のコメント: TMPDIR の範囲）
- D5 を CoS run だけでなく、reviewer の process と決定的検査（command check）まで広げた。daemon の環境に `TMPDIR` が無いため、
  `os.environ["TMPDIR"]` を読む検査が KeyError になっていた。
  - command check: `LocalWorkspace::exec` が exec ごとの専用 dir（`runs/exec-tmp-*`）を `TMPDIR`・`TMP`・`TEMP` に向け、終了で消す
    （`with_env` が `TMPDIR` を渡していればそれを使う）。
  - reviewer: `review::with_run_tmpdir` が `runs/<review_run_id>/tmp` を作って adapter の env に重ね、run の終了で消す。
  - 試験: `exec_gives_the_command_a_private_tmpdir_removed_afterwards`、`reviewer_run_gets_a_run_tmpdir_that_is_removed_on_drop`、`cos_run_env_sets_tmpdir_to_the_run_tmp_dir`。
- 固定 Core に D1 の記述を足したため、Core の上限試験を 7000 → 7500 バイトにした（実測 7439）。
- 範囲外（未対応）: ssh remote workspace の exec は remote 側の環境に従う。
