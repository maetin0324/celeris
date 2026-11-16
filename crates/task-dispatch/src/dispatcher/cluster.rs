//! cluster / tunnel の `Dispatcher` の側の入口（ADR-0082 の L1）。状態と処理は [`ClusterRuntime`]
//! （`crate::cluster_runtime`。ADR 2026-11-11 dispatcher-state-ownership D1・D2）が持つ。ここは
//! [`ClusterInputs`] を組んで owner の method を呼ぶ入口と、org・run の表を読むクラスタの解決だけを持つ。

use super::*;
use crate::cluster_runtime::ClusterInputs;
use task_core::OrgDirectory;

impl Dispatcher {
    /// owner と、owner の外の値（store・`[[clusters]]`・時計）を分けて借りる。
    fn cluster_parts<'a>(
        &'a mut self,
        clock: &'a dyn Fn() -> Instant,
    ) -> (&'a mut ClusterRuntime, ClusterInputs<'a>) {
        (
            &mut self.cluster,
            ClusterInputs {
                store: self.store.as_ref(),
                clusters: &self.config.clusters,
                cooldown: self.config.cluster_cooldown,
                verify_smoke: self.eligible.is_some(),
                clock,
                last_tick_gap_ms: self.coord.last_tick_gap_ms(),
            },
        )
    }

    /// ADR-0053 D3: 鍵認証の connect hook を挿す。
    pub fn set_cluster_connector(&mut self, connector: ClusterConnector) {
        self.cluster.set_cluster_connector(connector);
    }

    /// GUI の接続を待っている印を付ける／外す。
    pub fn set_cluster_connect_pending(&mut self, id: &str, pending: bool) {
        self.cluster.set_cluster_connect_pending(id, pending);
    }

    /// ADR-0066: forward を張り直す hook を挿す。
    pub fn set_tunnel_forward_ensurer(&mut self, ensurer: TunnelForwardEnsurer) {
        self.cluster.set_tunnel_forward_ensurer(ensurer);
    }

    /// ADR-0066: forward の先の probe を挿す。
    pub fn set_tunnel_probe(&mut self, probe: TunnelProbe) {
        self.cluster.set_tunnel_probe(probe);
    }

    /// ADR-0066: 手元の listener の probe を挿す。
    pub fn set_tunnel_listener_probe(&mut self, probe: TunnelListenerProbe) {
        self.cluster.set_tunnel_listener_probe(probe);
    }

    /// ssh master の生存確認の probe を差し替える。
    pub fn set_cluster_liveness_probe(&mut self, probe: ClusterLivenessProbe) {
        self.cluster.set_cluster_liveness_probe(probe);
    }

    /// remote の ssh・rsync の command を差し替える（試験用）。
    pub fn set_cluster_ssh_command_override(&mut self, command: Vec<String>) {
        self.cluster.set_cluster_ssh_command_override(command);
    }

    /// remote の run の ssh の設定（override を反映する）。
    pub(super) fn remote_ssh_settings(
        &self,
        spec: &ClusterSpec,
        remote_path: &std::path::Path,
        task_id: task_core::TaskId,
        mode: task_core::WorkspaceMode,
    ) -> SshSettings {
        self.cluster
            .remote_ssh_settings(spec, remote_path, task_id, mode)
    }

    /// ADR-0078 D3-3: `ssh -- true` の probe を挿す。
    pub fn set_cluster_command_probe(&mut self, probe: ClusterCommandProbe) {
        self.cluster.set_cluster_command_probe(probe);
    }

    /// ADR-0062 A: master の自発的な終了を見る hook を挿す。
    pub fn set_cluster_master_watcher(&mut self, watcher: ClusterMasterWatcher) {
        self.cluster.set_cluster_master_watcher(watcher);
    }

    /// ADR-0066: たまった tunnel の event を取り出す。
    pub fn take_tunnel_events(&mut self) -> Vec<TunnelEvent> {
        self.cluster.take_tunnel_events()
    }

    /// login が要るクラスタ（名前順）。
    pub fn clusters_needing_login(&self) -> Vec<String> {
        self.cluster.clusters_needing_login()
    }

    /// forward が使えるか（listener があり、先が健全）。
    pub fn tunnel_reachable(&self, cluster: &str, listen: &str) -> bool {
        self.cluster.tunnel_reachable(cluster, listen)
    }

    /// 手元の listener があるか。
    pub fn tunnel_listener_present(&self, cluster: &str, listen: &str) -> bool {
        self.cluster.tunnel_listener_present(cluster, listen)
    }

    /// forward の先が健全か。
    pub fn tunnel_target_healthy(&self, cluster: &str, listen: &str) -> bool {
        self.cluster.tunnel_target_healthy(cluster, listen)
    }

    /// forward の最後の誤り。
    pub fn tunnel_last_error(&self, cluster: &str, listen: &str) -> Option<String> {
        self.cluster.tunnel_last_error(cluster, listen)
    }

    /// ADR-0018 D5: クラスタが使えないことを記録し、cooldown に入れる。
    pub(super) fn mark_cluster_unavailable(
        &mut self,
        task_id: TaskId,
        spec: &ClusterSpec,
        reason: String,
    ) -> Result<(), DispatchError> {
        let clock = self.monotonic_clock();
        let (cluster, cx) = self.cluster_parts(&clock);
        Ok(cluster.mark_cluster_unavailable(&cx, task_id, spec, reason)?)
    }

    /// ADR-0053 D3: publickey のクラスタに自動で接続する（hook は async の外で走らせる）。
    pub(super) fn try_auto_connect_cluster(
        &self,
        spec: &ClusterSpec,
    ) -> Option<Result<(), String>> {
        self.cluster.try_auto_connect_cluster(spec)
    }

    /// ADR-0023・ADR-0078: master の生存確認（間引きあり）。
    pub(super) fn refresh_cluster_liveness(&mut self) {
        let clock = self.monotonic_clock();
        let (cluster, cx) = self.cluster_parts(&clock);
        cluster.refresh_cluster_liveness(&cx);
    }

    /// ADR-0078: 接続の状態の遷移を記録する。
    pub(super) fn set_cluster_connected(
        &mut self,
        id: &str,
        alive: bool,
        change: ClusterConnChange,
    ) {
        let clock = self.monotonic_clock();
        let (cluster, cx) = self.cluster_parts(&clock);
        cluster.set_cluster_connected(&cx, id, alive, change);
    }

    /// ADR-0078: 鍵認証の再接続の試みを記録する。
    pub(super) fn note_key_auth_attempt(&mut self, spec: &ClusterSpec, ok: bool) {
        let clock = self.monotonic_clock();
        let (cluster, cx) = self.cluster_parts(&clock);
        cluster.note_key_auth_attempt(&cx, spec, ok);
    }

    /// ADR-0062 A: master の自発的な終了を拾う。
    pub(super) fn refresh_cluster_master_exits(&mut self) {
        let clock = self.monotonic_clock();
        let (cluster, cx) = self.cluster_parts(&clock);
        cluster.refresh_cluster_master_exits(&cx);
    }

    /// ADR-0066: tunnel の forward を確かめ、張り直す（間引きあり）。
    pub(super) fn refresh_cluster_tunnels(&mut self) {
        let clock = self.monotonic_clock();
        let (cluster, cx) = self.cluster_parts(&clock);
        cluster.refresh_cluster_tunnels(&cx);
    }

    /// ADR-0066: tunnel の前に master を確かめる（試験の入口）。
    #[cfg(test)]
    pub(super) fn ensure_cluster_master_for_tunnel(&mut self, spec: &ClusterSpec) -> bool {
        let clock = self.monotonic_clock();
        let (cluster, cx) = self.cluster_parts(&clock);
        cluster.ensure_cluster_master_for_tunnel(&cx, spec)
    }

    /// ADR-0018: `WorkspaceSpec::Remote` のタスクのクラスタ設定とリモートのパス。ローカルのタスクは `None`。
    ///
    /// ADR-0062 B1（Phase 107）: **担当が `cluster:<id>` を持たないなら接続経路を渡さない**（remote を
    /// 組まない）。人間の指示により、ADR-0046 D8 の「道具を 1 つも宣言していないノードは従来どおり通す」
    /// という例外は**クラスタの利用可否についてだけ**廃止した（`task_may_use_cluster`）。
    ///
    /// ADR-0059 D6: `path` が絶対・`~`/`~/…` ならそのまま、それ以外（省略・相対）は実効 `work_dir`
    /// （DB 上書き `cluster_settings` > 設定の `[[clusters]] work_dir` > 無し）からの相対に解決する。
    /// 解決できなければ `path` をそのまま返す（`remote_dir_is_resolved` で検出できる形のまま。
    /// ここでは `None` にしない — 「クラスタが無い」とは別の失敗なので `unroutable` に混ぜない）。
    pub(super) fn cluster_of(
        &mut self,
        task: &Task,
    ) -> Option<(ClusterSpec, PathBuf, WorkspaceMode)> {
        match self.resolve_cluster(task) {
            ClusterResolution::Resolved(spec, path, mode) => Some((*spec, path, mode)),
            ClusterResolution::Local
            | ClusterResolution::NotConfigured
            | ClusterResolution::AssigneeLacksTool { .. } => None,
        }
    }

    /// `cluster_of` の中身。`None` の理由を dispatch_ready が区別できるようにする（ADR-0062 B1）。
    pub(super) fn resolve_cluster(&mut self, task: &Task) -> ClusterResolution {
        match &task.workspace {
            WorkspaceSpec::Local { .. } => ClusterResolution::Local,
            WorkspaceSpec::Remote { cluster, path, .. } => {
                if !self.task_may_use_cluster(task, cluster) {
                    return ClusterResolution::AssigneeLacksTool {
                        cluster: cluster.clone(),
                    };
                }
                let Some(spec) = self.config.clusters.get(cluster).cloned() else {
                    return ClusterResolution::NotConfigured;
                };
                let db_work_dir = self
                    .store
                    .cluster_settings_get(cluster)
                    .ok()
                    .flatten()
                    .and_then(|s| s.work_dir)
                    .map(PathBuf::from);
                let work_dir = db_work_dir.or_else(|| spec.work_dir.clone());
                let resolved =
                    resolve_remote_dir(path, work_dir.as_deref()).unwrap_or_else(|| path.clone());
                ClusterResolution::Resolved(Box::new(spec), resolved, task.workspace.remote_mode())
            }
        }
    }

    /// ADR-0062 B1: そのタスクの担当がそのクラスタを使えるか（決定的。組織の profile だけを見る）。
    /// 人間の指示（Phase 107 追加指示）により、ADR-0046 D8 の「道具を 1 つも宣言していないノードは
    /// 従来どおり通す」という例外はここでは**廃止した**（remote 作業場所でクラスタに繋いでよいのは、
    /// 実効 profile に `cluster:<id>` を持つノードだけ）。warn はタスクごとに 1 回
    /// （`warned_cluster_tool`。`warned_unroutable` と同じ流儀）。
    pub(super) fn task_may_use_cluster(&mut self, task: &Task, cluster: &str) -> bool {
        let Some(assignee) = task.assignee.as_deref() else {
            return true;
        };
        let org = match self.store.org_list() {
            Ok(org) => org,
            Err(e) => {
                tracing::warn!(task_id = %task.id, error = %e, "could not read the org tree; allowing the cluster");
                return true;
            }
        };
        let effective = task_core::resolve_profile(&org, assignee);
        let wanted = format!("{}{cluster}", task_core::CLUSTER_TOOL_PREFIX);
        let allowed = effective.has_tool(&wanted);
        if self.cluster.note_cluster_tool(task.id, allowed) {
            tracing::warn!(
                task_id = %task.id, %assignee, %cluster,
                "assignee does not have the cluster tool; not wiring the remote (ADR-0062 B1)"
            );
        }
        allowed
    }

    /// そのクラスタで走っている run の数（ADR-0018 D5: プロバイダとクラスタの二次元）。
    pub(super) fn cluster_in_use(&self, cluster: &str) -> usize {
        self.supervisor
            .runs()
            .values()
            .filter(|e| e.cluster.as_deref() == Some(cluster))
            .count()
            + self
                .supervisor
                .reviews()
                .values()
                .filter(|e| e.cluster.as_deref() == Some(cluster))
                .count()
    }
}
