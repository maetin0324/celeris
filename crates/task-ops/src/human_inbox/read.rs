use super::*;

/// Current question of one Task, identical to its item in the complete inbox.
/// The Task and root are fetched by primary key; unrelated Task payloads are not read.
pub fn question_for_task(
    store: &dyn TaskStore,
    id: TaskId,
    now: OffsetDateTime,
) -> Result<Option<InboxItem>, OpsError> {
    let Some(task) = store.get(id)? else {
        return Ok(None);
    };
    if task.status != task_core::Status::Blocked {
        return Ok(None);
    }
    let Some(question) = crate::inbox::question_for_task(store, &task)? else {
        return Ok(None);
    };
    if question.approval_id.is_some() {
        return Ok(None);
    }
    let root = task_core::tree::root_id_of(&task);
    let mut by_id = HashMap::new();
    if root != id
        && let Some(parent) = store.get(root)?
    {
        by_id.insert(root, parent);
    }
    by_id.insert(id, task);
    Ok(Some(Builder { by_id: &by_id, now }.question(&question)))
}

pub(crate) fn retain_touching(mut human: HumanInbox, touching: &BTreeSet<TaskId>) -> HumanInbox {
    human
        .items
        .retain(|item| item.task_ids().any(|id| touching.contains(&id)));
    let mut by_kind: BTreeMap<String, u32> = BTreeMap::new();
    for it in &human.items {
        *by_kind.entry(it.kind.as_str().to_string()).or_default() += 1;
    }
    human.counts = HumanInboxCounts {
        total: human.items.len() as u32,
        by_kind,
    };
    human
}
