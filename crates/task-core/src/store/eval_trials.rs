//! `eval_trials` の読み書き（ADR 2026-10-11-llm-source-model-routing-evaluation 付記 2026-10-12）。
//!
//! 状態遷移は条件付き UPDATE で守る（変わった行が 0 なら `StoreError::Invalid`）。二重 claim しない。
use rusqlite::{OptionalExtension, Row, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::routing_eval_trial::{EvalTrial, EvalTrialSpec, EvalTrialStatus};

const COLUMNS: &str = "trial_id,batch_id,manifest_sha,problem_id,trial_no,source_id,model_id,\
harness_id,harness_version,repo,base_commit,objective,check_cmd,check_timeout_secs,status,run_id,\
decision_id,worktree_path,check_exit,timed_out,error,created_at,started_at,finished_at";

fn now() -> Result<String, StoreError> {
    format_rfc3339(OffsetDateTime::now_utc())
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<EvalTrial> {
    let status: String = row.get(14)?;
    let status = EvalTrialStatus::parse(&status).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            14,
            rusqlite::types::Type::Text,
            format!("unknown eval trial status {status}").into(),
        )
    })?;
    Ok(EvalTrial {
        trial_id: row.get(0)?,
        batch_id: row.get(1)?,
        manifest_sha: row.get(2)?,
        problem_id: row.get(3)?,
        trial_no: row.get(4)?,
        source_id: row.get(5)?,
        model_id: row.get(6)?,
        harness_id: row.get(7)?,
        harness_version: row.get(8)?,
        repo: row.get(9)?,
        base_commit: row.get(10)?,
        objective: row.get(11)?,
        check_cmd: row.get(12)?,
        check_timeout_secs: row.get(13)?,
        status,
        run_id: row.get(15)?,
        decision_id: row.get(16)?,
        worktree_path: row.get(17)?,
        check_exit: row.get(18)?,
        timed_out: row.get(19)?,
        error: row.get(20)?,
        created_at: row.get(21)?,
        started_at: row.get(22)?,
        finished_at: row.get(23)?,
    })
}

fn get_on(conn: &rusqlite::Connection, trial_id: &str) -> Result<Option<EvalTrial>, StoreError> {
    let sql = format!("SELECT {COLUMNS} FROM eval_trials WHERE trial_id = ?1");
    Ok(conn.query_row(&sql, [trial_id], read_row).optional()?)
}

/// 条件付き UPDATE が 1 行も変えなかったときの誤り（行が無いか、状態が合わない）。
fn transition_error(
    conn: &rusqlite::Connection,
    trial_id: &str,
    action: &str,
) -> Result<(), StoreError> {
    match get_on(conn, trial_id)? {
        None => Err(StoreError::Invalid(format!(
            "eval trial {trial_id} not found"
        ))),
        Some(t) => Err(StoreError::Invalid(format!(
            "eval trial {trial_id}: cannot {action} from status {}",
            t.status.as_str()
        ))),
    }
}

impl SqliteStore {
    /// 試行を投入する。同じ試行（manifest_sha・problem_id・trial_no・source・model・harness）が
    /// 既にあれば既存行を返す（冪等。batch_id も既存のまま）。返す順は `specs` の順。
    pub fn insert_eval_trials(
        &self,
        batch_id: &str,
        specs: &[EvalTrialSpec],
    ) -> Result<Vec<EvalTrial>, StoreError> {
        let created_at = now()?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let mut out = Vec::with_capacity(specs.len());
        for spec in specs {
            if spec.check_timeout_secs == 0 {
                return Err(StoreError::Invalid(
                    "eval trial check_timeout_secs must be positive".into(),
                ));
            }
            tx.execute(
                "INSERT INTO eval_trials(trial_id,batch_id,manifest_sha,problem_id,trial_no,source_id,\
                 model_id,harness_id,repo,base_commit,objective,check_cmd,check_timeout_secs,status,\
                 created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'queued',?14) \
                 ON CONFLICT(manifest_sha,problem_id,trial_no,source_id,model_id,harness_id) DO NOTHING",
                params![
                    ulid::Ulid::new().to_string(),
                    batch_id,
                    spec.manifest_sha,
                    spec.problem_id,
                    spec.trial_no,
                    spec.source_id,
                    spec.model_id,
                    spec.harness_id,
                    spec.repo,
                    spec.base_commit,
                    spec.objective,
                    spec.check_cmd,
                    spec.check_timeout_secs,
                    created_at,
                ],
            )?;
            let sql = format!(
                "SELECT {COLUMNS} FROM eval_trials WHERE manifest_sha=?1 AND problem_id=?2 \
                 AND trial_no=?3 AND source_id=?4 AND model_id=?5 AND harness_id=?6"
            );
            let row = tx.query_row(
                &sql,
                params![
                    spec.manifest_sha,
                    spec.problem_id,
                    spec.trial_no,
                    spec.source_id,
                    spec.model_id,
                    spec.harness_id
                ],
                read_row,
            )?;
            out.push(row);
        }
        tx.commit()?;
        Ok(out)
    }

