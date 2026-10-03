//! ADR-0131 D9: cron 式と次回時刻の純関数試験。DST は host の tzdata に依存しないよう POSIX TZ で固定する。

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::*;

/// 米国東部（2026: 3/8 02:00 → 03:00、11/1 02:00 → 01:00）。
const US_EASTERN: &str = "EST5EDT,M3.2.0,M11.1.0";

fn t(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339).unwrap()
}

fn sched(expr: &str) -> CronSchedule {
    expr.parse().unwrap()
}

fn utc() -> CronTz {
    CronTz::posix("UTC0").unwrap()
}

fn eastern() -> CronTz {
    CronTz::posix(US_EASTERN).unwrap()
}

fn next(expr: &str, tz: &CronTz, after: &str) -> Option<OffsetDateTime> {
    next_after(&sched(expr), tz, t(after)).unwrap()
}

/// `after` から `n` 回の発火時刻を順に並べる。
fn series(expr: &str, tz: &CronTz, after: &str, n: usize) -> Vec<OffsetDateTime> {
    let s = sched(expr);
    let mut out = Vec::new();
    let mut cursor = t(after);
    for _ in 0..n {
        let Some(next) = next_after(&s, tz, cursor).unwrap() else {
            break;
        };
        out.push(next);
        cursor = next;
    }
    out
}

#[test]
fn cron_job_parse_rejects_invalid_expressions() {
    for bad in [
        "",
        "* * * *",
        "* * * * * *",
        "60 * * * *",
        "* 24 * * *",
        "* * 0 * *",
        "* * 32 * *",
        "* * * 0 *",
        "* * * 13 *",
        "* * * * 8",
        "5-1 * * * *",
        "*/0 * * * *",
        "1/5 * * * *",
        "1,,2 * * * *",
        "x * * * *",
        "L * * * *",
        "0 0 L * *",
        "0 0 * * 1#2",
        "@reboot",
        "@every 5m",
        // 暦の上で存在しない日。
        "0 0 31 2 *",
        "0 0 30 2 *",
        "0 0 31 4,6,9,11 *",
    ] {
        let err = bad.parse::<CronSchedule>().unwrap_err();
        assert!(
            matches!(err, CronError::Expression { .. }),
            "{bad:?} should be rejected, got {err:?}"
        );
    }
}

#[test]
fn cron_job_parse_accepts_names_aliases_and_sunday_seven() {
    let s = sched("0 9 * jan-mar,DEC mon-fri");
    assert!(s.month.has(1) && s.month.has(3) && s.month.has(12) && !s.month.has(4));
    assert!(s.dow.has(1) && s.dow.has(5) && !s.dow.has(0) && !s.dow.has(6));
    // 7 も日曜。
    let sun = sched("0 0 * * 7");
    assert!(sun.dow.has(0) && !sun.dow.has(7));
    assert_eq!(sched("@daily").minute, sched("0 0 * * *").minute);
    assert_eq!(sched("@hourly").hour, sched("0 * * * *").hour);
    assert_eq!(sched("  30 4 * * *  ").expr(), "30 4 * * *");
    // 2/29 は閏年にある。
    assert!("0 12 29 2 *".parse::<CronSchedule>().is_ok());
    // 刻み。
    let step = sched("*/15 8-18/5 * * *");
    assert!(step.minute.has(0) && step.minute.has(45) && !step.minute.has(50));
    assert!(step.hour.has(8) && step.hour.has(13) && step.hour.has(18) && !step.hour.has(9));
}

#[test]
fn cron_job_next_after_daily_in_asia_tokyo() {
    let tokyo = CronTz::iana("Asia/Tokyo").unwrap();
    // 2026-10-02 09:00 JST の後の 04:30 JST は翌日。
    assert_eq!(
        next("30 4 * * *", &tokyo, "2026-10-02T00:00:00Z"),
        Some(t("2026-10-02T19:30:00Z"))
    );
    // 予定時刻ちょうどを渡すと「より後」なので翌日。
    assert_eq!(
        next("30 4 * * *", &tokyo, "2026-10-02T19:30:00Z"),
        Some(t("2026-10-03T19:30:00Z"))
    );
    // 秒の端数は切り捨てて次の分から探す。
    assert_eq!(
        next("30 4 * * *", &tokyo, "2026-10-02T19:29:59.5Z"),
        Some(t("2026-10-02T19:30:00Z"))
    );
}

#[test]
fn cron_job_unknown_time_zone_is_rejected() {
    let err = CronTz::iana("Mars/Olympus_Mons").unwrap_err();
    assert!(matches!(err, CronError::TimeZone { .. }), "{err:?}");
}

