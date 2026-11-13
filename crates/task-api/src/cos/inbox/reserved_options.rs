//! ADR 2026-11-12-discord-person-or-operator D2: an escalation to the person names the options only
//! the person may choose, and CoS answers a recommendation outside them itself.
//!
//! Case 1 of the ADR (task 01M782BS, 2026-11-12): CoS sent a whole authorization to the person
//! because two of its options (stop and close with missing data, raise the node-h limit) were the
//! person's, while the recommended option fitted the approved frame. The person then found the
//! wait already answered. Now such an escalation is refused with [`ANSWER_FIRST`]: CoS answers the
//! recommended option (`within_approved_frame` or `task_scope`) and writes the reserved options into
//! `answer.note` as the question the worker asks later.

use task_core::store::SqliteStore;
use task_ops::human_inbox::InboxItem;

use super::{Addressee, CosInboxItem, EscalationPacket, kept_by_person, unprocessable};
use crate::problem::ApiProblem;

/// 422: the recommended option of an escalation to the person is not one only the person may choose.
pub const ANSWER_FIRST: &str = "cos_answer_unreserved_recommendation";

/// The check of `escalation.reserved_options` (run before the route is taken).
pub(super) fn check(
    store: &SqliteStore,
    packet: &EscalationPacket,
    item: &CosInboxItem,
    source: Option<&InboxItem>,
) -> Result<(), ApiProblem> {
    let invalid = |detail: String| unprocessable("cos_escalation_invalid", detail);
    for key in &packet.reserved_options {
        if !packet.options.iter().any(|o| &o.key == key) {
            return Err(invalid(format!(
                "escalation.reserved_options {key} is not one of escalation.options"
            )));
        }
    }
    if packet.to == Addressee::Operator {
        return if packet.reserved_options.is_empty() {
            Ok(())
        } else {
            Err(invalid(
                "reserved_options are the person's: escalate with to=human".into(),
            ))
        };
    }
    // The person keeps the whole item (an explicit human_required, an override wait, a D1 match).
    if kept_by_person(store, item, source)?.is_some() {
        return Ok(());
    }
    if packet.reserved_options.is_empty() {
        return Err(invalid(
            "an escalation to the person names escalation.reserved_options: the options only the \
             person may choose (credential input, beyond the approved frame, objective change, \
             security boundary, production config, paid trial, research judgment, weaker checks)"
                .into(),
        ));
    }
    if let Some(recommended) = &packet.recommended
        && !packet.reserved_options.contains(recommended)
    {
        return Err(unprocessable(
            ANSWER_FIRST,
            format!(
                "the recommended option {recommended} is not reserved for the person: answer it now \
                 (basis within_approved_frame or task_scope) and write the reserved options ({}) \
                 into answer.note as the question the worker asks the person later",
                packet.reserved_options.join(", ")
            ),
        ));
    }
    Ok(())
}