    /// 試行の一覧（投入順）。`batch_id`・`status` で絞れる。
    pub fn eval_trials_list(
        &self,
        batch_id: Option<&str>,
        status: Option<EvalTrialStatus>,
    ) -> Result<Vec<EvalTrial>, StoreError> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {COLUMNS} FROM eval_trials WHERE (?1 IS NULL OR batch_id = ?1) \
             AND (?2 IS NULL OR status = ?2) ORDER BY created_at, rowid"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![batch_id, status.map(|s| s.as_str())], read_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn eval_trial_get(&self, trial_id: &str) -> Result<Option<EvalTrial>, StoreError> {
        let conn = self.lock()?;
        get_on(&conn, trial_id)
    }

    /// `source_id` が `skip_sources` に無い `queued` の最古を `running` にして返す。条件付き UPDATE
    /// なので、並行して呼んでも同じ行を 2 回返さない。無ければ `None`。
    ///
    /// `skip_sources` は tick の中で起こせなかった（account・quota が埋まった）source。同じ tick で
    /// その source の試行を見直さずに、後ろの別 source の試行へ進む（先頭詰まりを避ける）。source の
    /// 中の順（作成順）は保つ。`idx_eval_trials_status` を順に辿り最初の 1 行で止まるので、queued が
    /// 多くても全件は読まない（飛ばすのは埋まった source の行だけ）。
    pub fn eval_trial_claim_next(
        &self,
        skip_sources: &[String],
    ) -> Result<Option<EvalTrial>, StoreError> {
        let skip = serde_json::to_string(skip_sources)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let candidate: Option<String> = tx
            .query_row(
                "SELECT trial_id FROM eval_trials WHERE status = 'queued' \
                 AND source_id NOT IN (SELECT value FROM json_each(?1)) \
                 ORDER BY created_at, rowid LIMIT 1",
                [&skip],
                |r| r.get(0),
            )
            .optional()?;
        let Some(trial_id) = candidate else {
            return Ok(None);
        };
        let changed = tx.execute(
            "UPDATE eval_trials SET status = 'running' WHERE trial_id = ?1 AND status = 'queued'",
            [&trial_id],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let trial = get_on(&tx, &trial_id)?;
        tx.commit()?;
        Ok(trial)
    }

    /// claim したが admission で起こせなかった試行を `queued` に戻す（run がまだ無いときだけ）。
    pub fn eval_trial_unclaim(&self, trial_id: &str) -> Result<(), StoreError> {
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE eval_trials SET status = 'queued' \
             WHERE trial_id = ?1 AND status = 'running' AND run_id IS NULL",
            [trial_id],
        )?;
        if changed == 0 {
            return transition_error(&conn, trial_id, "unclaim");
        }
        Ok(())
    }

    /// 起動した run を記録する（`running` かつ run 未記録のときだけ）。
    pub fn eval_trial_mark_started(
        &self,
        trial_id: &str,
        run_id: &str,
        decision_id: &str,
        harness_version: &str,
        worktree_path: &str,
    ) -> Result<(), StoreError> {
        let started_at = now()?;
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE eval_trials SET run_id = ?2, decision_id = ?3, harness_version = ?4, \
             worktree_path = ?5, started_at = ?6 \
             WHERE trial_id = ?1 AND status = 'running' AND run_id IS NULL",
            params![
                trial_id,
                run_id,
                decision_id,
                harness_version,
                worktree_path,
                started_at
            ],
        )?;
        if changed == 0 {
            return transition_error(&conn, trial_id, "mark started");
        }
        Ok(())
    }

    /// worker が終わった試行を採点中にする（`running` かつ run 記録済みのときだけ）。
    pub fn eval_trial_mark_scoring(&self, trial_id: &str) -> Result<(), StoreError> {
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE eval_trials SET status = 'scoring' \
             WHERE trial_id = ?1 AND status = 'running' AND run_id IS NOT NULL",
            [trial_id],
        )?;
        if changed == 0 {
            return transition_error(&conn, trial_id, "mark scoring");
        }
        Ok(())
    }

    /// 試行を終える。`passed`/`failed` は `scoring` からだけ、`error` は `running`/`scoring` から。
    pub fn eval_trial_finish(
        &self,
        trial_id: &str,
        status: EvalTrialStatus,
        check_exit: Option<i32>,
        timed_out: bool,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        let from: &[&str] = match status {
            EvalTrialStatus::Passed | EvalTrialStatus::Failed => &["scoring"],
            EvalTrialStatus::Error => &["running", "scoring"],
            other => {
                return Err(StoreError::Invalid(format!(
                    "eval trial {trial_id}: finish requires passed|failed|error, got {}",
                    other.as_str()
                )));
            }
        };
        let finished_at = now()?;
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE eval_trials SET status = ?2, check_exit = ?3, timed_out = ?4, error = ?5, \
             finished_at = ?6 WHERE trial_id = ?1 AND status IN (?7, ?8)",
            params![
                trial_id,
                status.as_str(),
                check_exit,
                timed_out,
                error,
                finished_at,
                from[0],
                from.get(1).copied().unwrap_or(from[0]),
            ],
        )?;
        if changed == 0 {
            return transition_error(&conn, trial_id, "finish");
        }
        Ok(())
    }

    /// 試行を取り消す（`queued`/`running` のときだけ）。`running` の run を止めるのは呼び出し側。
    pub fn eval_trial_cancel(&self, trial_id: &str) -> Result<EvalTrial, StoreError> {
        let finished_at = now()?;
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE eval_trials SET status = 'cancelled', finished_at = ?2 \
             WHERE trial_id = ?1 AND status IN ('queued', 'running')",
            params![trial_id, finished_at],
        )?;
        if changed == 0 {
            transition_error(&conn, trial_id, "cancel")?;
        }
        get_on(&conn, trial_id)?
            .ok_or_else(|| StoreError::Invalid(format!("eval trial {trial_id} not found")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(problem: &str, trial_no: u32) -> EvalTrialSpec {
        EvalTrialSpec {
            manifest_sha: "sha-1".into(),
            problem_id: problem.into(),
            trial_no,
            source_id: "codex-oauth".into(),
            model_id: "gpt-x".into(),
            harness_id: "codex".into(),
            repo: "/srv/repo".into(),
            base_commit: "abc123".into(),
            objective: "fix the bug".into(),
            check_cmd: "sh notes/check.sh".into(),
            check_timeout_secs: 60,
        }
    }

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().expect("open store")
    }

    #[test]
    fn insert_is_idempotent_per_trial_key() {
        let s = store();
        let first = s
            .insert_eval_trials("b1", &[spec("p1", 0), spec("p1", 1)])
            .expect("insert");
        assert_eq!(first.len(), 2);
        assert!(first.iter().all(|t| t.status == EvalTrialStatus::Queued));
        assert_ne!(first[0].trial_id, first[1].trial_id);
        let again = s
            .insert_eval_trials("b2", &[spec("p1", 1), spec("p2", 0)])
            .expect("reinsert");
        assert_eq!(again[0], first[1], "same trial returns the existing row");
        assert_eq!(again[1].batch_id, "b2");
        assert_eq!(s.eval_trials_list(None, None).expect("list").len(), 3);
        assert_eq!(s.eval_trials_list(Some("b1"), None).expect("list").len(), 2);
        assert_eq!(
            s.eval_trials_list(None, Some(EvalTrialStatus::Running))
                .expect("list")
                .len(),
            0
        );
        let mut bad = spec("p3", 0);
        bad.check_timeout_secs = 0;
        assert!(s.insert_eval_trials("b3", &[bad]).is_err());
    }

    #[test]
    fn claim_takes_oldest_once_and_unclaim_requeues() {
        let s = store();
        let rows = s
            .insert_eval_trials("b1", &[spec("p1", 0), spec("p1", 1)])
            .expect("insert");
        let a = s.eval_trial_claim_next(&[]).expect("claim").expect("some");
        assert_eq!(a.trial_id, rows[0].trial_id);
        assert_eq!(a.status, EvalTrialStatus::Running);
        let b = s.eval_trial_claim_next(&[]).expect("claim").expect("some");
        assert_eq!(b.trial_id, rows[1].trial_id);
        assert!(s.eval_trial_claim_next(&[]).expect("claim").is_none());
        s.eval_trial_unclaim(&a.trial_id).expect("unclaim");
        assert!(s.eval_trial_unclaim(&a.trial_id).is_err(), "already queued");
        let again = s.eval_trial_claim_next(&[]).expect("claim").expect("some");
        assert_eq!(again.trial_id, a.trial_id);
    }

    #[test]
    fn claim_skips_the_given_sources_and_keeps_their_order() {
        let s = store();
        let claude = |n| EvalTrialSpec {
            source_id: "claude-oauth".into(),
            ..spec("p1", n)
        };
        let rows = s
            .insert_eval_trials("b1", &[claude(0), claude(1), spec("p1", 0)])
            .expect("insert");
        let skip = vec!["claude-oauth".to_string()];
        let codex = s
            .eval_trial_claim_next(&skip)
            .expect("claim")
            .expect("some");
        assert_eq!(
            codex.trial_id, rows[2].trial_id,
            "the trial behind the skipped source"
        );
        assert!(s.eval_trial_claim_next(&skip).expect("claim").is_none());
        let first = s.eval_trial_claim_next(&[]).expect("claim").expect("some");
        assert_eq!(
            first.trial_id, rows[0].trial_id,
            "the skipped source keeps its order"
        );
    }

    #[test]
    fn concurrent_claims_never_return_the_same_trial() {
        let s = std::sync::Arc::new(store());
        let specs: Vec<_> = (0..8).map(|n| spec("p1", n)).collect();
        s.insert_eval_trials("b1", &specs).expect("insert");
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let s = s.clone();
                std::thread::spawn(move || {
                    let mut got = Vec::new();
                    while let Some(t) = s.eval_trial_claim_next(&[]).expect("claim") {
                        got.push(t.trial_id);
                    }
                    got
                })
            })
            .collect();
        let mut all: Vec<String> = handles
            .into_iter()
            .flat_map(|h| h.join().expect("join"))
            .collect();
        all.sort();
        let n = all.len();
        all.dedup();
        assert_eq!(n, 8);
        assert_eq!(all.len(), 8);
    }

    #[test]
    fn lifecycle_records_run_and_score() {
        let s = store();
        let id = s
            .insert_eval_trials("b1", &[spec("p1", 0)])
            .expect("insert")[0]
            .trial_id
            .clone();
        assert!(
            s.eval_trial_mark_scoring(&id).is_err(),
            "queued cannot score"
        );
        s.eval_trial_claim_next(&[]).expect("claim");
        assert!(s.eval_trial_mark_scoring(&id).is_err(), "no run yet");
        s.eval_trial_mark_started(&id, "run-1", "dec-1", "2", "/data/eval/x/repo")
            .expect("started");
        assert!(
            s.eval_trial_mark_started(&id, "run-2", "dec-2", "2", "/x")
                .is_err(),
            "run is recorded once"
        );
        assert!(s.eval_trial_unclaim(&id).is_err(), "started cannot requeue");
        assert!(
            s.eval_trial_finish(&id, EvalTrialStatus::Passed, Some(0), false, None)
                .is_err(),
            "passed needs scoring"
        );
        s.eval_trial_mark_scoring(&id).expect("scoring");
        s.eval_trial_finish(&id, EvalTrialStatus::Failed, Some(1), true, None)
            .expect("finish");
        let t = s.eval_trial_get(&id).expect("get").expect("row");
        assert_eq!(t.status, EvalTrialStatus::Failed);
        assert_eq!(t.run_id.as_deref(), Some("run-1"));
        assert_eq!(t.decision_id.as_deref(), Some("dec-1"));
        assert_eq!(t.harness_version.as_deref(), Some("2"));
        assert_eq!(t.check_exit, Some(1));
        assert!(t.timed_out);
        assert!(t.started_at.is_some() && t.finished_at.is_some());
        assert!(s.eval_trial_cancel(&id).is_err(), "terminal cannot cancel");
        assert!(
            s.eval_trial_finish(&id, EvalTrialStatus::Error, None, false, Some("x"))
                .is_err()
        );
        assert!(s.eval_trial_get("missing").expect("get").is_none());
    }

    #[test]
    fn error_from_running_and_cancel_from_queued_or_running() {
        let s = store();
        let rows = s
            .insert_eval_trials("b1", &[spec("p1", 0), spec("p1", 1), spec("p1", 2)])
            .expect("insert");
        s.eval_trial_claim_next(&[]).expect("claim");
        s.eval_trial_finish(
            &rows[0].trial_id,
            EvalTrialStatus::Error,
            None,
            false,
            Some("run vanished after restart"),
        )
        .expect("error");
        assert_eq!(
            s.eval_trial_get(&rows[0].trial_id)
                .expect("get")
                .expect("row")
                .error
                .as_deref(),
            Some("run vanished after restart")
        );
        assert!(
            s.eval_trial_finish(
                &rows[1].trial_id,
                EvalTrialStatus::Queued,
                None,
                false,
                None
            )
            .is_err()
        );
        let c = s
            .eval_trial_cancel(&rows[1].trial_id)
            .expect("cancel queued");
        assert_eq!(c.status, EvalTrialStatus::Cancelled);
        s.eval_trial_claim_next(&[]).expect("claim");
        s.eval_trial_cancel(&rows[2].trial_id)
            .expect("cancel running");
        assert!(s.eval_trial_claim_next(&[]).expect("claim").is_none());
        assert!(s.eval_trial_cancel("missing").is_err());
    }
}
