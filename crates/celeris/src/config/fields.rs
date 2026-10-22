//! ADR 2026-10-22-dynamic-config D1: config.toml の欄の分類（起動時のみ / 動的 / 種）と、設定画面が読む欄の一覧。
//!
//! 分類表（`RULES`）は欄の前置き（`.` 区切り）の最長一致。どれにも当たらない欄は起動時のみとして扱う
//! （再起動が要ると言い過ぎる方へ倒す。動的だと言って効かないよりよい）。

use std::collections::BTreeMap;

use serde_json::Value;

/// 欄の分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldClass {
    /// 稼働中の daemon が新しい値を使う。
    Dynamic,
    /// 起動時にしか決められない（再起動が要る）。
    Startup,
    /// 空の DB の種にだけ使う。真実は DB（別の画面・API）。
    Seed,
}

impl FieldClass {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldClass::Dynamic => "dynamic",
            FieldClass::Startup => "startup",
            FieldClass::Seed => "seed",
        }
    }
}

/// 反映の単位（`apply_unit`）。
pub const NEXT_TICK: &str = "next_tick";
pub const NEXT_DISPATCH: &str = "next_dispatch";
pub const RESTART: &str = "restart";
pub const ELSEWHERE: &str = "elsewhere";

/// 分類表の 1 行。
#[derive(Debug, Clone, Copy)]
pub struct FieldRule {
    pub key: &'static str,
    pub class: FieldClass,
    pub apply_unit: &'static str,
    pub reason: &'static str,
}

const fn dynamic(key: &'static str, apply_unit: &'static str, reason: &'static str) -> FieldRule {
    FieldRule {
        key,
        class: FieldClass::Dynamic,
        apply_unit,
        reason,
    }
}

const fn startup(key: &'static str, reason: &'static str) -> FieldRule {
    FieldRule {
        key,
        class: FieldClass::Startup,
        apply_unit: RESTART,
        reason,
    }
}

