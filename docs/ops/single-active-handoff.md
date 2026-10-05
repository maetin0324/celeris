---
tasks: [01M44Y0BR7HQRFNRWK2T2P2ZXE, 01M452R2MYRS45WCF15R73VQ0J]
---

# active が 1 つだけであることを確かめる手順

live 昇格の後に、旧と新の daemon が同時に `active` になっていないことを読み取りだけで確かめる手順。
仕組みは [ADR-0040 の 2026-10-05 付記（D4）](../../agent-docs/adr/0040-self-improvement-deploy.md)。

## 昇格の後

1. `promote.sh` が `handoff done: the new celeris is active and the old one is draining` を出したことを log で見る。
   出ていれば、その時点で active は 1 つ（新）。
2. daemon の一覧を読む（書き込みはしない。`SD_PROD_API` は `config.toml` の API の基底 URL。`GET /api/v1/releases` は
   `/api/v1/health` 以外の API と同じく Bearer が要る）。

   ```sh
   curl -s -H "Authorization: Bearer $(cat ~/.config/celeris/api.token)" "$SD_PROD_API/api/v1/releases" \
     | jq '[.instances[] | {release, role, pid, drained_at}]'
   ```

   API が読めないときは DB を読み取り専用で見る（`status.sh` と同じ。書かない）:

   ```sh
   sqlite3 "file:/local/celeris/data/db/celeris.sqlite3?mode=ro" \
     "select release, role, pid, handoff_requested_at, drained_at from daemon_instances order by started_at"
   ```

3. 生きている `active` の行（`role` が `active`、`drained_at` が無く、`ps --pid <pid>` でプロセスが居る）がちょうど 1 つで、
   その `release` が昇格した sha12 であることを確かめる。`draining` の旧は、run が終わると `drained_at` が付いて消える。
   数時間かかりうる（`status.sh` で見える）。

## 2 つ以上の active が見えたとき

- 生きた active が 2 つ見えたら、daemon は新しい方を残して古い方へ引き継ぎを要求する（ADR-0040 D4 付記）。
  数 tick（数十秒）待って、古い方が `draining` になれば正常。
- 古い方が `active` のまま動かないとき（heartbeat は止まっているのにプロセスが生きている）は、
  自動では奪わない（二重 dispatch になるため）。`ps` で pid を確かめ、人が判断して止める。止めた後は `daemon_instances` の行が消えることを上の手順で確かめる。
- 本番の DB の行を手で消さない。

## `promote.sh` が打ち切ったとき（ADR-0040 付記 2026-10-05b）

`promote.sh` の最後の行で何が起きたかが分かる。

- `live handoff failed: two active instances (… the new celeris@<sha> was stopped; the old one keeps serving …)`:
  旧が生きた `active` のまま新も `active` だったので新を止めた。旧がそのまま本番。上の手順で旧 1 つが active なのを見る。
- `live handoff not confirmed: … The new celeris@<sha> … was left running`:
  `daemon_instances` が読めなかった（API と DB）か、health と表が食い違った。新は止めていない。`current` は旧のまま。
  上の手順で表を読み、新だけが active なら `promote.sh <sha>` を再実行する（「celeris already took over」で続きをやる）。
- `… celeris@<sha> was started again (restarted=true)`:
  新を止めた後に誰も health に答えなかったので新を起こし直した（active 0 を残さない）。`current` は旧のまま。
  `/api/v1/health` が新で `active` なのを見て、`promote.sh <sha>` を再実行する。
- どの場合も、`/api/v1/health` が 000 のまま（active 0）なら `bash /local/celeris/state/releases/<sha>/scripts/promote.sh <sha>`
  を即再実行する（旧が draining で health が無ければ stop-start になる）。
