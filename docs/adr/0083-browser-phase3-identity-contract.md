# ADR-0083: Browser Phase 3 の Browser Identity の契約（P3-A）

---
tasks: [01M3PBAVFAYPDWMQMDBXPTE2V8]
---

- 日付: 2026-09-29
- 状態: **Accepted（束縛・期限・失効・混入拒否の純粋関数だけ）。封緘（AEAD）・保存・worker への配線は未実装。利用は P4-A の後**
- 関連: [ADR-0078](0078-browser-execution-capability.md) D8 P3-A、[ADR-0080](0080-browser-phase2-policy-broker-approval.md) H1・H3・H6、[ADR-0081](0081-browser-phase3-control-lease.md)、[ADR-0082](0082-browser-phase3-live-proxy-acl.md)、人の決定 H5（需要が確認された project+origin に限り期限付き identity）

## 範囲

本 ADR が決めるのは「どの identity をどの project/origin の browser session に戻してよいか」の契約だけである。
実装は `crates/task-core/src/browser_identity.rs`（I/O・時計・暗号を持たない。`now` は呼び出し側が渡す）。

## D2. identity の規則

1. identity は project+origin の組に 1 対 1 で束縛する（H5）。origin は正規化した https origin だけ。
2. 登録には人の確認の参照（`demand_confirmed_by`）が要る。無ければ `demand_not_confirmed`。自動では作らない。
3. 期限は既定 7 日、上限 30 日。上限を超える要求は切り詰めずに `ttl_too_long` で拒否する。期限に達したら `identity_expired`。
4. 鍵は project+origin 単位。`key_label` は project と origin の SHA-256 で、label から project/origin は読めない。broker（celeris-credentiald）がこの label から鍵を導出する。
5. 封緘の AAD は identity・project・origin・世代を束縛する。別の identity の封緘は開けない。
6. 失効は世代を上げる。以前の封緘は `stale_generation` で拒否する。削除は tombstone だけを残し、呼び出し側が封緘と鍵を消す。
7. 混入の拒否: 封緘の外側の identity / project / origin / key label が一致しなければ拒否する。保存・復元する state の項目に identity の origin 以外が 1 件でもあれば、全体を `foreign_origin` で拒否する（部分的に捨てて通さない）。
8. エラーは固定コードだけを返す。

## D3. P4-A との契約（循環を作らない）

- identity の利用（browser session への復元）は `Isolation::Isolated` の session だけに許す。ADR-0080 H6 の trusted local は `isolation_required` で拒否する。
- `Isolated` を名乗れるのは P4-A（container / 別 UID / egress 制限）が入ってから。それまで task-core の外に `Isolated` を作る経路を置かない。
- したがって Phase 3 で入るのは契約と判定だけで、利用は Phase 4 の後になる。P4-A は本 ADR の判定に依存しない。

## D4. agent-browser 0.38.1 の allowlist と restore の境界

- ADR-0078 の契約どおり、Celeris は `--restore`・`--state`・`--profile`・CDP attach を agent に渡さない。action の allowlist も identity のために広げない（解除だけで対応しない）。
- 0.38.1 の restore は origin 単位の絞り込みを持たないと仮定する（未検証）。そのままでは project/origin 単位の束縛と合わないので、identity の復元は agent の action ではなく、broker が run の開始前に行う別の経路にする。復元する state は D2-7 の検査を通ったものだけ。
- 0.38.1 の restore の実際の挙動はこの ADR の時点で確かめていない。上の仮定は安全側（restore を使わない）に倒すための前提であり、配線の前に固定 version 上の負例で確かめる。
- identity を復元した session は credential を注入した session と同じ扱いにし、session の終わりまで LLM の観測と Live View を止める（ADR-0080 H3、ADR-0082 D2-4）。

## 未実装（後続）

- 封緘の実体（celeris-credentiald の chacha20poly1305 で `key_label`・`aad` を使う）、保存、削除時の鍵の消去。
- 登録・失効・削除の API と GUI、期限切れの掃除。
- worker の browser 実行への配線（P4-A の後）。
