---
tasks: [01M44Y0BR7HQRFNRWK2T2P2ZXE]
---

# active が 1 つだけであることを確かめる手順

live 昇格の後に、旧と新の daemon が同時に `active` になっていないことを読み取りだけで確かめる手順。
仕組みは [ADR-0040 の 2026-10-05 付記（D4）](../../agent-docs/adr/0040-self-improvement-deploy.md)。

## 昇格の後

1. `promote.sh` が `handoff done: the new celeris is active and the old one is draining` を出したことを log で見る。
   出ていれば、その時点で active は 1 つ（新）。
2. daemon の一覧を読む（書き込みはしない。`SD_PROD_API` は `config.toml` の API の基底 URL）。

   ```sh
   curl -s "$SD_PROD_API/api/v1/releases" | jq '[.instances[] | {release, role, pid, drained_at}]'
   ```

3. `role` が `active` の行がちょうど 1 つで、その `release` が昇格した sha12 であることを確かめる。
   `draining` の旧は、run が終わると `drained_at` が付いて消える。数時間かかりうる（`status.sh` で見える）。

## 2 つ以上の active が見えたとき

- 生きた active が 2 つ見えたら、daemon は新しい方を残して古い方へ引き継ぎを要求する（ADR-0040 D4 付記）。
  数 tick（数十秒）待って、古い方が `draining` になれば正常。
- 古い方が `active` のまま動かないとき（heartbeat は止まっているのにプロセスが生きている）は、
  自動では奪わない（二重 dispatch になるため）。`ps` で pid を確かめ、人が判断して止める。止めた後は `daemon_instances` の行が消えることを上の手順で確かめる。
- 本番の DB の行を手で消さない。
