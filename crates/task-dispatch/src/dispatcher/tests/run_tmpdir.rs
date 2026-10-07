//! ADR 2026-10-07-build-tmp-hygiene D2: ローカルの run は `runs/<run_id>/tmp` を `TMPDIR`・`TMP`・`TEMP` に
//! 受け取り、run の終わり方（成功・失敗）に依らずその dir は消える（`runs/<run_id>/` 自体は残る）。

use super::*;

#[derive(Debug, Clone, Default)]
struct Seen {
    run_id: String,
    workspace: PathBuf,
    env: Vec<(String, String)>,
    tmp_existed_during_run: bool,
}

#[derive(Clone)]
struct TmpProbeAdapter {
    env: Vec<(String, String)>,
    fail: bool,
    seen: Arc<StdMutex<Vec<Seen>>>,
}

#[async_trait]
impl WorkerAdapter for TmpProbeAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let tmp = self
            .env
            .iter()
            .find(|(k, _)| k == "TMPDIR")
            .map(|(_, v)| PathBuf::from(v));
        let existed = tmp.as_deref().is_some_and(Path::is_dir);
        // worker が置く写しの代わり（終了で消えることを確かめる）。
        if let Some(dir) = &tmp {
            std::fs::create_dir_all(dir.join("rw/.git")).unwrap();
            std::fs::write(dir.join("rw/big.bin"), b"x").unwrap();
        }
        self.seen.lock().unwrap().push(Seen {
            run_id: run_id.to_string(),
            workspace: req.workspace.clone(),
            env: self.env.clone(),
            tmp_existed_during_run: existed,
        });
        if self.fail {
            return Err(AdapterError::Other("boom".into()));
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut env = self.env.clone();
        env.extend(extra.iter().cloned());
        Some(Arc::new(TmpProbeAdapter {
            env,
            ..self.clone()
        }))
    }
}

/// 1 run 目が終わる（adapter が返り、run の終端処理が済む）まで tick する。
async fn run_one(fail: bool) -> (Seen, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(TmpProbeAdapter {
        env: Vec::new(),
        fail,
        seen: seen.clone(),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        d.tick().unwrap();
        let task_now = store.get(task.id).unwrap().expect("task");
        let ran = !seen.lock().unwrap().is_empty();
        // 失敗した run は retry されうるので、run が 1 回走って lease を手放した（running でない）時点で見る。
        if ran && task_now.status != Status::Running {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "run did not finish: {task_now:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let first = seen.lock().unwrap()[0].clone();
    (first, dir)
}

fn expected_tmp(seen: &Seen) -> PathBuf {
    seen.workspace.join("runs").join(&seen.run_id).join("tmp")
}

#[tokio::test]
async fn run_tmpdir_env_is_passed_to_a_local_run() {
    let (seen, _dir) = run_one(false).await;
    let expected = expected_tmp(&seen).display().to_string();
    for key in ["TMPDIR", "TMP", "TEMP"] {
        assert!(
            seen.env.contains(&(key.to_string(), expected.clone())),
            "{key} not set to {expected}: {:?}",
            seen.env
        );
    }
    assert!(seen.tmp_existed_during_run, "{seen:?}");
}

#[tokio::test]
async fn run_tmpdir_removed_when_the_run_succeeds() {
    let (seen, _dir) = run_one(false).await;
    let tmp = expected_tmp(&seen);
    assert!(seen.tmp_existed_during_run, "{seen:?}");
    assert!(!tmp.exists(), "{} still exists", tmp.display());
    assert!(
        tmp.parent().expect("run dir").is_dir(),
        "run records must stay"
    );
}

#[tokio::test]
async fn run_tmpdir_removed_when_the_run_fails() {
    let (seen, _dir) = run_one(true).await;
    let tmp = expected_tmp(&seen);
    assert!(seen.tmp_existed_during_run, "{seen:?}");
    assert!(!tmp.exists(), "{} still exists", tmp.display());
}
