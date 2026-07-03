use crate::payment_processor::PaymentProcessorDbSnapshot;

pub(super) fn pp_snapshot_is_terminal_for_summary(
    snapshot: &PaymentProcessorDbSnapshot,
    accepted_batches: u32,
) -> bool {
    if snapshot.has_upstream_signing_or_broadcast_error() {
        return true;
    }
    let accepted_batches = usize::try_from(accepted_batches).unwrap_or(usize::MAX);
    snapshot.batches.len() >= accepted_batches
        && snapshot
            .batches
            .iter()
            .all(|batch| matches!(batch.status.as_str(), "CONFIRMED" | "FAILED" | "CANCELLED"))
}
