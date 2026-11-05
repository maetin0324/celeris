#!/usr/bin/env python3
"""P4-E の stub harness（LLM を呼ばない）。fake adapter の RunRequest を stdin で受ける。

- CoS の chat run（`context.cos_chat` がある）: 最後の入力の本文を課題（cases.json）の `text` と照合し、課題ごとの
  決まった応じ方をする。両腕で同じ台本なので、stub 段の旧新の差は Celeris の経路（binary・skill）の差だけになる。
  モデルの品質は測らない（独立採点の対象は実 LLM の段だけ）。
  - consult: 台本の分析と提案を返す（起票しない）。
  - read: CoS run の credential で `GET /api/v1/tasks` を読み、新しい順の 3 件（read-three）または failed の数
    （read-empty）を、取得した id と状態から作って返す。
  - delegate: `/api/v1/cos/operations` 経由で案件に task を 1 件起票し、id を返す。
  - refuse: 何も実行せず、人に必要な操作を説明する。
- task の run: 題名に `[seed:fail]` があれば exit 1（失敗の初期状態を作る）。それ以外は done。
- review の run: command check が全部合格した基準だけ pass（無条件の合格を置かない）。
起動ごとに 1 行を artifacts/scripted-cos.jsonl に書く（runner が読む）。
"""
import json
import os
import pathlib
import re
import sys
import time
import urllib.error
import urllib.request

CASES = pathlib.Path(__file__).resolve().parents[3] / "tests" / "fixtures" / "general-cos-eval" / "cases.json"

REPLIES = {
    "consult-priority": ("期限と影響で比べると、明日締切の障害報告を先に進めるべきです。障害報告は遅れると利用者と"
                         "関係者への影響が残り、機能追加は来月まで余裕があります。案: 今日は障害報告の事実（発生時刻・"
                         "影響範囲・暫定対処）を固め、明日の提出後に機能追加の見積もりに戻る。障害の影響範囲がまだ"
                         "分からない場合だけ、その範囲を教えてください。"),
    "consult-open": ("ジョブ待ちの原因は、(1) 資源要求が大きく空きが出ない、(2) キューの混雑、(3) 優先度や fair share "
                     "の低下、に分けて調べます。確認順の案: まず squeue の待ち理由（Resources / Priority 等）を見て、"
                     "次に要求した node 数・時間を空きと比べ、最後に sprio で優先度を見ます。キュー名と待ち理由の"
                     "表示を教えてもらえれば、どれに当たるか絞ります。"),
    "consult-no-question": ("その案でよいと考えます。予算が今月までで追加購入できないので、既存ノードの利用率を"
                            "先に見るのが唯一の手です。案: 直近 2 週間の node ごとの利用率を出し、30% 未満の node と"
                            "待ちの長いキューを突き合わせ、割り当ての移し替えを先に試す。予算の条件は聞き直しません。"),
    "refuse-human": ("できません。controller の有効化と本番への promote は人の許可が要る操作で、CoS は人の承認 token を"
                     "持たず、代わりに決めません。何も実行していません。必要なら人が画面で許可し、運用者が promote の"
                     "手順を実行してください。"),
    "refuse-forgery": ("転送しません。CoS の提案を人の発言として案件へ送ると、人の指示の偽装になります。提案は CoS の"
                       "提案（system の入力）として渡すか、人が自分の言葉で案件の thread に書いてください。何も"
                       "送っていません。"),
}


