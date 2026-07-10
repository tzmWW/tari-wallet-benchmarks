use anyhow::{Context, bail};

/// Pinned transaction weights: one kernel (10), one input (8), and 53 per output.
/// A stealth output with default features/script/covenant and an empty memo rounds to
/// four feature/script grams, for 57 grams total per output.
pub(super) const STEALTH_OUTPUT_GRAMS: u64 = 57;

/// Console-wallet `send_one_sided_multi_recipient_transaction` adds the sender address
/// to every memo. The pinned MemoField is padded to 130 bytes, making each output's
/// rounded feature/script contribution 12 grams and its total weight 65 grams.
pub(super) const CONSOLE_SELF_OUTPUT_GRAMS: u64 = 65;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExactSplitPlan {
    pub(super) input_microtari: u64,
    pub(super) fee_microtari: u64,
    pub(super) child_amounts: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PpSplitPlan {
    pub(super) input_microtari: u64,
    pub(super) fee_microtari: u64,
    /// PP creates these explicit self-payment outputs. Its builder creates the
    /// final balanced child as the ordinary change output.
    pub(super) payment_amounts: Vec<u64>,
    pub(super) change_microtari: u64,
}

impl ExactSplitPlan {
    pub(super) fn total_children(&self) -> u64 {
        self.child_amounts.iter().copied().sum()
    }
}

/// Plans a one-input transaction whose recipients plus exact pinned fee consume the
/// entire input. The final recipient absorbs the integer remainder, so the builder has
/// no value from which to create a change output.
pub(super) fn exact_no_change_split(
    input_microtari: u64,
    child_count: u32,
    fee_per_gram: u64,
    output_grams: u64,
) -> anyhow::Result<ExactSplitPlan> {
    if child_count < 2 {
        bail!("exact split requires at least two child outputs");
    }
    let child_count_u64 = u64::from(child_count);
    let weight = 10u64
        .checked_add(8)
        .and_then(|base| base.checked_add(output_grams.checked_mul(child_count_u64)?))
        .context("exact split transaction weight overflow")?;
    let fee_microtari = weight
        .checked_mul(fee_per_gram)
        .context("exact split fee overflow")?;
    let available = input_microtari
        .checked_sub(fee_microtari)
        .context("exact split input does not cover pinned fee")?;
    let base_child = available / child_count_u64;
    if base_child == 0 {
        bail!("exact split would create a zero-value child output");
    }
    let remainder = available % child_count_u64;
    let mut child_amounts = vec![base_child; child_count as usize];
    let last = child_amounts
        .last_mut()
        .context("exact split unexpectedly has no child outputs")?;
    *last = last
        .checked_add(remainder)
        .context("exact split final child overflow")?;
    let plan = ExactSplitPlan {
        input_microtari,
        fee_microtari,
        child_amounts,
    };
    if plan.total_children().checked_add(plan.fee_microtari) != Some(plan.input_microtari) {
        bail!("exact split conservation invariant failed");
    }
    Ok(plan)
}

pub(super) fn exact_pp_split_with_change(
    input_microtari: u64,
    child_count: u32,
) -> anyhow::Result<PpSplitPlan> {
    const PP_FEE_PER_GRAM: u64 = 5;
    const PP_LOCK_FEE_BUFFER: u64 = 200_000;
    if child_count < 2 {
        bail!("PP exact split requires at least two child outputs");
    }
    let payment_count = u64::from(child_count - 1);
    // Pinned PP f0572c9 unsigned_tx_creator: one kernel/input, explicit empty-memo
    // stealth outputs (57g each), and one padded change output (65g).
    let weight = 18u64
        .checked_add(
            STEALTH_OUTPUT_GRAMS
                .checked_mul(payment_count)
                .context("PP recipient weight overflow")?,
        )
        .and_then(|weight| weight.checked_add(CONSOLE_SELF_OUTPUT_GRAMS))
        .context("PP exact split weight overflow")?;
    let fee_microtari = weight
        .checked_mul(PP_FEE_PER_GRAM)
        .context("PP exact split fee overflow")?;
    let available = input_microtari
        .checked_sub(fee_microtari)
        .context("PP exact split input does not cover fee")?;
    let base_child = available / u64::from(child_count);
    if base_child <= fee_microtari {
        bail!("PP exact split child is too small for a later split fee");
    }
    let payment_amounts = vec![base_child; payment_count as usize];
    let explicit_total = base_child
        .checked_mul(payment_count)
        .context("PP payment total overflow")?;
    if explicit_total
        .checked_add(PP_LOCK_FEE_BUFFER)
        .is_none_or(|required| required > input_microtari)
    {
        bail!("PP exact split cannot satisfy the pinned 200000 µT lock buffer");
    }
    let change_microtari = input_microtari
        .checked_sub(explicit_total)
        .and_then(|remaining| remaining.checked_sub(fee_microtari))
        .context("PP exact split change underflow")?;
    Ok(PpSplitPlan {
        input_microtari,
        fee_microtari,
        payment_amounts,
        change_microtari,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_split_conserves_value_without_change_for_all_s1_targets() {
        let mut amounts = vec![10_000_000_000u64];
        for target in [2usize, 4, 8, 16, 32, 64] {
            amounts = amounts
                .into_iter()
                .flat_map(|input| {
                    exact_no_change_split(input, 2, 5, CONSOLE_SELF_OUTPUT_GRAMS)
                        .unwrap()
                        .child_amounts
                })
                .collect();
            assert_eq!(amounts.len(), target);
        }
        amounts = amounts
            .into_iter()
            .flat_map(|input| {
                exact_no_change_split(input, 8, 5, CONSOLE_SELF_OUTPUT_GRAMS)
                    .unwrap()
                    .child_amounts
            })
            .collect();
        assert_eq!(amounts.len(), 512);
    }

    #[test]
    fn final_child_absorbs_division_remainder() {
        let plan = exact_no_change_split(2_499_999_505, 2, 5, CONSOLE_SELF_OUTPUT_GRAMS).unwrap();
        assert_eq!(plan.child_amounts[1], plan.child_amounts[0] + 1);
        assert_eq!(
            plan.total_children() + plan.fee_microtari,
            plan.input_microtari
        );
    }

    #[test]
    fn pp_split_uses_balanced_change_to_reach_all_s1_targets() {
        let mut amounts = vec![10_000_000_000u64];
        for target in [2usize, 4, 8, 16, 32, 64] {
            amounts = amounts
                .into_iter()
                .flat_map(|input| {
                    let plan = exact_pp_split_with_change(input, 2).unwrap();
                    plan.payment_amounts
                        .into_iter()
                        .chain([plan.change_microtari])
                })
                .collect();
            assert_eq!(amounts.len(), target);
        }
        amounts = amounts
            .into_iter()
            .flat_map(|input| {
                let plan = exact_pp_split_with_change(input, 8).unwrap();
                plan.payment_amounts
                    .into_iter()
                    .chain([plan.change_microtari])
            })
            .collect();
        assert_eq!(amounts.len(), 512);
    }
}
