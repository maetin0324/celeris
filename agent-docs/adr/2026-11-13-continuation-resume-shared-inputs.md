---
tasks: [01M7B2KBNJ72C5F75RAEWSFECG]
status: accepted
---
# ADR 2026-11-13: continuation-resume で共有する照合を、保存 session の id・context・token に絞る

- 状態: 採用（人の決定 fresh-contract の答え a。実装は task 01M7B2KBNJ72C5F75RAEWSFECG の WorkUnit settle-fresh）
- 関係: [ADR 2026-11-11 Dispatcher の状態の owner と、判断と副作用の分け方](2026-11-11-dispatcher-state-ownership.md) D4 の
  continuation-resume の行と `sessions::stored_session_fresh_reason` の契約を絞る付記。ADR 2026-11-11 の本文は変えない。
  [ADR-0140](0140-claude-session-resume.md) D1（task・WU の判断表）、
  [ADR 2026-10-05 CoS チャット](2026-10-05-cos-chat-home.md) D2（CoS chat の session の resume と retire）

## 背景

ADR 2026-11-11 D4 は、task・WU の `sessions::decide_continuation` と CoS chat の `sessions::cos_chat::decide_cos_chat_session` が
共有する `sessions::stored_session_fresh_reason` の入力を「照合 key の一致・前回の拒否・id の妥当性・context の枯渇・測った token と上限」
と決めた。WorkUnit settle-fresh の調べ（[settle-fresh.md](../progress/2026-11-11-health-v2-r4/settle-fresh.md)）で、この契約のままでは
2 つの関数から等価に呼べないと分かった。

- 前回の拒否と照合 key を見る順が逆である。
  - task・WU は拒否を先に見る。試験 `session_resume_session_reuse_rejected_resume_falls_back_to_checkpoint` は、直前が `Failed` でも、
    保存 session が無くても `ResumeRejected` を求める。拒否で選ばれる `ResumeRejected` だけが `dispatcher/continuation_session.rs` で
    直近の checkpoint の文脈（`latest_checkpoint_context`）を前置きに入れる。
  - CoS chat は照合 key を先に見る。試験 `cos_chat_run_session_reason_precedence_is_fixed` は、key の変化と拒否が重なると `key_changed` を求める。
- task・WU では、拒否の判定と key・id・token の判定の間に呼び出し元だけの判定（continuation でない・設定の fresh 指定・保存 session が無い）がある。
  拒否と key を 1 回の呼び出しで共有すると、どちらかの位置に寄せることになり、理由ごとの session の作成と引退が入力によって変わる。
- 順を引数で切り替えると、共通の関数に呼び出し元ごとの分岐を足すことになる。

## 決定

- **D1 共有する入力と返す値**: `sessions::stored_session_fresh_reason` の入力は、保存 session の id の妥当性を見るための
  adapter と id、context の枯渇、測った token と上限の 4 つにする。返す値は Fresh にする理由（不正な id・context の枯渇・token の上限）で、
  理由が無ければ Resume（`None`）。順は id → context → token に固定する。
  - task・WU は context の枯渇と token の上限をどちらも `ContextRollover` に写すので、context と token の順は結果を変えない。
  - CoS chat の cache の消失は呼び出し元の判定のまま id より前に見る。cache の消失も不正な id も `CacheMissing` なので、順は結果を変えない。
- **D2 各関数に残すもの**: 照合 key（task・WU は adapter → account・provider → cwd、CoS chat は方針の部分 → それ以外）、
  前回の拒否、CoS chat の cache の有無、task・WU の cwd、役割・continuation かどうか・設定の fresh 指定・container は各関数に残す。
  - 拒否と key を見る順が task・WU と CoS chat で逆で、既存の試験 2 本（背景に挙げた）が今の順を求める。この 2 本の assertion は変えない。
  - 拒否だけが checkpoint の文脈を前置きに入れる。task・WU で拒否を後ろに寄せると、拒否と adapter の変化が重なった run が checkpoint の文脈を失う。
  - cwd の不一致は task・WU では `SurfaceUnsupported`（session を作らない）で、CoS chat では照合 key の一部（`KeyChanged`）である。意味が違うので共有しない。
- **D3 変えないもの**: token の測り方（task・WU は `approx_tokens`、CoS chat は `rollover_measure`）、共有の理由から各 enum への写し方、
  理由ごとの session の作成と引退（`starts_session`・`retire`）、session の解決（`resolve_continuation_session`）は変えない。
  node 単位の session の `sessions::decide`（ADR-0054 D1）は D4 の表に無く、id → account → token の順で見るので、この共有に含めない。
- **D4 ADR 2026-11-11 D4 の表との差**: D4 の表の continuation-resume の行の「共有する」の列にある「拒否」と、契約の入力の
  「照合 key の一致」「前回の拒否」は共有しない。表の本文は変えず、この ADR を正本の付記とする。

## 残る問い

- 拒否と照合 key を見る順を task・WU と CoS chat で 1 つにそろえるか（どちらかの振る舞いと試験の assertion を変える）は、後で人が決める。
  そろえると決めたら、そのときに照合 key と拒否を共有の入力に戻せる。