#[test]
fn cron_job_next_after_month_end_and_leap_day() {
    let tz = utc();
    // 31 日の無い月は飛ばす。
    assert_eq!(
        series("0 0 31 * *", &tz, "2026-01-31T00:00:00Z", 3),
        vec![
            t("2026-03-31T00:00:00Z"),
            t("2026-05-31T00:00:00Z"),
            t("2026-07-31T00:00:00Z"),
        ]
    );
    // 2/29 は次の閏年。
    assert_eq!(
        next("0 12 29 2 *", &tz, "2026-03-01T00:00:00Z"),
        Some(t("2028-02-29T12:00:00Z"))
    );
    // 年末の桁上げ。
    assert_eq!(
        next("0 0 1 1 *", &tz, "2026-12-31T23:59:00Z"),
        Some(t("2027-01-01T00:00:00Z"))
    );
    // 月末の時・分の桁上げ（30 日 23:59 の次は翌月 1 日）。
    assert_eq!(
        next("*/30 * * * *", &tz, "2026-09-30T23:45:00Z"),
        Some(t("2026-10-01T00:00:00Z"))
    );
}

#[test]
fn cron_job_day_of_month_or_weekday_like_vixie() {
    let tz = utc();
    // 日と曜日が両方とも制限: 13 日 または 金曜。2026-10-02 は金、10-13 は火。
    assert_eq!(
        series("0 9 13 * 5", &tz, "2026-10-01T00:00:00Z", 3),
        vec![
            t("2026-10-02T09:00:00Z"),
            t("2026-10-09T09:00:00Z"),
            t("2026-10-13T09:00:00Z"),
        ]
    );
    // 日が `*` なら曜日だけ。
    assert_eq!(
        series("0 9 * * fri", &tz, "2026-10-01T00:00:00Z", 2),
        vec![t("2026-10-02T09:00:00Z"), t("2026-10-09T09:00:00Z")]
    );
    // `*/n` で始まる欄は `*` 扱い（AND）: 奇数日（1,3,..）かつ金曜。10-02 は偶数日、10-09 は奇数日の金曜。
    assert_eq!(
        next("0 9 */2 * 5", &tz, "2026-10-01T00:00:00Z"),
        Some(t("2026-10-09T09:00:00Z"))
    );
}

#[test]
fn cron_job_dst_spring_forward_fires_once_shifted() {
    let tz = eastern();
    // 2026-03-08 の 02:30 は存在しない → 03:30 EDT（07:30Z）に 1 回。前後の日は 02:30 そのまま。
    assert_eq!(
        series("30 2 * * *", &tz, "2026-03-06T12:00:00Z", 3),
        vec![
            t("2026-03-07T07:30:00Z"), // 02:30 EST
            t("2026-03-08T07:30:00Z"), // 存在しない 02:30 → 03:30 EDT
            t("2026-03-09T06:30:00Z"), // 02:30 EDT
        ]
    );
    // 飛びの中の 02:00 と 02:30 は 1 回にまとまり、03:00 EDT の後は 03:30 EDT。
    assert_eq!(
        series("*/30 * * * *", &tz, "2026-03-08T06:15:00Z", 3),
        vec![
            t("2026-03-08T06:30:00Z"), // 01:30 EST
            t("2026-03-08T07:00:00Z"), // 存在しない 02:00 → 03:00 EDT
            t("2026-03-08T07:30:00Z"), // 03:30 EDT
        ]
    );
}

#[test]
fn cron_job_dst_fall_back_fires_once_on_earlier_offset() {
    let tz = eastern();
    // 2026-11-01 の 01:30 は 2 回ある → 早い方（EDT, 05:30Z）だけ。遅い方（EST, 06:30Z）は拾わない。
    assert_eq!(
        series("30 1 * * *", &tz, "2026-10-31T12:00:00Z", 3),
        vec![
            t("2026-11-01T05:30:00Z"), // 01:30 EDT
            t("2026-11-02T06:30:00Z"), // 01:30 EST
            t("2026-11-03T06:30:00Z"),
        ]
    );
    // 戻った後半（01:10 EST）から探しても、同じ日の 01:30 は既に過ぎている。
    assert_eq!(
        next("30 1 * * *", &tz, "2026-11-01T06:10:00Z"),
        Some(t("2026-11-02T06:30:00Z"))
    );
    // 毎時は壁時計の各時に 1 回（UTC では 01:00 EDT → 02:00 EST が 2 時間空く）。
    assert_eq!(
        series("0 * * * *", &tz, "2026-11-01T04:30:00Z", 3),
        vec![
            t("2026-11-01T05:00:00Z"), // 01:00 EDT
            t("2026-11-01T07:00:00Z"), // 02:00 EST
            t("2026-11-01T08:00:00Z"), // 03:00 EST
        ]
    );
}