pub const RULES: &[FieldRule] = &[
    // ---- 動的: 次の tick から ----
    dynamic(
        "max_concurrency",
        NEXT_TICK,
        "dispatcher が tick ごとに実効値を解く（store の上書きが勝つ）",
    ),
    dynamic(
        "error_cooldown_secs",
        NEXT_TICK,
        "dispatcher の設定を差し替える",
    ),
    dynamic(
        "lease_grace_secs",
        NEXT_TICK,
        "dispatcher の設定を差し替える",
    ),
    dynamic("idle_timeout_secs", NEXT_DISPATCH, "run の起動時に読む"),
    dynamic("kill_grace_secs", NEXT_DISPATCH, "run の起動時に読む"),
    dynamic(
        "review_timeout_secs",
        NEXT_DISPATCH,
        "review run の起動時に読む",
    ),
    dynamic("retry_backoff_base_secs", NEXT_TICK, "次の再試行の待ちから"),
    dynamic("retry_backoff_max_secs", NEXT_TICK, "次の再試行の待ちから"),
    dynamic("max_requeues", NEXT_TICK, "次の requeue の判断から"),
    dynamic("plan", NEXT_TICK, "計画の子の承認の判断から"),
    dynamic("reviewer", NEXT_DISPATCH, "次に起動する review run から"),
    dynamic("review", NEXT_TICK, "次の review の判断から"),
    dynamic("dispatch", NEXT_TICK, "次の dispatch の判断から"),
    dynamic(
        "execution",
        NEXT_DISPATCH,
        "gate・repair・replan・並列度・continuation は次にその判断をするところから（dispatcher が共有の設定から読む）",
    ),
    dynamic(
        "sessions.rollover_tokens",
        NEXT_DISPATCH,
        "次の継続 run から",
    ),
    dynamic(
        "sessions.continuation_resume",
        NEXT_DISPATCH,
        "次の継続 run から",
    ),
    dynamic("routing", NEXT_DISPATCH, "次の routing の判断から"),
    dynamic(
        "routing_eval.reserved_runs",
        NEXT_TICK,
        "次の tick の枠の計算から",
    ),
    dynamic(
        "jev",
        NEXT_TICK,
        "tick ループが設定の変化を見て分類段を作り直す",
    ),
    dynamic("llm_sources", NEXT_DISPATCH, "route の登録簿を作り直す"),
    dynamic("llm_routes", NEXT_DISPATCH, "route の登録簿を作り直す"),
    dynamic("route_defaults", NEXT_DISPATCH, "route の登録簿を作り直す"),
    dynamic("route_selection", NEXT_DISPATCH, "route の登録簿を作り直す"),
    dynamic(
        "harness_selection",
        NEXT_DISPATCH,
        "execution class の既定の候補。route の登録簿を作り直す",
    ),
    dynamic(
        "harness_declarations",
        NEXT_DISPATCH,
        "harness の宣言。adapter 一式を作り直す",
    ),
    dynamic(
        "harnesses",
        NEXT_DISPATCH,
        "harness の宣言。adapter 一式を作り直す",
    ),
    dynamic(
        "adapters",
        NEXT_DISPATCH,
        "adapter 一式を作り直す（次に起動する run から）",
    ),
    dynamic(
        "roles",
        NEXT_DISPATCH,
        "次に起動する run（と委譲の子）の budget から",
    ),
    dynamic("genres", NEXT_DISPATCH, "次に起動する run から"),
    dynamic("delegation", NEXT_DISPATCH, "次の委譲から"),
    dynamic("reports", NEXT_TICK, "次の tick の報告から"),
    dynamic("notify", NEXT_TICK, "次の通知から"),
    dynamic("conversation", NEXT_DISPATCH, "次の対話 run から"),
    dynamic("cos", NEXT_DISPATCH, "CoS の対話 run の起動時に読む"),
    dynamic(
        "selfdeploy.delivery_default_departments",
        NEXT_TICK,
        "次の配送の判断から",
    ),
    dynamic("maintenance", NEXT_TICK, "掃除・ディスク監視を組み直す"),
    // ---- 起動時のみ ----
    startup(
        "cos.attachments",
        "upload の middleware が起動時にこの値を持つ",
    ),
    startup(
        "cos.stream_retention_days",
        "背景の GC が起動時にこの値を持つ",
    ),
    startup(
        "db",
        "DB を開くのは起動時の 1 回（path・busy_timeout・checkpoint）",
    ),
    startup(
        "workspace_root",
        "足回りの root。store の上書き（ローカル設定画面）が勝つ。設定ファイルの値は起動時に解く",
    ),
    startup("tick_ms", "tick ループの周期は起動時に決まる"),
    startup(
        "api",
        "listen address・token・TLS は socket を開く起動時に決まる",
    ),
    startup("clusters", "ssh master・forward の表を起動時に組む"),
    startup(
        "accounts",
        "AccountBook の保存先と root が起動時に固定される（個々の account の max_runs は PATCH /accounts で DB が正）",
    ),
    startup("secrets", "secret の置き場。値そのものはこの画面で扱わない"),
    startup("memory", "記憶の置き場"),
    startup("handoff", "旧 daemon との引き継ぎは起動時の手順"),
    startup(
        "selfdeploy",
        "releases の置き場・昇格の手順は起動時に決まる",
    ),
    startup("delivery", "配送の設定は起動時に読む"),
    startup("workspace", "作業場所・build cache の配置は起動時に決まる"),
    startup(
        "scratch",
        "scratch pool の seed・reflink の基盤は起動時に作る",
    ),
    startup("github", "GitHub 連携の token と接続は起動時に作る"),
    startup("containers", "コンテナ runtime を起動時に 1 度だけ調べる"),
    startup("cron", "cron の executor 表は起動時に組む"),
    startup("storage", "保存先の基盤"),
    startup("knowledge", "知識の根と langmem の接続を起動時に作る"),
    startup("docs_maintenance", "docs の整理の executor は起動時に組む"),
    startup("llm_proxy", "proxy の listener は起動時に開く"),
    startup("model_catalog", "モデル目録は起動時に読む（上書きは DB）"),
    startup("mcp", "MCP の listener は起動時に開く"),
    startup(
        "browser",
        "browser runtime・egress・launcher の基盤（userns / sandbox）は起動時に決まる",
    ),
    startup(
        "routing_eval",
        "評価 run の基盤は起動時に作る（reserved_runs だけ動的）",
    ),
    startup(
        "sessions",
        "session の基盤は起動時に作る（rollover_tokens・continuation_resume だけ動的）",
    ),
    FieldRule {
        key: "org",
        class: FieldClass::Seed,
        apply_unit: ELSEWHERE,
        reason: "空の DB の種。組織の真実は DB（POST/PATCH /api/v1/org）",
    },
    FieldRule {
        key: "org_include",
        class: FieldClass::Seed,
        apply_unit: ELSEWHERE,
        reason: "空の DB の種。組織の真実は DB（POST/PATCH /api/v1/org）",
    },
];

