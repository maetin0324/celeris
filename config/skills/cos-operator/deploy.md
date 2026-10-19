# cos-operator: 登録 project のアプリの配備

人が「デプロイしてください」と依頼したとき、登録 project のアプリ（例: docker compose で動く PRA）の配備に読む。
決定は ADR 2026-10-19-cos-registered-project-deploy。

## 範囲

- 「本番 host を変えない」（Core・[production.md](production.md)）は **Celeris 自身の本番**（systemd unit・`~/.config/celeris`・
  `~/.local/celeris/releases`・`/local/celeris/state`・daemon の再起動・Celeris の SQLite と knowledge）に限る。
- 登録 project のアプリ（その compose 内の DB を含む）の配備は Celeris の本番ではない。**人が依頼し、下の表に手順がある project** は、
  その手順どおりに CoS が実行してよい。何も試さずに「決まりで変えない」と断らない。
- 表に無い project、または手順に無い操作は従来どおり: 手順書を書いて人に渡す（実行しない）。

## 確認を求める範囲（実行前に人へ一言）

- config（`.env`・secrets・compose の環境値）の変更を含む配備。
- DB の破壊的な migration（列・表の削除、型変更、データを消す文）。
- 手順の事前条件が満たせないとき（同期中が避けられない、バックアップが取れない）は止めて報告する。

## 手順の置き場所

project ごとの手順は KB の `projects/<project>/deploy.md`（`celerisctl knowledge get` で読む）が正本。この表は入口で、
手順がある project だけを載せる。新しい project は KB に手順を置いてからこの表に 1 行足す。

| project | 作業 dir | 手順 |
|---|---|---|
| PRA（pairs-reply-assistant） | `/home/rmaeda/workspace/pra-deploy` | 下の「PRA」 |

## PRA

1. 同期中でないことを確かめる（取込・同期のジョブが走っている間は配備しない。走っていれば待つか人に報告）。
2. 対象 commit を取ってきて checkout する（人が指定した ref。指定が無ければ人に聞く）:
   `git fetch /home/rmaeda/workspace/pairs-reply-assistant refs/heads/main` → `git checkout --detach <sha>`。
3. 事前に DB をバックアップする: `docker compose -f compose.yaml -f compose.prod.yaml exec -T postgres pg_dump -U pra pra | gzip`
   の出力を `/local/celeris/data/scratch/pra-db-backup-<YYYYMMDD>-pre-<sha7>.sql.gz` に置く（`/tmp` や repo 内に置かない）。
   取れなければ配備しない。
4. 変更に config（`.env` など）の変更・破壊的 migration が含まれるなら、ここで人に確認する。
5. 本番の override を必ず重ねる: `docker compose -f compose.yaml -f compose.prod.yaml up -d --build`（作業 dir で）。
   `-f compose.prod.yaml` を省いた `docker compose up` や `compose.dev.yaml` は本番で使わない。
6. 健全性を確かめる: migrate service が exit 0、全 service が healthy。満たさなければ失敗として報告し、バックアップの場所を添える
   （自動で戻さない）。
7. 報告に commit・バックアップ file・各 service の状態を書く。
