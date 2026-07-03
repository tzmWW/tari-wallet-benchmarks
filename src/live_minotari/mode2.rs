use crate::versions::TX_MINED_CONFIRMED_STATUS;

pub(super) fn mode2_completed_transaction_status(status: &str) -> (u32, bool) {
    match status {
        "mined_confirmed" => (TX_MINED_CONFIRMED_STATUS, true),
        "mined_unconfirmed" => (2, false),
        "broadcast" => (1, false),
        "completed" => (0, false),
        "rejected" => (7, false),
        "canceled" => (14, false),
        _ => (0, false),
    }
}
