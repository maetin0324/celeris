use super::*;

const NOW: &str = "2026-11-05T12:00:00Z";

/// 試験用の DB（案件 `p` の CoS thread `pc`、他案件 `q` の thread、案件 `p` の task `t1`）。
fn fixture() -> (tempfile::TempDir, SqliteStore, rusqlite::Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("budget.db");
    let store = SqliteStore::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO chat_threads (id,kind,title,project_id,status,created_at,updated_at)
         VALUES ('pc','project','project','p','open','2026-11-05T00:00:00Z','2026-11-05T00:00:00Z'),
                ('other','project','other','q','open','2026-11-05T00:00:00Z','2026-11-05T00:00:00Z');
         INSERT INTO tasks (id,status,kind,priority,created_at,json,project_id)
         VALUES ('t1','running','coding',0,'2026-11-05T00:00:00Z','{}','p');",
    )
    .unwrap();
    (dir, store, conn)
}

fn chat_run(
    conn: &rusqlite::Connection,
    id: &str,
    thread: &str,
    at: &str,
    model: Option<&str>,
    usage: Option<&str>,
) {
    let cfg = model.map_or("{}".to_owned(), |m| format!("{{\"model\":\"{m}\"}}"));
    conn.execute(
        "INSERT INTO chat_runs (run_id,thread_id,input_message_id,state,started_at,finished_at,usage_json,resolved_config_json)
         VALUES (?1,?2,'input','completed',?3,?3,?4,?5)",
        rusqlite::params![id, thread, at, usage, cfg],
    )
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
fn task_run(
    conn: &rusqlite::Connection,
    id: &str,
    status: &str,
    finished: bool,
    model: Option<&str>,
    usage: Option<&str>,
) {
    conn.execute(
        "INSERT INTO runs (run_id,task_id,role,seq,status,model,usage_json,started_at,finished_at)
         VALUES (?1,'t1','worker',1,?2,?3,?4,'2026-11-05T01:00:00Z',?5)",
        rusqlite::params![
            id,
            status,
            model,
            usage,
            finished.then_some("2026-11-05T02:00:00Z")
        ],
    )
    .unwrap();
}

fn at_now() -> OffsetDateTime {
    OffsetDateTime::parse(NOW, &Rfc3339).unwrap()
}

fn limit_usd(usd: u64) -> BudgetLimits {
    BudgetLimits {
        max_usd_per_day_micros: usd * 1_000_000,
        ..BudgetLimits::default()
    }
}

#[test]
fn chat_runs_are_project_scoped_daily_and_priced() {
    let (_d, store, conn) = fixture();
    // claude-sonnet-5: input 2.0 / output 10.0 USD per 100 万 token。1,000,000 in + 100,000 out = 3.0 USD。
    let usage = r#"{"input_tokens":1000000,"output_tokens":100000}"#;
    chat_run(
        &conn,
        "today",
        "pc",
        "2026-11-05T01:00:00Z",
        Some("claude-sonnet-5"),
        Some(usage),
    );
    chat_run(
        &conn,
        "yesterday",
        "pc",
        "2026-11-04T01:00:00Z",
        Some("claude-sonnet-5"),
        Some(usage),
    );
    chat_run(
        &conn,
        "other",
        "other",
        "2026-11-05T01:00:00Z",
        Some("claude-sonnet-5"),
        Some(usage),
    );
    let rates = BudgetRates::default();
    let u = usage_fn(&store, &rates);
    assert_eq!(u.usd_micros, 3_000_000);
    assert_eq!(u.usd_unknown_runs, 0);
    assert_eq!(u.tokens, 1_100_000, "tokens は表示用");
    assert_eq!(u.chat_starts, 1);
    let start_limit = BudgetLimits {
        max_cos_inputs_per_day: 1,
        ..BudgetLimits::default()
    };
    assert_eq!(exceeded(&start_limit, &u), Some(BudgetKind::CosInputs));
    assert_eq!(
        exceeded(&limit_usd(3), &u),
        Some(BudgetKind::Usd),
        "上限ちょうどで止まる"
    );
    assert_eq!(exceeded(&limit_usd(4), &u), None);
    let next = usage_on_day(
        &store,
        "p",
        "2026-11-06",
        at_now() + time::Duration::days(1),
        &rates,
    )
    .unwrap();
    assert_eq!(next.usd_micros, 0);
}

fn usage_fn(store: &SqliteStore, rates: &BudgetRates) -> BudgetUsage {
    usage(store, "p", at_now(), rates).unwrap()
}

#[test]
fn cache_read_and_write_are_counted_at_their_own_prices() {
    let (_d, store, conn) = fixture();
    // 同じ input + output（5,500 token）でも、cache read 47 万 token の run は cache read の単価（0.2 USD/100 万）で
    // 数えるので 0.094 USD 増えるだけ。cache write 47 万 token なら 2.5 USD/100 万で 1.175 USD 増える。
    let few = r#"{"input_tokens":5000,"output_tokens":500}"#; // 0.01 + 0.005 = 0.015 USD
    let many_read = r#"{"input_tokens":5000,"output_tokens":500,"cache_read_tokens":470000}"#;
    let many_write = r#"{"input_tokens":5000,"output_tokens":500,"cache_creation_tokens":470000}"#;
    let rates = BudgetRates::default();
    let cost_of = |usage: &str| {
        let (_d, store, conn) = fixture();
        task_run(
            &conn,
            "r",
            "completed",
            true,
            Some("claude-sonnet-5"),
            Some(usage),
        );
        usage_fn(&store, &rates).usd_micros
    };
    assert_eq!(cost_of(few), 15_000);
    assert_eq!(cost_of(many_read), 15_000 + 94_000);
    assert_eq!(cost_of(many_write), 15_000 + 1_175_000);
    // 同じ上限でも判定が単価どおりに変わる（0.1 USD 上限: few は通り、many_read は止まる）。
    task_run(
        &conn,
        "r",
        "completed",
        true,
        Some("claude-sonnet-5"),
        Some(few),
    );
    assert_eq!(
        exceeded(
            &BudgetLimits {
                max_usd_per_day_micros: 100_000,
                ..BudgetLimits::default()
            },
            &usage_fn(&store, &rates)
        ),
        None
    );
    let (_d2, store2, conn2) = fixture();
    task_run(
        &conn2,
        "r",
        "completed",
        true,
        Some("claude-sonnet-5"),
        Some(many_read),
    );
    assert_eq!(
        exceeded(
            &BudgetLimits {
                max_usd_per_day_micros: 100_000,
                ..BudgetLimits::default()
            },
            &usage_fn(&store2, &rates)
        ),
        Some(BudgetKind::Usd)
    );
}

#[test]
fn unpriced_model_and_missing_usage_stop_as_unknown_cost() {
    let rates = BudgetRates::default();
    let ok = r#"{"input_tokens":1000,"output_tokens":100}"#;
    // 単価表に無い model（local の Qwen）は 0 USD ではなく費用不明。
    let (_d, store, conn) = fixture();
    task_run(
        &conn,
        "r",
        "completed",
        true,
        Some("qwen3.8-flash-next"),
        Some(ok),
    );
    let u = usage_fn(&store, &rates);
    assert_eq!((u.usd_micros, u.usd_unknown_runs), (0, 1));
    assert_eq!(exceeded(&limit_usd(100), &u), Some(BudgetKind::UsdUnknown));
    assert_eq!(severity(&limit_usd(100), &u), BudgetSeverity::Dangerous);
    // usage が無い終わった run（model あり）も費用不明。
    let (_d, store, conn) = fixture();
    task_run(&conn, "r", "failed", true, Some("claude-sonnet-5"), None);
    assert_eq!(usage_fn(&store, &rates).usd_unknown_runs, 1);
    // CoS chat run の usage null も費用不明。
    let (_d, store, conn) = fixture();
    chat_run(
        &conn,
        "c",
        "pc",
        "2026-11-05T01:00:00Z",
        Some("claude-sonnet-5"),
        None,
    );
    assert_eq!(usage_fn(&store, &rates).usd_unknown_runs, 1);
    // 明示の換算単価（budget_rates）があれば数えられる。単価 0 の宣言は 0 USD として数える。
    let (_d, store, conn) = fixture();
    task_run(
        &conn,
        "r",
        "completed",
        true,
        Some("qwen3.8-flash-next"),
        Some(ok),
    );
    let rate = |v: f64| {
        BudgetRates::new([(
            "qwen3.8".to_owned(),
            task_core::pricing::BudgetRate {
                input_per_million: v,
                output_per_million: v * 2.0,
                cache_read_per_million: 0.0,
                cache_write_per_million: 0.0,
            },
        )])
        .unwrap()
    };
    let u = usage_fn(&store, &rate(1.0));
    assert_eq!((u.usd_micros, u.usd_unknown_runs), (1_200, 0)); // 1000*1 + 100*2 = 1200 micro-USD
    assert_eq!(exceeded(&limit_usd(100), &u), None);
    let u = usage_fn(&store, &rate(0.0));
    assert_eq!((u.usd_micros, u.usd_unknown_runs), (0, 0));
}

#[test]
fn running_and_never_started_runs_are_not_counted() {
    let (_d, store, conn) = fixture();
    let ok = r#"{"input_tokens":1000000,"output_tokens":0}"#;
    // 走っている run、harness 起動前に終わった run（記録が全部無い）は数えない。
    task_run(
        &conn,
        "running",
        "running",
        false,
        Some("claude-sonnet-5"),
        None,
    );
    task_run(&conn, "never", "failed", true, None, None);
    task_run(
        &conn,
        "done",
        "completed",
        true,
        Some("claude-sonnet-5"),
        Some(ok),
    );
    let u = usage_fn(&store, &BudgetRates::default());
    assert_eq!((u.usd_micros, u.usd_unknown_runs), (2_000_000, 0));
}

#[test]
fn route_prefixed_model_is_priced_and_severity_uses_double_limit() {
    let (_d, store, conn) = fixture();
    let ok = r#"{"input_tokens":1000000,"output_tokens":0}"#;
    task_run(
        &conn,
        "r",
        "completed",
        true,
        Some("claude/claude-sonnet-5"),
        Some(ok),
    );
    let u = usage_fn(&store, &BudgetRates::default());
    assert_eq!(u.usd_micros, 2_000_000);
    assert_eq!(severity(&limit_usd(1), &u), BudgetSeverity::Dangerous);
    assert_eq!(severity(&limit_usd(2), &u), BudgetSeverity::Daily);
}

#[test]
fn project_budget_signature_ignores_numbers_and_spacing() {
    let a = failure_signature("root_failed", "check failed: exit 1 after  12s");
    let b = failure_signature("root_failed", "check failed: exit 2 after 9s");
    let c = failure_signature("root_failed", "build failed");
    let d = failure_signature("review_failed", "check failed: exit 1 after 12s");
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(a, d);
}

#[test]
fn project_budget_exceeded_at_the_limit() {
    let l = BudgetLimits {
        max_cos_inputs_per_day: 2,
        ..BudgetLimits::default()
    };
    let mut u = BudgetUsage::default();
    assert_eq!(exceeded(&l, &u), None);
    u.cos_inputs = 2;
    assert_eq!(exceeded(&l, &u), Some(BudgetKind::CosInputs));
    u.cos_inputs = 0;
    u.usd_micros = l.max_usd_per_day_micros;
    assert_eq!(exceeded(&l, &u), Some(BudgetKind::Usd));
}
