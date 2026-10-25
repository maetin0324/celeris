//! 評価側の本番 scheduler 照会（`observe::SchedulerTransport` の read-only 実装）。子 process（ssh）を起動するのはこの file だけ。
use crate::observe::{Origin, SchedulerReply, SchedulerTransport, is_scheduler_job_id};

/// ssh の宛先に渡せる cluster 名か（英数字と `.`・`_`・`-`、先頭 `-` は不可）。recovery の `SshPbs` も共用する。
pub(crate) fn is_safe_cluster(cluster: &str) -> bool {
    !cluster.is_empty()
        && !cluster.starts_with('-')
        && cluster
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// 本番の read-only 照会: `ssh -o BatchMode=yes <cluster> qstat -x -f <job id>`（PBS のみ）。
/// cluster 名と job id は shell に渡るので、英数字と `.`・`-`（cluster は `_` も）だけを受ける。
pub struct SshQstat;

impl SchedulerTransport for SshQstat {
    fn origin(&self) -> Origin {
        Origin::Live
    }

    fn qstat(
        &self,
        scheduler: &str,
        cluster: &str,
        job_id: &str,
    ) -> Result<SchedulerReply, String> {
        if scheduler != "pbs" {
            return Err(format!("scheduler {scheduler} is not supported (pbs only)"));
        }
        if !is_safe_cluster(cluster) || !is_scheduler_job_id(job_id) {
            return Err(format!("refusing to query {cluster}:{job_id}"));
        }
        let o = std::process::Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "--",
                cluster,
                "qstat",
                "-x",
                "-f",
                job_id,
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("ssh {cluster}: {e}"))?;
        if !o.status.success() {
            return Err(format!(
                "ssh {cluster} qstat -x -f {job_id}: {} {}",
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Ok(SchedulerReply {
            cluster: cluster.into(),
            raw: String::from_utf8_lossy(&o.stdout).into_owned(),
        })
    }
}