const UNKNOWN_REASON: &str = "分類表に無い欄は起動時に読むものとして扱う";

/// 欄 `key` の分類（最長一致）。
pub fn rule_for(key: &str) -> FieldRule {
    let mut best: Option<&FieldRule> = None;
    for rule in RULES {
        let hit = key == rule.key
            || key
                .strip_prefix(rule.key)
                .is_some_and(|r| r.starts_with('.'));
        if hit && best.is_none_or(|b| rule.key.len() > b.key.len()) {
            best = Some(rule);
        }
    }
    match best {
        Some(rule) => *rule,
        None => FieldRule {
            key: "",
            class: FieldClass::Startup,
            apply_unit: RESTART,
            reason: UNKNOWN_REASON,
        },
    }
}

/// 値を伏せる欄か（欄の末尾の名前で判断する。secret の値は設定画面にも履歴にも出さない）。
pub fn is_secret_key(key: &str) -> bool {
    let leaf = key.rsplit('.').next().unwrap_or(key).to_ascii_lowercase();
    [
        "secret",
        "token",
        "password",
        "api_key",
        "apikey",
        "credential",
        "private_key",
    ]
    .iter()
    .any(|word| leaf.contains(word))
}

pub const REDACTED: &str = "<redacted>";

/// 値を JSON にして、secret の欄なら伏せる。
pub fn display_value(key: &str, value: &Value) -> Value {
    if is_secret_key(key) && !value.is_null() {
        Value::String(REDACTED.to_string())
    } else {
        value.clone()
    }
}

/// 設定ファイルの本文を欄ごとに平らにする。table は再帰し、配列（`[[roles]]` を含む）は 1 つの欄の値にする。
pub fn flatten(text: &str) -> Result<BTreeMap<String, Value>, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let json = serde_json::to_value(&table).map_err(|e| e.to_string())?;
    let mut out = BTreeMap::new();
    if let Value::Object(map) = json {
        for (k, v) in map {
            flatten_into(&k, v, &mut out);
        }
    }
    Ok(out)
}

fn flatten_into(prefix: &str, value: Value, out: &mut BTreeMap<String, Value>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (k, v) in map {
                flatten_into(&format!("{prefix}.{k}"), v, out);
            }
        }
        other => {
            out.insert(prefix.to_string(), other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix_wins_and_unknown_is_startup() {
        assert_eq!(rule_for("execution.max_replans").class, FieldClass::Dynamic);
        assert_eq!(rule_for("cos.max_turns").class, FieldClass::Dynamic);
        assert_eq!(
            rule_for("cos.attachments.max_bytes").class,
            FieldClass::Startup
        );
        assert_eq!(
            rule_for("sessions.rollover_tokens").class,
            FieldClass::Dynamic
        );
        assert_eq!(rule_for("sessions.other").class, FieldClass::Startup);
        assert_eq!(rule_for("executionx").class, FieldClass::Startup);
        assert_eq!(rule_for("something_new").class, FieldClass::Startup);
        assert_eq!(rule_for("api.listen").apply_unit, RESTART);
        assert_eq!(rule_for("org").class, FieldClass::Seed);
    }

    #[test]
    fn secrets_are_redacted() {
        assert!(is_secret_key("notify.discord_webhook_secret"));
        assert!(is_secret_key("api.token_file"));
        assert!(!is_secret_key("execution.max_replans"));
        assert_eq!(
            display_value("x.api_key", &Value::String("sk-1".into())),
            Value::String(REDACTED.into())
        );
    }

    #[test]
    fn flatten_keeps_arrays_as_one_field() {
        let m = flatten("a = 1\n[b]\nc = 2\n[[roles]]\nid = \"x\"\n").unwrap();
        assert_eq!(m["a"], serde_json::json!(1));
        assert_eq!(m["b.c"], serde_json::json!(2));
        assert!(m["roles"].is_array());
    }
}
