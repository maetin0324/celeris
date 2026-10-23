//! Backend tripwire: observe the saved wait, never stop inside wait_started.
use super::*;
use celeris_campaign::dagbench::ext::{ExtBackend, Lookup, Notice, SubmitError, SubmitReply};
use std::{collections::BTreeSet, io, io::Write};
use task_core::cluster_job::ClusterJobWaitStore;

pub fn saved(p: &Params, physical: &str) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    if p.arm == "baseline" {
        let state: Value = serde_json::from_slice(&fs::read(p.out.join("state.json"))?)?;
        Ok(state["nodes"].as_object().and_then(|nodes| {
            nodes.iter().find_map(|(id, n)| {
                (n["status"] == "waiting" && n["physical_id"] == physical).then(|| {
                    json!({"wait_id": id, "job_id": physical, "state": n, "source": "state.json"})
                })
            })
        }))
    } else {
        let store = task_core::SqliteStore::open(&p.out.join("celeris.db"))?;
        Ok(store.cluster_job_waits_waiting()?.into_iter()
            .find(|w| w.jobs.iter().any(|j| j == physical.trim_start_matches("phys-")))
            .map(|w| json!({"wait_id": w.wait_id, "job_id": physical, "state": w, "source": "TaskStore"})))
    }
}

pub struct DurableBackend {
    pub inner: IpcExt,
    pub params: Params,
    pub nth: Option<usize>,
    pub seen: BTreeSet<String>,
    pub selected: Option<(String, String)>,
}
impl DurableBackend {
    fn boundary(&self) -> io::Result<()> {
        if let Some((logical, physical)) = &self.selected
            && let Some(snapshot) =
                saved(&self.params, physical).map_err(|e| io::Error::other(e.to_string()))?
        {
            println!(
                "DURABLE {}",
                json!({"logical_job_id": logical, "physical_id": physical, "saved": snapshot})
            );
            io::stdout().flush()?;
            loop {
                std::thread::park();
            }
        }
        Ok(())
    }
}
impl ExtBackend for DurableBackend {
    fn submit(&mut self, l: &str, s: u64) -> io::Result<Result<SubmitReply, SubmitError>> {
        self.boundary()?;
        self.inner.submit(l, s)
    }
    fn wait_started(&mut self, l: &str) -> io::Result<()> {
        self.inner.wait_started(l)?;
        self.seen.insert(l.into());
        if self.nth == Some(self.seen.len()) && self.selected.is_none() {
            if let Lookup::Known { physical_id, .. } = self.inner.lookup(l)? {
                self.selected = Some((l.into(), physical_id));
            }
        }
        Ok(())
    }
    fn lookup(&mut self, l: &str) -> io::Result<Lookup> {
        self.boundary()?;
        self.inner.lookup(l)
    }
    fn advance(&mut self, n: u64) -> io::Result<Vec<Notice>> {
        self.boundary()?;
        self.inner.advance(n)
    }
    fn record_consumed(&mut self, l: &str) -> io::Result<bool> {
        self.boundary()?;
        self.inner.record_consumed(l)
    }
    fn now(&mut self) -> io::Result<u64> {
        self.inner.now()
    }
    fn due_secs(&mut self, l: &str) -> io::Result<Option<u64>> {
        self.inner.due_secs(l)
    }
}
