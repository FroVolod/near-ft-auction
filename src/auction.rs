use near_sdk::{AccountId, Promise, PromiseOrValue, env, json_types::U128, require};

use super::{AuctionState, Bid, Contract, PayoutStatus, StoredBid, ft_properties, transfer_tokens};

/// Starts a new auction and finalizes the previous round when needed.
pub(super) fn start_new_auction(
    contract: &mut Contract,
    auction_duration: String,
    description: String,
    bid_step: ft_properties::FungibleToken,
    max_bid_step: Option<ft_properties::FungibleToken>,
) -> PromiseOrValue<bool> {
    require!(
        env::attached_deposit().as_yoctonear() == contract.auction_start_fee.as_yoctonear(),
        "Attached deposit must exactly match the auction start fee"
    );
    require!(
        !description.trim().is_empty(),
        "Auction description cannot be empty"
    );
    require!(
        description.len() <= 256,
        "Auction description cannot exceed 256 bytes"
    );

    let duration = super::auction_time::parse_duration(&auction_duration)
        .unwrap_or_else(|error| env::panic_str(&error));
    let auction_start_time = env::block_timestamp();
    let auction_end_time = auction_start_time
        .checked_add(duration)
        .unwrap_or_else(|| env::panic_str("Auction end time exceeds the maximum timestamp"));
    let bid_step = bid_step
        .normalize(contract.token_decimals, &contract.token_symbol)
        .unwrap_or_else(|error| env::panic_str(&error));
    require!(bid_step.amount() > 0, "Bid step must be greater than zero");
    let max_bid_step = max_bid_step.map(|max_bid_step| {
        let max_bid_step = max_bid_step
            .normalize(contract.token_decimals, &contract.token_symbol)
            .unwrap_or_else(|error| env::panic_str(&error));
        require!(
            max_bid_step.amount() >= bid_step.amount(),
            "Maximum bid step must not be less than the minimum bid step"
        );
        max_bid_step
    });

    let payout_promise = if let Some(auction) = &contract.auction {
        require!(
            env::block_timestamp() > auction.auction_end_time,
            "The current auction has not ended yet"
        );
        if auction.is_claimed {
            None
        } else {
            finalize_current_auction(contract)
        }
    } else {
        None
    };

    let auctioneer = env::predecessor_account_id();
    let auction_id = contract.next_auction_id;
    contract.next_auction_id = auction_id
        .checked_add(1)
        .unwrap_or_else(|| env::panic_str("Auction ID overflow"));
    contract.auction = Some(AuctionState {
        id: auction_id,
        description,
        auctioneer,
        highest_bid: StoredBid {
            bidder: env::current_account_id(),
            bid: bid_step.with_amount(0),
        },
        bid_step,
        max_bid_step,
        auction_start_time,
        auction_end_time,
        is_claimed: false,
    });
    contract.accrued_start_fees = contract
        .accrued_start_fees
        .checked_add(contract.auction_start_fee.as_yoctonear())
        .unwrap_or_else(|| env::panic_str("Accumulated auction fees exceed the maximum"));

    match payout_promise {
        Some(promise) => PromiseOrValue::Promise(promise),
        None => PromiseOrValue::Value(true),
    }
}

/// Finalizes the current auction and initiates the proceeds transfer to its auctioneer.
pub(super) fn finalize_current_auction(contract: &mut Contract) -> Option<Promise> {
    let (auction_id, description, auctioneer, winning_bid, auction_end_time) = {
        let auction = contract
            .auction
            .as_mut()
            .unwrap_or_else(|| env::panic_str("No auction has been started"));
        require!(
            env::block_timestamp() > auction.auction_end_time,
            "Auction has not ended yet"
        );
        require!(!auction.is_claimed, "Auction has already been claimed");
        auction.is_claimed = true;
        (
            auction.id,
            auction.description.clone(),
            auction.auctioneer.clone(),
            Bid {
                bidder: auction.highest_bid.bidder.clone(),
                bid: auction.highest_bid.bid.clone(),
            },
            auction.auction_end_time,
        )
    };
    let winning_amount = winning_bid.bid.amount();
    let claim_fee = calculate_fee(winning_amount, contract.claim_fee_basis_points);
    contract.accrued_claim_fees = contract
        .accrued_claim_fees
        .checked_add(claim_fee)
        .unwrap_or_else(|| env::panic_str("Accumulated claim fees exceed the maximum"));
    let payout_amount = winning_amount - claim_fee;
    let mut totals = contract
        .auctioneer_totals
        .get(&auctioneer)
        .cloned()
        .unwrap_or_default();
    let auctioneer_index = totals.auction_count;
    totals.auction_count = totals
        .auction_count
        .checked_add(1)
        .unwrap_or_else(|| env::panic_str("Auctioneer auction count overflow"));
    totals.total_winning_amount = totals
        .total_winning_amount
        .checked_add(winning_amount)
        .unwrap_or_else(|| env::panic_str("Total winning amount exceeds the maximum"));

    let payout_status = if payout_amount == 0 {
        PayoutStatus::NotRequired
    } else {
        totals.pending_payout_amount = totals
            .pending_payout_amount
            .checked_add(payout_amount)
            .unwrap_or_else(|| env::panic_str("Pending payout total exceeds the maximum"));
        PayoutStatus::InFlight
    };
    contract
        .auctioneer_totals
        .insert(auctioneer.clone(), totals);
    contract.auctioneer_auctions.insert(
        (auctioneer.clone(), auctioneer_index),
        super::AuctioneerAuction {
            id: auction_id,
            description,
            auction_end_time,
            winning_bid,
            claim_fee: ft_properties::FungibleToken::from_params_ft(
                claim_fee,
                contract.token_decimals,
                contract.token_symbol.clone(),
            ),
            payout_amount: ft_properties::FungibleToken::from_params_ft(
                payout_amount,
                contract.token_decimals,
                contract.token_symbol.clone(),
            ),
            payout_status,
        },
    );

    if payout_amount == 0 {
        return None;
    }

    let payout_id = contract.next_payout_id;
    contract.next_payout_id = payout_id
        .checked_add(1)
        .unwrap_or_else(|| env::panic_str("Pending payout ID overflow"));
    contract.pending_payouts.push(super::PendingPayout {
        id: payout_id,
        auction_id,
        auctioneer_index,
        receiver_id: auctioneer,
        amount: ft_properties::FungibleToken::from_params_ft(
            payout_amount,
            contract.token_decimals,
            contract.token_symbol.clone(),
        ),
        in_flight: true,
    });

    Some(transfer_pending_payout(contract, payout_id))
}