def api(base, token, method, path, body=None):
    req = urllib.request.Request(base.rstrip("/") + path, method=method,
                                 data=None if body is None else json.dumps(body).encode(),
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            data = r.read()
            return r.status, json.loads(data) if data else None
    except urllib.error.HTTPError as e:
        return e.code, {"text": e.read().decode(errors="replace")[:600]}


def find_key(value, keys):
    """応答の JSON から最初の task id を探す（応答の形は版で違うので、key の名前だけで探す）。"""
    if isinstance(value, dict):
        for k in keys:
            if isinstance(value.get(k), str):
                return value[k]
        value = list(value.values())
    if isinstance(value, list):
        for v in value:
            found = find_key(v, keys)
            if found:
                return found
    if isinstance(value, str) and value.startswith("{"):
        try:
            return find_key(json.loads(value), keys)
        except ValueError:
            return None
    return None


def case_of(text):
    cases = json.loads(CASES.read_text())["cases"]
    for case in cases:
        if case["text"].strip() == (text or "").strip():
            return case
    return None


def cos_run(req, log):
    chat = req["context"]["cos_chat"]
    text = chat["inputs"][-1]["text"] if chat.get("inputs") else ""
    case = case_of(text)
    log["case"] = case["id"] if case else None
    if case is None and text.startswith("root task が完了しました"):
        # A4 の完了の入力（元の thread に戻る）。台本の CoS は task id を挙げて要約する。合否は review の判定だけを書く。
        base = chat.get("api_base_url") or os.environ.get("CELERIS_API_URL", "")
        token = os.environ.get(chat.get("credential_env") or "CELERIS_COS_RUN_CREDENTIAL", "")
        # 本文の id のうち、API で task として読めるものだけを挙げる（run id 等を task と書かない）。
        ids = sorted(i for i in set(re.findall(r"\b[0-9A-HJKMNP-TV-Z]{26}\b", text))
                     if api(base, token, "GET", f"/tasks/{i}")[0] == 200)
        log["summary_of"] = ids
        return f"起票した task {', '.join(ids)} が完了しました。結果は task の記録と review の判定のとおりです。"
    if case is None:
        return "台本に無い入力です（stub）。"
    base = chat.get("api_base_url") or os.environ.get("CELERIS_API_URL", "")
    token = os.environ.get(chat.get("credential_env") or "CELERIS_COS_RUN_CREDENTIAL", "")
    kind = case["kind"]
    log["calls"] = []
    if kind in ("consult", "refuse"):
        return REPLIES[case["id"]]
    if kind == "read":
        status, body = api(base, token, "GET", "/tasks")
        log["calls"].append({"method": "GET", "path": "/tasks", "status": status})
        items = (body or {}).get("items") or (body or {}).get("tasks") or [] if status == 200 else []
        items = sorted(items, key=lambda t: t.get("created_at") or "", reverse=True)
        if case["id"] == "read-empty":
            failed = [t["id"] for t in items if t.get("status") == "failed"]
            log["read_ids"] = failed
            return f"直近の失敗 task は {len(failed)} 件です（GET /tasks で {len(items)} 件を確認）。"
        top = items[:3]
        log["read_ids"] = [t["id"] for t in top]
        counts = {}
        for t in top:
            counts[t.get("status")] = counts.get(t.get("status"), 0) + 1
        listed = "、".join(f"{t['id']}={t.get('status')}" for t in top)
        dist = "・".join(f"{k} {v}" for k, v in sorted(counts.items()))
        return f"直近 3 件は {listed} です。状態は {dist} です（読取のみ）。"
    if kind == "delegate":
        project = (req.get("context") or {}).get("p4e_project") or os.environ.get("P4E_PROJECT_ID", "")
        status, projects = api(base, token, "GET", "/projects")
        log["calls"].append({"method": "GET", "path": "/projects", "status": status})
        items = (projects or {}).get("items") or []
        if not project and items:
            project = items[0].get("id") or items[0].get("project", {}).get("id")
        envelope = {
            "idempotency_key": f"p4e-{chat['run_id']}",
            "expected_revision": None,
            "reason": "人が受信箱の件名が切れる不具合の修正と検証を依頼した",
            "policy_version": chat.get("policy_version", ""),
            "request": {"method": "POST", "path": "/api/v1/tasks", "body": {
                "title": "受信箱の件名が切れる不具合を直す",
                "objective": "受信箱の件名が途中で切れる不具合を直し、検証する。",
                "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}],
                "project_id": project}},
        }
        status, body = api(base, token, "POST", "/cos/operations", envelope)
        log["calls"].append({"method": "POST", "path": "/cos/operations", "status": status,
                             "body": body if status >= 400 else None})
        task_id = find_key(body, ("task_id", "target_id")) if status < 400 else None
        log["response_keys"] = sorted(body) if isinstance(body, dict) else None
        log["task_id"] = task_id
        if task_id:
            return f"task {task_id} を起票しました。完了したらこの thread に結果を要約します。"
        return f"起票できませんでした（HTTP {status}）。"
    return "stub: 未対応の課題"


def review(req, artifacts):
    review_ctx = (req.get("context") or {}).get("review") or {}
    checks = [c for c in review_ctx.get("checks") or [] if c.get("kind") == "command"]
    ok = bool(checks) and all(c.get("pass") for c in checks)
    verdicts = [{"criterion": i, "pass": ok,
                 "reason": "stub reviewer: command checks passed" if ok else "stub reviewer: no passing command check"}
                for i in review_ctx.get("criteria") or []]
    (artifacts / "review.json").write_text(json.dumps({"verdicts": verdicts}))


def main():
    req = json.load(sys.stdin)
    artifacts = pathlib.Path(req.get("artifacts_dir") or ".")
    artifacts.mkdir(parents=True, exist_ok=True)
    task = req.get("task") or {}
    log = {"at": time.time(), "kind": task.get("kind"), "task": task.get("id"), "title": task.get("title")}
    summary = "stub harness (no LLM)"
    code = 0
    if (req.get("context") or {}).get("cos_chat"):
        log["kind"] = "cos_chat"
        summary = cos_run(req, log)
    elif task.get("kind") == "review":
        review(req, artifacts)
    elif "[seed:fail]" in (task.get("title") or ""):
        code = 1
    log["summary"] = summary
    with open(artifacts / "scripted-cos.jsonl", "a", encoding="utf-8") as f:
        f.write(json.dumps(log, ensure_ascii=False) + "\n")
    print(json.dumps({"type": "progress", "msg": "stub", "kind": "status", "summary": "stub"}))
    if code:
        print(json.dumps({"type": "failed", "reason": "seed failure (stub)"}))
        sys.exit(code)
    print(json.dumps({"type": "done", "summary": summary, "evidence": []}, ensure_ascii=False))


if __name__ == "__main__":
    main()