#[test]
fn cron_job_due_fires_counts_missed_and_picks_latest() {
    let tz = utc();
    let s = sched("0 * * * *");
    // まだ時刻ではない。
    assert_eq!(
        due_fires(
            &s,
            &tz,
            t("2026-10-02T06:00:00Z"),
            t("2026-10-02T05:59:00Z")
        )
        .unwrap(),
        None
    );
    // 00:00〜05:00 の 6 回が過ぎた。
    let due = due_fires(
        &s,
        &tz,
        t("2026-10-02T00:00:00Z"),
        t("2026-10-02T05:30:00Z"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(due.earliest, t("2026-10-02T00:00:00Z"));
    assert_eq!(due.latest, t("2026-10-02T05:00:00Z"));
    assert_eq!(due.count, 6);
    assert!(!due.count_capped);
    assert_eq!(due.next, Some(t("2026-10-02T06:00:00Z")));
    // 予定時刻ちょうどの now は含む（通常運転の 1 件）。
    let one = due_fires(
        &s,
        &tz,
        t("2026-10-02T06:00:00Z"),
        t("2026-10-02T06:00:00Z"),
    )
    .unwrap()
    .unwrap();
    assert_eq!((one.count, one.latest), (1, t("2026-10-02T06:00:00Z")));
    assert_eq!(one.next, Some(t("2026-10-02T07:00:00Z")));
}

#[test]
fn cron_job_due_fires_caps_count_but_finds_latest() {
    let tz = utc();
    let s = sched("* * * * *");
    // 2 日分（2881 回）過ぎても数えるのは 1000 回まで。最新は now ちょうど。
    let due = due_fires(
        &s,
        &tz,
        t("2026-10-01T00:00:00Z"),
        t("2026-10-03T00:00:00Z"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(due.count, MISSED_COUNT_CAP);
    assert!(due.count_capped);
    assert_eq!(due.latest, t("2026-10-03T00:00:00Z"));
    assert_eq!(due.next, Some(t("2026-10-03T00:01:00Z")));
}

#[test]
fn cron_job_render_title_uses_job_time_zone_date() {
    let tokyo = CronTz::iana("Asia/Tokyo").unwrap();
    assert_eq!(
        render_title("日次整理: {date}", &tokyo, t("2026-10-02T19:30:00Z")).unwrap(),
        "日次整理: 2026-10-03"
    );
    assert_eq!(
        render_title("no date", &tokyo, t("2026-10-02T19:30:00Z")).unwrap(),
        "no date"
    );
}

#[test]
fn cron_job_template_keeps_extra_fields_and_lane() {
    let toml_src = r#"
title = "日次整理: {date}"
harness = "knowledge-curation"
lane = "cheap"
project = "agent-platform"
mode = "dry_run"
"#;
    let tpl: CronTaskTemplate = toml::from_str(toml_src).unwrap();
    assert_eq!(tpl.lane, Some(Tier::Cheap));
    assert_eq!(tpl.harness.as_deref(), Some("knowledge-curation"));
    assert_eq!(tpl.extra.get("mode"), Some(&serde_json::json!("dry_run")));
    let json = serde_json::to_string(&tpl).unwrap();
    let back: CronTaskTemplate = serde_json::from_str(&json).unwrap();
    assert_eq!(back, tpl);
}

#[test]
fn cron_job_enum_strings_round_trip() {
    for o in [CronOverlap::Skip, CronOverlap::Queue] {
        assert_eq!(o.as_str().parse::<CronOverlap>().unwrap(), o);
    }
    for c in [CronCatchUp::Latest, CronCatchUp::Skip] {
        assert_eq!(c.as_str().parse::<CronCatchUp>().unwrap(), c);
    }
    for tr in [
        CronTrigger::Schedule,
        CronTrigger::CatchUp,
        CronTrigger::Manual,
    ] {
        assert_eq!(tr.as_str().parse::<CronTrigger>().unwrap(), tr);
    }
    for out in [
        CronRunOutcome::Created,
        CronRunOutcome::Queued,
        CronRunOutcome::SkippedOverlap,
        CronRunOutcome::SkippedMissed,
        CronRunOutcome::Error,
    ] {
        assert_eq!(out.as_str().parse::<CronRunOutcome>().unwrap(), out);
    }
    assert!("both".parse::<CronOverlap>().is_err());
}
