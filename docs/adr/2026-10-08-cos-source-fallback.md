# ADR 2026-10-08: CoS の代替経路と Claude 利用枠の予約

---
tasks: [01M4E0WA5ZYVT32A4W6E74GQGC, 01M4EKPXZF7M6QT78Q86GBDCVR]
---

状態: 採用（実装・検証は進捗ファイルに記録）。
関連: agent-docs/adr/2026-10-05-cos-chat-home.md、2026-10-06-cos-chat-harness-adapters.md、0089-cos-runs-bypass-concurrency.md。

## 決定

1. `[cos]` は主経路を維持し、`[[cos.fallbacks]]` に harness / llm_source / provider / account_id / model / tier を順に設定する。代替は明示設定だけを使い、各経路を一度ずつ試す。未ログイン、quota、rejected、cooldown、capacity、利用上限応答で次へ進む。同じ chat run ID・入力・出力 message を維持する。固定 account の別 account への暗黙変更は行わず、明示された次経路へ移る。これは home ADR D2 の「固定 account は待つ」を拡張する。固定 account が使えなければ明示した次経路を試し、代替が無い／尽きた場合は理由付き failed として再送を案内する（無言で queue に残さない）。
2. 切替は旧 worker の終了後。停止・割り込み・未確定の操作は再試行に優先する。各経路の失敗は run status とシステム message に残し、実効経路は chat_runs の既存 resolved_config に更新する。経路の実行開始もチャットに記録する。全経路が失敗した場合、人の出力 message に理由と「利用枠・ログインを確認して再送してください」を残す。時刻を確定できないため、自動再試行時刻は約束しない。
3. harness/source/provider/account/model が変わると既存 session key の照合で旧 session を retire し新 session を作る。DB の要約・未要約履歴・同じ入力と添付を渡す。別 harness の session ID を流用しない。dispatcher/store は文字列・状態の決定的な制御だけを行い、要約を LLM に依頼しない。旧 credential を revoke してから同じ run に新しい credential を発行する。旧 token は再使用できない。再試行先に適用済み操作の receipt を渡し、操作の既存冪等性規則を適用する。未確定の操作は従来の人確認へ送る。
4. ADR-0089 の CoS 同時実行例外（pool concurrency 外、account +1、max_cos_runs）を維持する。追加で `[cos] worker_reserve_five_hour = 0.90`（既定、0 より大きく 0.97 以下）を設ける。CoS 有効時、Claude の観測済み 5 時間使用率が閾値以上の account は通常 worker の新規割当・sticky 再利用から除外し、CoS は従来の 0.97 未満まで使える。reset 時刻以降は解除。観測不明なら既存どおり割当可能。既に走っている worker は停止しないため、予約は新規 admission の保護であって利用可能性の保証ではない。代替経路を併用する。
5. 代替経路へ渡す wall clock 予算は、最初の run 開始からの経過を差し引いた残りとする。daemon 再起動時の孤児回収は既存の仕組みを維持する。各経路の resume 拒否後の fresh retry は既存規則を維持する。

## 設定例（運用者が配送後に適用）

```toml
[cos]
harness = "claude-code"
llm_source = "claude_oauth"
worker_reserve_five_hour = 0.90

[[cos.fallbacks]]
harness = "codex"
llm_source = "codex_oauth"
model = "gpt-6.1-sol"
tier = "frontier"

# 既存の ACP provider がある場合だけ追加する。
# [[cos.fallbacks]]
# harness = "opencode"
# provider = "opencode-go"
# llm_source = "opencode_go"
```

provider/source/model は導入済みの設定と整合する必要がある。本タスクは本番設定・daemon・DB を変更しない。

## 実装付記

設定は daemon 起動時と `POST /api/v1/reload` 時に同じ関数で解決する。`[cos]` の経路（harness / llm_source / provider / account_id / model / tier / fallbacks）・enabled・worker_reserve_five_hour・実行予算は reload 後の新しい run / admission から反映し、triage 設定は次の tick から反映する。実行中 run のハンドル・account/provider の使用数・triage 状態は保持し、同じ run 内の代替経路と予算には開始時の設定を使う。`[cos.attachments]` と `stream_retention_days` は API middleware と背景 GC が起動時に保持するため、変更を含む reload 全体を更新前に 400 で拒否し、再起動が必要と返す。reload 時も DB path と API base URL は稼働 daemon の値を使う。run の最終経路は既存の `harness` / `llm_source` / `provider` / `account_id` / `model` / `tier` に記録し、切替前後の Run event と system message を保存する。既存チャット UI は system message を表示するため、新しい API schema・画面変更は不要。

予約対象は通常 worker の account 選択、sticky 再利用、routing shadow の候補に共通で適用する。CoS の account 使用数は主経路の harness ではなく実際の account adapter と ID で会計する。使用量の不明な account は予約しない。全体・account の同時実行上限と quota は ADR-0089 の例外の範囲内にとどめる。

この ADR は起動済み worker の停止や既存失敗 run の自動再開を導入しない。全候補失敗の返答では理由を表示し、枠・ログインの確認後の再送を案内する。LLM に接続しない試験結果は [進捗](../progress/2026-10-08-cos-source-fallback.md) を参照。