/// Retries a payout that previously failed.
pub(super) fn retry_pending_payout(contract: &mut Contract, payout_id: u64) -> Promise {
    let payout = contract
        .pending_payouts
        .iter_mut()
        .find(|payout| payout.id == payout_id)
        .unwrap_or_else(|| env::panic_str("Pending payout was not found"));
    require!(!payout.in_flight, "Payout is already in progress");
    payout.in_flight = true;
    let receiver_id = payout.receiver_id.clone();
    let auctioneer_index = payout.auctioneer_index;
    contract
        .auctioneer_auctions
        .get_mut(&(receiver_id, auctioneer_index))
        .unwrap_or_else(|| env::panic_str("Auctioneer auction history was not found"))
        .payout_status = PayoutStatus::InFlight;
    transfer_pending_payout(contract, payout_id)
}

/// Processes an FT transfer as a bid and returns any unused amount.
pub(super) fn receive_bid(
    contract: &mut Contract,
    sender_id: AccountId,
    amount: U128,
    msg: String,
) -> PromiseOrValue<U128> {
    let _ = msg;
    require!(
        env::predecessor_account_id() == contract.token_contract,
        "Only the configured token contract can submit bids"
    );

    let auction = contract
        .auction
        .as_mut()
        .unwrap_or_else(|| env::panic_str("No auction has been started"));
    require!(
        env::block_timestamp() < auction.auction_end_time,
        "Auction has ended"
    );
    let StoredBid {
        bidder: last_bidder,
        bid: last_bid,
    } = auction.highest_bid.clone();
    let last_bid_amount = last_bid.amount();
    let bid_step_amount = auction.bid_step.amount();
    let is_current_bidder = sender_id == last_bidder;
    let minimum_payment = if is_current_bidder {
        bid_step_amount
    } else {
        last_bid_amount
            .checked_add(bid_step_amount)
            .expect("Minimum bid exceeds the maximum token amount")
    };
    if amount.0 < minimum_payment {
        return PromiseOrValue::Value(amount);
    }

    let accepted_amount = match auction.max_bid_step.as_ref() {
        Some(max_bid_step) if is_current_bidder => amount.0.min(max_bid_step.amount()),
        Some(max_bid_step) => {
            let maximum_bid = last_bid_amount
                .checked_add(max_bid_step.amount())
                .expect("Maximum bid exceeds the maximum token amount");
            amount.0.min(maximum_bid)
        }
        None => amount.0,
    };
    let refund_amount = amount.0 - accepted_amount;

    let new_bid = if is_current_bidder {
        last_bid_amount
            .checked_add(accepted_amount)
            .expect("Bid exceeds the maximum token amount")
    } else {
        accepted_amount
    };
    auction.highest_bid = StoredBid {
        bidder: sender_id,
        bid: auction.bid_step.with_amount(new_bid),
    };

    if !is_current_bidder && last_bid_amount > 0 {
        // This example does not recover the refund if the asynchronous token transfer fails.
        let _ = transfer_tokens(
            contract.token_contract.clone(),
            last_bidder,
            U128(last_bid_amount),
        );
    }

    PromiseOrValue::Value(U128(refund_amount))
}

/// Creates an FT transfer for a pending payout and a callback to process its result.
fn transfer_pending_payout(contract: &Contract, payout_id: u64) -> Promise {
    let payout = contract
        .pending_payouts
        .iter()
        .find(|payout| payout.id == payout_id)
        .unwrap_or_else(|| env::panic_str("Pending payout was not found"));
    transfer_tokens(
        contract.token_contract.clone(),
        payout.receiver_id.clone(),
        U128(payout.amount.amount()),
    )
    .then(contract.callback(
        "on_payout_complete",
        near_sdk::serde_json::json!({ "payout_id": payout_id }),
    ))
}

/// Calculates a fee in basis points without overflowing intermediate multiplication.
fn calculate_fee(amount: u128, fee_basis_points: u16) -> u128 {
    let fee_basis_points = u128::from(fee_basis_points);
    (amount / 10_000) * fee_basis_points + ((amount % 10_000) * fee_basis_points) / 10_000
}
