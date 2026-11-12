//! ADR 2026-11-11-cos-fallback-timing: where CoS's first response to one inbox item or root-done
//! summary stands (D1・D4), and why the daemon did not start a CoS run for a thread with pending
//! items (D3). Pure: the dispatcher reads the times from SQLite and acts on the result.

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// An RFC 3339 time of SQLite (`chat_runs.started_at`, `cos_inbox_items.created_at`).
pub fn parse_ts(at: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(at, &Rfc3339).ok()
}

/// One CoS run of the item's thread, as the wait of a queued input needs it.
#[derive(Debug, Clone, Copy)]
pub struct ThreadRun<'a> {
    /// `queued`・`running`・`stopping`.
    pub live: bool,
    pub started_at: Option<&'a str>,
    pub finished_at: Option<&'a str>,
}

/// Where CoS's first response to one item stands (D1). The safety net counts from its time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstResponseWait {
    /// The item's own CoS run is live since this time.
    OwnRun(OffsetDateTime),
    /// The item waits behind another live run of its thread, live since this time.
    BehindRun(OffsetDateTime),
    /// No run of its thread is live: the item has been pickable since this time.
    Pickable(OffsetDateTime),
}

impl FirstResponseWait {
    /// The wait of an item that arrived at `arrived` with no live run of its own: behind the
    /// thread's live run if any, else pickable since its arrival or the end of the thread's last
    /// run, whichever is later.
    pub fn queued(
        arrived: OffsetDateTime,
        thread_run_started: Option<OffsetDateTime>,
        thread_idle_since: Option<OffsetDateTime>,
    ) -> Self {
        match thread_run_started {
            Some(started) => Self::BehindRun(started),
            None => Self::Pickable(thread_idle_since.map_or(arrived, |idle| idle.max(arrived))),
        }
    }

    /// [`Self::queued`] from the runs of the thread (the earliest live run, the last end).
    pub fn queued_behind<'a>(
        arrived: OffsetDateTime,
        runs: impl IntoIterator<Item = ThreadRun<'a>>,
    ) -> Self {
        let (mut live, mut idle) = (None::<OffsetDateTime>, None::<OffsetDateTime>);
        for run in runs {
            if run.live {
                let started = run.started_at.and_then(parse_ts).unwrap_or(arrived);
                live = Some(live.map_or(started, |l| l.min(started)));
            }
            if let Some(finished) = run.finished_at.and_then(parse_ts) {
                idle = Some(idle.map_or(finished, |i| i.max(finished)));
            }
        }
        Self::queued(arrived, live, idle)
    }

    /// Why the safety net `limit_secs` has run out, or `None` while CoS may still answer.
    pub fn overdue(self, limit_secs: u64, now: OffsetDateTime) -> Option<String> {
        let limit = i64::try_from(limit_secs).unwrap_or(i64::MAX);
        let (Self::OwnRun(since) | Self::BehindRun(since) | Self::Pickable(since)) = self;
        let n = (now - since).whole_seconds();
        if n <= limit {
            return None;
        }
        Some(match self {
            Self::OwnRun(_) => {
                format!("CoS の一次対応が {limit} 秒を超えて終わらない（run の経過 {n} 秒）")
            }
            Self::BehindRun(_) => format!(
                "CoS の前の run が {limit} 秒を超えて終わらず、一次対応が始まらない（run の経過 {n} 秒）"
            ),
            Self::Pickable(_) => format!(
                "CoS の一次対応が {limit} 秒を超えても始まらない（容量待ち・停止を含む。拾える状態になってから {n} 秒）"
            ),
        })
    }
}

/// Why the daemon did not start a CoS run for a thread with pending items (D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageSkip {
    /// The daemon does not take new work (standby, draining).
    NotAccepting,
    /// The disk guard holds new work.
    DiskNotReady,
    /// `cos.enabled=false` (the fallback routes the items at once).
    Disabled,
    /// The KB's CoS skills do not fit `[cos.triage]` (ADR 2026-11-11-cos-skill-sync D3).
    SkillDrift,
    /// This process runs the thread's CoS run; the items are claimed after it.
    ThreadRunning,
    /// Every CoS slot is in use.
    NoCapacity,
    /// SQLite has a live run of the thread that this process does not own.
    RunInDb,
}

impl TriageSkip {
    /// The daemon-wide reason no CoS run may start now, checked before any thread.
    pub fn gate(
        accepting_new_work: bool,
        disk_ready: bool,
        cos_enabled: bool,
        skills_ready: bool,
    ) -> Option<Self> {
        [
            (accepting_new_work, Self::NotAccepting),
            (disk_ready, Self::DiskNotReady),
            (cos_enabled, Self::Disabled),
            (skills_ready, Self::SkillDrift),
        ]
        .into_iter()
        .find_map(|(ok, skip)| (!ok).then_some(skip))
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotAccepting => "not_accepting_new_work",
            Self::DiskNotReady => "disk_not_ready",
            Self::Disabled => "cos_disabled",
            Self::SkillDrift => "cos_skill_drift",
            Self::ThreadRunning => "thread_run_live",
            Self::NoCapacity => "no_cos_slot",
            Self::RunInDb => "thread_run_live_in_db",
        }
    }
}
