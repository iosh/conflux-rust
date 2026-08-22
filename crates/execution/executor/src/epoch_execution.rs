//! Applies mandatory protocol transitions before epoch and block transactions.

use cfx_statedb::Result as DbResult;
use cfx_types::U256;
use primitives::{Block, BlockNumber};

use crate::{
    internal_contract::{
        block_hash_slot, epoch_hash_slot, initialize_internal_contract_accounts,
    },
    machine::Machine,
    state::{
        initialize_cip107, initialize_cip137,
        initialize_or_update_dao_voted_params, State,
    },
};

/// Applies the protocol state changes that precede transaction execution in an
/// epoch.
///
/// # Errors
///
/// Returns a state database error when a required system-state write fails.
pub fn before_epoch_execution(
    state: &mut State, machine: &Machine, pivot_block: &Block,
) -> DbResult<()> {
    let params = machine.params();

    let epoch_number = pivot_block.block_header.height();
    let hash = pivot_block.hash();
    let parent_hash = pivot_block.block_header.parent_hash();

    if epoch_number >= params.transition_heights.cip133e {
        state.set_system_storage(
            epoch_hash_slot(epoch_number).into(),
            U256::from_big_endian(&hash.0),
        )?;
    }

    if epoch_number >= params.transition_heights.eip2935 {
        state.set_eip2935_storage(epoch_number - 1, *parent_hash)?;
    }

    Ok(())
}

/// Applies the protocol state changes that precede transaction execution in a
/// block and returns the secondary reward observed at that boundary.
///
/// # Errors
///
/// Returns a state database error when a required state transition fails.
pub fn before_block_execution(
    state: &mut State, machine: &Machine, block_number: BlockNumber,
    block: &Block,
) -> DbResult<U256> {
    let params = machine.params();
    let transition_numbers = &params.transition_numbers;

    let cip94_start = transition_numbers.cip94n;
    let period = params.params_dao_vote_period;
    // Update/initialize parameters before processing rewards.
    if block_number >= cip94_start
        && (block_number - cip94_start) % period == 0
    {
        let set_pos_staking = block_number > transition_numbers.cip105;
        initialize_or_update_dao_voted_params(state, set_pos_staking)?;
    }

    // Initialize old_storage_point_prop_ratio in the state.
    // The time may not be in the vote period boundary, so this is not
    // integrated with initialize_or_update_dao_voted_params, but that
    // function will update the value after cip107 is enabled here.
    if block_number == transition_numbers.cip107 {
        initialize_cip107(state)?;
    }

    if block_number >= transition_numbers.cip133b {
        state.set_system_storage(
            block_hash_slot(block_number).into(),
            U256::from_big_endian(&block.hash().0),
        )?;
    }

    if block_number == transition_numbers.cip137 {
        initialize_cip137(state);
    }

    if block_number < transition_numbers.cip43a {
        state.bump_block_number_accumulate_interest();
    }

    let secondary_reward = state.secondary_reward();

    state.inc_distributable_pos_interest(block_number)?;

    initialize_internal_contract_accounts(
        state,
        machine.internal_contracts().initialized_at(block_number),
    )?;

    state.commit_cache(false);

    Ok(secondary_reward)
}
