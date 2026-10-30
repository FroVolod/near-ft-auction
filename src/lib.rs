use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::json_types::U128;
use near_sdk::store::LookupMap;
use near_sdk::{
    AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError, PromiseOrValue, Timestamp,
    env, near, require,
};

mod auction_time;
mod ft_properties;

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    token_contract: AccountId,
    token_symbol: String,
    token_decimals: u8,
    auction_start_fee: NearToken,
    claim_fee_basis_points: u16,
    owner_id: AccountId,
    accrued_start_fees: u128,
    accrued_claim_fees: u128,
    auction: Option<AuctionState>,
    pending_payouts: Vec<PendingPayout>,
    next_payout_id: u64,
    next_auction_id: u64,
    auctioneer_totals: LookupMap<AccountId, AuctioneerTotals>,
    auctioneer_auctions: LookupMap<(AccountId, u64), AuctioneerAuction>,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
struct AuctionState {
    id: u64,
    description: String,
    auctioneer: AccountId,
    highest_bid: StoredBid,
    bid_step: ft_properties::FungibleToken,
    max_bid_step: Option<ft_properties::FungibleToken>,
    auction_end_time: Timestamp,
    is_claimed: bool,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
struct StoredBid {
    bidder: AccountId,
    bid: ft_properties::FungibleToken,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Bid {
    pub bidder: AccountId,
    pub bid: ft_properties::FungibleToken,
}

#[near(serializers = [json])]
pub struct AuctionView {
    pub id: u64,
    pub description: String,
    pub auctioneer: AccountId,
    pub highest_bid: Bid,
    pub bid_step: ft_properties::FungibleToken,
    pub max_bid_step: Option<ft_properties::FungibleToken>,
    pub auction_end_time: Timestamp,
    pub time_remaining: String,
    pub is_claimed: bool,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct PendingPayout {
    pub id: u64,
    pub auction_id: u64,
    pub auctioneer_index: u64,
    pub receiver_id: AccountId,
    pub amount: ft_properties::FungibleToken,
    pub in_flight: bool,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayoutStatus {
    NotRequired,
    InFlight,
    Pending,
    Paid,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct AuctioneerAuction {
    pub id: u64,
    pub description: String,
    pub auction_end_time: Timestamp,
    pub winning_bid: Bid,
    pub claim_fee: ft_properties::FungibleToken,
    pub payout_amount: ft_properties::FungibleToken,
    pub payout_status: PayoutStatus,
}

#[near(serializers = [borsh])]
#[derive(Clone, Default)]
struct AuctioneerTotals {
    auction_count: u64,
    total_winning_amount: u128,
    total_paid_out: u128,
    pending_payout_amount: u128,
}

#[near(serializers = [json])]
pub struct AuctioneerHistory {
    pub auction_count: u64,
    pub total_winning_amount: ft_properties::FungibleToken,
    pub total_paid_out: ft_properties::FungibleToken,
    pub pending_payout_amount: ft_properties::FungibleToken,
    pub auctions: Vec<AuctioneerAuction>,
    pub next_index: Option<u64>,
}

#[near]
impl Contract {
    /// Initializes the contract with FT metadata and fee settings.
    #[init]
    pub fn init(
        token_contract: AccountId,
        token_symbol: String,
        token_decimals: u8,
        auction_start_fee: String,
        claim_fee_basis_points: u16,
    ) -> Self {
        require!(!token_symbol.is_empty(), "Token symbol cannot be empty");
        require!(
            claim_fee_basis_points <= 10_000,
            "Claim fee must not exceed 100%"
        );
        let auction_start_fee = auction_start_fee.parse::<NearToken>().unwrap_or_else(|_| {
            env::panic_str("Auction start fee must be a valid NEAR amount, e.g. \"0.1 NEAR\"")
        });
        Self {
            token_contract,
            token_symbol,
            token_decimals,
            auction_start_fee,
            claim_fee_basis_points,
            owner_id: env::predecessor_account_id(),
            accrued_start_fees: 0,
            accrued_claim_fees: 0,
            auction: None,
            pending_payouts: Vec::new(),
            next_payout_id: 0,
            next_auction_id: 0,
            auctioneer_totals: LookupMap::new(&b"auctioneer_totals"[..]),
            auctioneer_auctions: LookupMap::new(&b"auctioneer_auctions"[..]),
        }
    }

    /// Starts a new auction and finalizes the previous round when needed.
    #[payable]
    pub fn start_new_auction(
        &mut self,
        auction_duration: String,
        description: String,
        bid_step: ft_properties::FungibleToken,
        max_bid_step: Option<ft_properties::FungibleToken>,
    ) -> PromiseOrValue<bool> {
        require!(
            env::attached_deposit().as_yoctonear() == self.auction_start_fee.as_yoctonear(),
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

        let duration = auction_time::parse_duration(&auction_duration)
            .unwrap_or_else(|error| env::panic_str(&error));
        let auction_end_time = env::block_timestamp()
            .checked_add(duration)
            .unwrap_or_else(|| env::panic_str("Auction end time exceeds the maximum timestamp"));
        let bid_step = bid_step
            .normalize(self.token_decimals, &self.token_symbol)
            .unwrap_or_else(|error| env::panic_str(&error));
        require!(bid_step.amount() > 0, "Bid step must be greater than zero");
        let max_bid_step = max_bid_step.map(|max_bid_step| {
            let max_bid_step = max_bid_step
                .normalize(self.token_decimals, &self.token_symbol)
                .unwrap_or_else(|error| env::panic_str(&error));
            require!(
                max_bid_step.amount() >= bid_step.amount(),
                "Maximum bid step must not be less than the minimum bid step"
            );
            max_bid_step
        });

        let payout_promise = if let Some(auction) = &self.auction {
            require!(
                env::block_timestamp() > auction.auction_end_time,
                "The current auction has not ended yet"
            );
            if auction.is_claimed {
                None
            } else {
                self.finalize_current_auction()
            }
        } else {
            None
        };

        let auctioneer = env::predecessor_account_id();
        let auction_id = self.next_auction_id;
        self.next_auction_id = auction_id
            .checked_add(1)
            .unwrap_or_else(|| env::panic_str("Auction ID overflow"));
        self.auction = Some(AuctionState {
            id: auction_id,
            description,
            auctioneer,
            highest_bid: StoredBid {
                bidder: env::current_account_id(),
                bid: bid_step.with_amount(0),
            },
            bid_step,
            max_bid_step,
            auction_end_time,
            is_claimed: false,
        });
        self.accrued_start_fees = self
            .accrued_start_fees
            .checked_add(self.auction_start_fee.as_yoctonear())
            .unwrap_or_else(|| env::panic_str("Accumulated auction fees exceed the maximum"));

        match payout_promise {
            Some(promise) => PromiseOrValue::Promise(promise),
            None => PromiseOrValue::Value(true),
        }
    }

    /// Finalizes the current auction and initiates the proceeds transfer to its auctioneer.
    pub fn claim(&mut self) -> PromiseOrValue<bool> {
        let promise = self.finalize_current_auction();
        match promise {
            Some(promise) => PromiseOrValue::Promise(promise),
            None => PromiseOrValue::Value(true),
        }
    }

    /// Retries a payout that previously failed.
    pub fn retry_pending_payout(&mut self, payout_id: u64) -> Promise {
        let payout = self
            .pending_payouts
            .iter_mut()
            .find(|payout| payout.id == payout_id)
            .unwrap_or_else(|| env::panic_str("Pending payout was not found"));
        require!(!payout.in_flight, "Payout is already in progress");
        payout.in_flight = true;
        let receiver_id = payout.receiver_id.clone();
        let auctioneer_index = payout.auctioneer_index;
        self.auctioneer_auctions
            .get_mut(&(receiver_id, auctioneer_index))
            .unwrap_or_else(|| env::panic_str("Auctioneer auction history was not found"))
            .payout_status = PayoutStatus::InFlight;
        self.transfer_pending_payout(payout_id)
    }

    /// Transfers accrued auction start fees to the contract owner.
    pub fn withdraw_start_fees(&mut self) -> Promise {
        self.assert_owner();
        let amount = self.accrued_start_fees;
        require!(amount > 0, "There are no accrued auction start fees");
        self.accrued_start_fees = 0;
        Promise::new(self.owner_id.clone())
            .transfer(NearToken::from_yoctonear(amount))
            .then(self.callback(
                "on_start_fee_withdrawal",
                near_sdk::serde_json::json!({ "amount": U128(amount) }),
            ))
    }

    /// Transfers accrued winning-bid commissions to the contract owner.
    pub fn withdraw_claim_fees(&mut self) -> Promise {
        self.assert_owner();
        let amount = self.accrued_claim_fees;
        require!(amount > 0, "There are no accrued claim fees");
        self.accrued_claim_fees = 0;
        transfer_tokens(
            self.token_contract.clone(),
            self.owner_id.clone(),
            U128(amount),
        )
        .then(self.callback(
            "on_claim_fee_withdrawal",
            near_sdk::serde_json::json!({ "amount": U128(amount) }),
        ))
    }

    /// Returns the configured FT contract account ID.
    pub fn get_token_contract(&self) -> AccountId {
        self.token_contract.clone()
    }

    /// Returns the configured FT symbol.
    pub fn get_token_symbol(&self) -> String {
        self.token_symbol.clone()
    }

    /// Returns the number of decimals for the configured FT.
    pub fn get_token_decimals(&self) -> u8 {
        self.token_decimals
    }

    /// Returns the fixed fee for starting an auction.
    pub fn get_auction_start_fee(&self) -> NearToken {
        self.auction_start_fee
    }

    /// Returns the winning-bid commission in basis points.
    pub fn get_claim_fee_basis_points(&self) -> u16 {
        self.claim_fee_basis_points
    }

    /// Returns the account authorized to withdraw contract fees.
    pub fn get_contract_owner(&self) -> AccountId {
        self.owner_id.clone()
    }

    /// Returns the accrued auction start fees in yoctoNEAR.
    pub fn get_accrued_start_fees(&self) -> U128 {
        U128(self.accrued_start_fees)
    }

    /// Returns the accrued FT commissions in a human-readable format.
    pub fn get_accrued_claim_fees(&self) -> ft_properties::FungibleToken {
        ft_properties::FungibleToken::from_params_ft(
            self.accrued_claim_fees,
            self.token_decimals,
            self.token_symbol.clone(),
        )
    }

    /// Returns the highest bid for the active auction, if one has started.
    pub fn get_highest_bid(&self) -> Option<Bid> {
        self.auction.as_ref().map(|auction| Bid {
            bidder: auction.highest_bid.bidder.clone(),
            bid: auction.highest_bid.bid.clone(),
        })
    }

    /// Returns the full status of the active auction.
    pub fn get_auction_status(&self) -> Option<AuctionView> {
        self.auction.as_ref().map(|auction| AuctionView {
            id: auction.id,
            description: auction.description.clone(),
            auctioneer: auction.auctioneer.clone(),
            highest_bid: Bid {
                bidder: auction.highest_bid.bidder.clone(),
                bid: auction.highest_bid.bid.clone(),
            },
            bid_step: auction.bid_step.clone(),
            max_bid_step: auction.max_bid_step.clone(),
            auction_end_time: auction.auction_end_time,
            time_remaining: auction_time::format_time_remaining(
                auction
                    .auction_end_time
                    .saturating_sub(env::block_timestamp()),
            ),
            is_claimed: auction.is_claimed,
        })
    }

    /// Returns the readable time remaining in the active auction.
    pub fn get_auction_time_remaining(&self) -> Option<String> {
        self.auction.as_ref().map(|auction| {
            auction_time::format_time_remaining(
                auction
                    .auction_end_time
                    .saturating_sub(env::block_timestamp()),
            )
        })
    }

    /// Returns the auctioneer of the active auction.
    pub fn get_auctioneer(&self) -> Option<AccountId> {
        self.auction
            .as_ref()
            .map(|auction| auction.auctioneer.clone())
    }

    /// Returns the lot description for the active auction.
    pub fn get_auction_description(&self) -> Option<String> {
        self.auction
            .as_ref()
            .map(|auction| auction.description.clone())
    }

    /// Returns the minimum bid increment for the active auction.
    pub fn get_bid_step(&self) -> Option<ft_properties::FungibleToken> {
        self.auction
            .as_ref()
            .map(|auction| auction.bid_step.clone())
    }

    /// Returns the maximum bid increment, if the round has a limit.
    pub fn get_max_bid_step(&self) -> Option<ft_properties::FungibleToken> {
        self.auction
            .as_ref()
            .and_then(|auction| auction.max_bid_step.clone())
    }

    /// Indicates whether the current auction's payout has been processed.
    pub fn is_already_claimed(&self) -> bool {
        self.auction
            .as_ref()
            .is_some_and(|auction| auction.is_claimed)
    }

    /// Returns payouts that are in progress or need to be retried.
    pub fn get_pending_payouts(&self) -> Vec<PendingPayout> {
        self.pending_payouts.clone()
    }

    /// Returns an account's auction statistics and a page of its completed-auction history.
    pub fn get_auctioneer_history(
        &self,
        auctioneer_id: AccountId,
        from_index: u64,
        limit: u32,
    ) -> AuctioneerHistory {
        require!(limit > 0, "History page limit must be greater than zero");
        let limit = limit.min(50) as u64;
        let totals = self
            .auctioneer_totals
            .get(&auctioneer_id)
            .cloned()
            .unwrap_or_default();
        let end_index = from_index.saturating_add(limit).min(totals.auction_count);
        let auctions = (from_index..end_index)
            .filter_map(|index| {
                self.auctioneer_auctions
                    .get(&(auctioneer_id.clone(), index))
                    .cloned()
            })
            .collect();

        AuctioneerHistory {
            auction_count: totals.auction_count,
            total_winning_amount: ft_properties::FungibleToken::from_params_ft(
                totals.total_winning_amount,
                self.token_decimals,
                self.token_symbol.clone(),
            ),
            total_paid_out: ft_properties::FungibleToken::from_params_ft(
                totals.total_paid_out,
                self.token_decimals,
                self.token_symbol.clone(),
            ),
            pending_payout_amount: ft_properties::FungibleToken::from_params_ft(
                totals.pending_payout_amount,
                self.token_decimals,
                self.token_symbol.clone(),
            ),
            auctions,
            next_index: (end_index < totals.auction_count).then_some(end_index),
        }
    }

    /// Updates payout history when an asynchronous transfer completes.
    #[private]
    pub fn on_payout_complete(
        &mut self,
        payout_id: u64,
        #[callback_result] result: Result<(), PromiseError>,
    ) -> bool {
        let Some(index) = self
            .pending_payouts
            .iter()
            .position(|payout| payout.id == payout_id)
        else {
            return false;
        };

        if result.is_ok() {
            let payout = self.pending_payouts[index].clone();
            let totals = self
                .auctioneer_totals
                .get_mut(&payout.receiver_id)
                .unwrap_or_else(|| env::panic_str("Auctioneer totals were not found"));
            totals.pending_payout_amount = totals
                .pending_payout_amount
                .checked_sub(payout.amount.amount())
                .unwrap_or_else(|| env::panic_str("Pending payout total is inconsistent"));
            totals.total_paid_out = totals
                .total_paid_out
                .checked_add(payout.amount.amount())
                .unwrap_or_else(|| env::panic_str("Total paid amount exceeds the maximum"));
            let auction = self
                .auctioneer_auctions
                .get_mut(&(payout.receiver_id.clone(), payout.auctioneer_index))
                .unwrap_or_else(|| env::panic_str("Auctioneer auction history was not found"));
            auction.payout_status = PayoutStatus::Paid;
            self.pending_payouts.remove(index);
            true
        } else {
            let payout = &mut self.pending_payouts[index];
            payout.in_flight = false;
            self.auctioneer_auctions
                .get_mut(&(payout.receiver_id.clone(), payout.auctioneer_index))
                .unwrap_or_else(|| env::panic_str("Auctioneer auction history was not found"))
                .payout_status = PayoutStatus::Pending;
            false
        }
    }

    /// Restores the accrued auction fees if the NEAR transfer fails.
    #[private]
    pub fn on_start_fee_withdrawal(
        &mut self,
        amount: U128,
        #[callback_result] result: Result<(), PromiseError>,
    ) -> bool {
        if result.is_err() {
            self.accrued_start_fees = self
                .accrued_start_fees
                .checked_add(amount.0)
                .unwrap_or_else(|| env::panic_str("Accumulated auction fees exceed the maximum"));
            false
        } else {
            true
        }
    }

    /// Restores the accrued FT commission if the token transfer fails.
    #[private]
    pub fn on_claim_fee_withdrawal(
        &mut self,
        amount: U128,
        #[callback_result] result: Result<(), PromiseError>,
    ) -> bool {
        if result.is_err() {
            self.accrued_claim_fees = self
                .accrued_claim_fees
                .checked_add(amount.0)
                .unwrap_or_else(|| env::panic_str("Accumulated claim fees exceed the maximum"));
            false
        } else {
            true
        }
    }
}

#[near]
impl FungibleTokenReceiver for Contract {
    /// Processes an FT transfer as a bid and returns any unused amount.
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let _ = msg;
        require!(
            env::predecessor_account_id() == self.token_contract,
            "Only the configured token contract can submit bids"
        );

        let auction = self
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
                self.token_contract.clone(),
                last_bidder,
                U128(last_bid_amount),
            );
        }

        PromiseOrValue::Value(U128(refund_amount))
    }
}

impl Contract {
    /// Records a completed auction, its commission, and the payout owed to its auctioneer.
    fn finalize_current_auction(&mut self) -> Option<Promise> {
        let (auction_id, description, auctioneer, winning_bid, auction_end_time) = {
            let auction = self
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
        let claim_fee = calculate_fee(winning_amount, self.claim_fee_basis_points);
        self.accrued_claim_fees = self
            .accrued_claim_fees
            .checked_add(claim_fee)
            .unwrap_or_else(|| env::panic_str("Accumulated claim fees exceed the maximum"));
        let payout_amount = winning_amount - claim_fee;
        let mut totals = self
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
        self.auctioneer_totals.insert(auctioneer.clone(), totals);
        self.auctioneer_auctions.insert(
            (auctioneer.clone(), auctioneer_index),
            AuctioneerAuction {
                id: auction_id,
                description,
                auction_end_time,
                winning_bid,
                claim_fee: ft_properties::FungibleToken::from_params_ft(
                    claim_fee,
                    self.token_decimals,
                    self.token_symbol.clone(),
                ),
                payout_amount: ft_properties::FungibleToken::from_params_ft(
                    payout_amount,
                    self.token_decimals,
                    self.token_symbol.clone(),
                ),
                payout_status,
            },
        );

        if payout_amount == 0 {
            return None;
        }

        let payout_id = self.next_payout_id;
        self.next_payout_id = payout_id
            .checked_add(1)
            .unwrap_or_else(|| env::panic_str("Pending payout ID overflow"));
        self.pending_payouts.push(PendingPayout {
            id: payout_id,
            auction_id,
            auctioneer_index,
            receiver_id: auctioneer,
            amount: ft_properties::FungibleToken::from_params_ft(
                payout_amount,
                self.token_decimals,
                self.token_symbol.clone(),
            ),
            in_flight: true,
        });

        Some(self.transfer_pending_payout(payout_id))
    }

    /// Creates an FT transfer for a pending payout and a callback to process its result.
    fn transfer_pending_payout(&self, payout_id: u64) -> Promise {
        let payout = self
            .pending_payouts
            .iter()
            .find(|payout| payout.id == payout_id)
            .unwrap_or_else(|| env::panic_str("Pending payout was not found"));
        transfer_tokens(
            self.token_contract.clone(),
            payout.receiver_id.clone(),
            U128(payout.amount.amount()),
        )
        .then(self.callback(
            "on_payout_complete",
            near_sdk::serde_json::json!({ "payout_id": payout_id }),
        ))
    }

    /// Creates a callback call to this contract with the supplied JSON payload.
    fn callback(&self, method: &str, args: near_sdk::serde_json::Value) -> Promise {
        Promise::new(env::current_account_id()).function_call(
            method.to_string(),
            near_sdk::serde_json::to_vec(&args).expect("Failed to serialize callback arguments"),
            NearToken::from_yoctonear(0),
            Gas::from_tgas(10),
        )
    }

    /// Verifies that the caller is the contract owner.
    fn assert_owner(&self) {
        require!(
            env::predecessor_account_id() == self.owner_id,
            "Only the contract owner can withdraw fees"
        );
    }
}

/// Calculates a fee in basis points without overflowing intermediate multiplication.
fn calculate_fee(amount: u128, fee_basis_points: u16) -> u128 {
    let fee_basis_points = u128::from(fee_basis_points);
    (amount / 10_000) * fee_basis_points + ((amount % 10_000) * fee_basis_points) / 10_000
}

/// Creates a standard NEP-141 `ft_transfer` call.
fn transfer_tokens(token_contract: AccountId, receiver_id: AccountId, amount: U128) -> Promise {
    let args = near_sdk::serde_json::json!({
        "receiver_id": receiver_id,
        "amount": amount,
        "memo": Option::<String>::None,
    });
    Promise::new(token_contract).function_call(
        "ft_transfer".to_string(),
        near_sdk::serde_json::to_vec(&args).expect("Failed to serialize token transfer arguments"),
        NearToken::from_yoctonear(1),
        Gas::from_tgas(10),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::{VMContextBuilder, accounts};
    use near_sdk::testing_env;

    // Returns the test FT contract account ID.
    fn token_contract() -> AccountId {
        "usdn.testnet".parse().unwrap()
    }

    // Creates a test context with the specified predecessor account.
    fn get_context(predecessor_account_id: AccountId) -> VMContextBuilder {
        let mut builder = VMContextBuilder::new();
        builder
            .current_account_id(accounts(0))
            .signer_account_id(predecessor_account_id.clone())
            .predecessor_account_id(predecessor_account_id);
        builder
    }

    // Initializes the contract and starts a test auction.
    fn new_contract() -> Contract {
        testing_env!(get_context(accounts(1)).build());
        let mut contract = Contract::init(
            token_contract(),
            "USN".to_string(),
            18,
            "0 NEAR".to_string(),
            200,
        );
        testing_env!(get_context(accounts(2)).build());
        let _ = contract.start_new_auction(
            "1 second".to_string(),
            "Test lot".to_string(),
            ft_properties::FungibleToken::from_params_ft(1, 0, "USN".to_string()),
            Some(ft_properties::FungibleToken::from_params_ft(
                30,
                0,
                "USN".to_string(),
            )),
        );
        contract
    }

    // Simulates a NEP-141 bid callback and returns the refunded amount.
    fn receive_bid(contract: &mut Contract, bidder: AccountId, amount: u128) -> u128 {
        let mut context = get_context(token_contract());
        context.block_timestamp(500_000_000);
        testing_env!(context.build());
        match contract.ft_on_transfer(bidder, U128(amount), String::new()) {
            PromiseOrValue::Value(refund) => refund.0,
            PromiseOrValue::Promise(_) => panic!("Expected an immediate FT refund amount"),
        }
    }

    #[test]
    // Verifies token and fee configuration without an active auction.
    fn initializes_token_and_fee_configuration_without_starting_an_auction() {
        testing_env!(get_context(accounts(1)).build());
        let contract = Contract::init(
            token_contract(),
            "USN".to_string(),
            18,
            "0.1 NEAR".parse().unwrap(),
            250,
        );

        assert_eq!(contract.get_token_contract(), token_contract());
        assert_eq!(contract.get_token_symbol(), "USN");
        assert_eq!(contract.get_token_decimals(), 18);
        assert_eq!(
            contract.get_auction_start_fee(),
            NearToken::from_millinear(100)
        );
        assert_eq!(contract.get_claim_fee_basis_points(), 250);
        assert_eq!(contract.get_contract_owner(), accounts(1));
        assert!(contract.get_highest_bid().is_none());
    }

    #[test]
    // Verifies that the caller starting a round becomes its auctioneer.
    fn starts_auction_with_new_callers_as_auctioneer() {
        let contract = new_contract();

        let highest_bid = contract.get_highest_bid().unwrap();
        assert_eq!(highest_bid.bidder, env::current_account_id());
        assert_eq!(highest_bid.bid.to_string(), "0 USN");
        assert_eq!(contract.get_token_contract(), token_contract());
        assert_eq!(contract.get_auctioneer(), Some(accounts(2)));
        assert_eq!(
            contract.get_auction_description(),
            Some("Test lot".to_string())
        );
        assert_eq!(contract.get_bid_step().unwrap().to_string(), "1 USN");
        assert_eq!(contract.get_max_bid_step().unwrap().to_string(), "30 USN");
    }

    #[test]
    // Verifies the first bid and a subsequent increase by the same bidder.
    fn accepts_first_bid_and_same_bidder_increment() {
        let mut contract = new_contract();
        let alice = accounts(3);

        assert_eq!(
            receive_bid(&mut contract, alice.clone(), 20_000_000_000_000_000_000),
            0
        );
        assert_eq!(
            contract.get_highest_bid().unwrap().bid.to_string(),
            "20 USN"
        );

        assert_eq!(
            receive_bid(&mut contract, alice.clone(), 10_000_000_000_000_000_000),
            0
        );
        let highest_bid = contract.get_highest_bid().unwrap();
        assert_eq!(highest_bid.bidder, alice);
        assert_eq!(highest_bid.bid.to_string(), "30 USN");
    }

    #[test]
    // Verifies that a higher bid from a new bidder is accepted.
    fn accepts_higher_bid_from_new_bidder() {
        let mut contract = new_contract();
        let alice = accounts(3);
        let bob = accounts(4);

        let _ = receive_bid(&mut contract, alice, 20_000_000_000_000_000_000);
        let _ = receive_bid(&mut contract, bob.clone(), 30_000_000_000_000_000_000);

        let highest_bid = contract.get_highest_bid().unwrap();
        assert_eq!(highest_bid.bidder, bob);
        assert_eq!(highest_bid.bid.to_string(), "30 USN");
    }

    #[test]
    #[should_panic(expected = "Only the configured token contract can submit bids")]
    // Rejects an attempt to submit a bid through an unconfigured token contract.
    fn rejects_bids_from_other_tokens() {
        let mut contract = new_contract();
        testing_env!(
            get_context(accounts(5))
                .block_timestamp(500_000_000)
                .build()
        );
        let _ = contract.ft_on_transfer(accounts(3), U128(20), String::new());
    }

    #[test]
    // Verifies that a bid below the minimum increment is fully refunded.
    fn refunds_bid_below_minimum_increment_without_changing_highest_bid() {
        let mut contract = new_contract();
        let alice = accounts(3);
        let bob = accounts(4);
        let _ = receive_bid(&mut contract, alice.clone(), 20_000_000_000_000_000_000);

        let refund = receive_bid(&mut contract, bob, 20_900_000_000_000_000_000);
        assert_eq!(refund, 20_900_000_000_000_000_000);
        let highest_bid = contract.get_highest_bid().unwrap();
        assert_eq!(highest_bid.bidder, alice);
        assert_eq!(highest_bid.bid.to_string(), "20 USN");
    }

    #[test]
    // Verifies the maximum increment limit and refund of excess funds.
    fn caps_highest_bid_and_refunds_payment_excess() {
        let context = get_context(accounts(1));
        testing_env!(context.build());
        let mut contract = Contract::init(
            token_contract(),
            "USN".to_string(),
            18,
            "0 NEAR".to_string(),
            0,
        );

        testing_env!(get_context(accounts(2)).build());
        let _ = contract.start_new_auction(
            "1 second".to_string(),
            "Test lot".to_string(),
            ft_properties::FungibleToken::from_params_ft(3, 0, "USN".to_string()),
            Some(ft_properties::FungibleToken::from_params_ft(
                10,
                0,
                "USN".to_string(),
            )),
        );
        let alice = accounts(3);
        let bob = accounts(4);
        let carol = accounts(5);
        let usn = 1_000_000_000_000_000_000;

        assert_eq!(receive_bid(&mut contract, alice, 5 * usn), 0);
        assert_eq!(contract.get_highest_bid().unwrap().bid.to_string(), "5 USN");

        assert_eq!(receive_bid(&mut contract, bob, 7 * usn), 7 * usn);
        assert_eq!(contract.get_highest_bid().unwrap().bid.to_string(), "5 USN");

        assert_eq!(receive_bid(&mut contract, carol, 25 * usn), 10 * usn);
        assert_eq!(
            contract.get_highest_bid().unwrap().bid.to_string(),
            "15 USN"
        );
    }

    #[test]
    // Verifies that bids are accepted without a configured maximum increment.
    fn accepts_bids_without_a_maximum_bid_step() {
        let mut contract = Contract::init(
            token_contract(),
            "USN".to_string(),
            18,
            "0 NEAR".to_string(),
            0,
        );
        testing_env!(get_context(accounts(2)).build());
        let _ = contract.start_new_auction(
            "1 hour".to_string(),
            "Uncapped lot".to_string(),
            ft_properties::FungibleToken::from_params_ft(3, 0, "USN".to_string()),
            None,
        );

        assert_eq!(contract.get_max_bid_step(), None);
        let alice = accounts(3);
        let bob = accounts(4);
        let usn = 1_000_000_000_000_000_000;

        assert_eq!(receive_bid(&mut contract, alice, 5 * usn), 0);
        assert_eq!(receive_bid(&mut contract, bob.clone(), 25 * usn), 0);
        assert_eq!(
            contract.get_highest_bid().unwrap().bid.to_string(),
            "25 USN"
        );
        assert_eq!(receive_bid(&mut contract, bob, 50 * usn), 0);
        assert_eq!(
            contract.get_highest_bid().unwrap().bid.to_string(),
            "75 USN"
        );
    }

    #[test]
    #[should_panic(expected = "Auction has ended")]
    // Rejects bids after the auction has ended.
    fn rejects_bids_after_auction_end() {
        let mut contract = new_contract();
        let mut context = get_context(token_contract());
        context.block_timestamp(1_000_000_000);
        testing_env!(context.build());
        let _ = contract.ft_on_transfer(accounts(3), U128(20), String::new());
    }

    #[test]
    // Verifies commission accounting and payout creation after a claim.
    fn claim_transfers_winning_amount_to_auctioneer() {
        let mut contract = new_contract();
        let alice = accounts(3);
        let _ = receive_bid(&mut contract, alice, 20_000_000_000_000_000_000);

        let mut context = get_context(accounts(2));
        context.block_timestamp(1_001_000_000);
        testing_env!(context.build());
        let _ = contract.claim();

        assert!(contract.is_already_claimed());
        assert_eq!(contract.get_accrued_claim_fees().to_string(), "0.4 USN");
        let pending_payouts = contract.get_pending_payouts();
        assert_eq!(pending_payouts.len(), 1);
        assert_eq!(pending_payouts[0].receiver_id, accounts(2));
        assert_eq!(pending_payouts[0].amount.to_string(), "19.6 USN");
    }

    #[test]
    // Verifies payout persistence, retries, successful completion, and history pagination.
    fn failed_payout_remains_retryable() {
        let mut contract = new_contract();
        let _ = receive_bid(&mut contract, accounts(3), 20_000_000_000_000_000_000);

        let mut context = get_context(accounts(2));
        context.block_timestamp(1_001_000_000);
        testing_env!(context.build());
        let _ = contract.claim();

        assert!(!contract.on_payout_complete(0, Err(PromiseError::Failed)));
        assert_eq!(contract.get_pending_payouts().len(), 1);
        assert!(!contract.get_pending_payouts()[0].in_flight);

        let history = contract.get_auctioneer_history(accounts(2), 0, 10);
        assert_eq!(history.auction_count, 1);
        assert_eq!(history.total_winning_amount.to_string(), "20 USN");
        assert_eq!(history.total_paid_out.to_string(), "0 USN");
        assert_eq!(history.pending_payout_amount.to_string(), "19.6 USN");
        assert_eq!(history.auctions[0].winning_bid.bid.to_string(), "20 USN");
        assert_eq!(history.auctions[0].payout_status, PayoutStatus::Pending);

        let _ = contract.retry_pending_payout(0);
        assert!(contract.get_pending_payouts()[0].in_flight);
        assert_eq!(
            contract.get_auctioneer_history(accounts(2), 0, 10).auctions[0].payout_status,
            PayoutStatus::InFlight
        );
        assert!(contract.on_payout_complete(0, Ok(())));

        let mut context = get_context(accounts(2));
        context.block_timestamp(1_002_000_000);
        testing_env!(context.build());
        let _ = contract.start_new_auction(
            "1 second".to_string(),
            "Second lot".to_string(),
            ft_properties::FungibleToken::from_params_ft(1, 0, "USN".to_string()),
            Some(ft_properties::FungibleToken::from_params_ft(
                10,
                0,
                "USN".to_string(),
            )),
        );
        context = get_context(accounts(2));
        context.block_timestamp(2_003_000_000);
        testing_env!(context.build());
        let _ = contract.claim();

        let history = contract.get_auctioneer_history(accounts(2), 0, 1);
        assert_eq!(history.auction_count, 2);
        assert_eq!(history.total_winning_amount.to_string(), "20 USN");
        assert_eq!(history.total_paid_out.to_string(), "19.6 USN");
        assert_eq!(history.pending_payout_amount.to_string(), "0 USN");
        assert_eq!(history.auctions.len(), 1);
        assert_eq!(history.auctions[0].payout_status, PayoutStatus::Paid);
        assert_eq!(history.next_index, Some(1));

        let second_page = contract.get_auctioneer_history(accounts(2), 1, 10);
        assert_eq!(second_page.auctions.len(), 1);
        assert_eq!(
            second_page.auctions[0].payout_status,
            PayoutStatus::NotRequired
        );
        assert_eq!(second_page.next_index, None);

        let other_auctioneer = contract.get_auctioneer_history(accounts(4), 0, 10);
        assert_eq!(other_auctioneer.auction_count, 0);
        assert_eq!(other_auctioneer.total_winning_amount.to_string(), "0 USN");
    }

    #[test]
    #[should_panic(expected = "Auction has not ended yet")]
    // Rejects a claim before the auction has ended.
    fn rejects_claim_before_auction_end() {
        let mut contract = new_contract();
        let mut context = get_context(accounts(2));
        context.block_timestamp(999_000_000);
        testing_env!(context.build());
        let _ = contract.claim();
    }

    #[test]
    #[should_panic(expected = "Auction has already been claimed")]
    // Rejects a second claim for the same round.
    fn rejects_second_claim() {
        let mut contract = new_contract();
        let mut context = get_context(accounts(2));
        context.block_timestamp(1_001_000_000);
        testing_env!(context.build());
        let _ = contract.claim();
        let _ = contract.claim();
    }

    #[test]
    // Verifies that starting a new round finalizes the previous one.
    fn starts_next_auction_and_claims_the_previous_auction() {
        let mut contract = new_contract();
        let alice = accounts(3);
        let _ = receive_bid(&mut contract, alice, 20_000_000_000_000_000_000);

        let mut context = get_context(accounts(4));
        context.block_timestamp(1_001_000_000);
        testing_env!(context.build());
        let _ = contract.start_new_auction(
            "2 hours".to_string(),
            "Next lot".to_string(),
            ft_properties::FungibleToken::from_params_ft(2, 0, "USN".to_string()),
            Some(ft_properties::FungibleToken::from_params_ft(
                20,
                0,
                "USN".to_string(),
            )),
        );

        assert_eq!(contract.get_auctioneer(), Some(accounts(4)));
        assert_eq!(
            contract.get_auction_description(),
            Some("Next lot".to_string())
        );
        assert_eq!(contract.get_highest_bid().unwrap().bid.to_string(), "0 USN");
        assert_eq!(contract.get_accrued_claim_fees().to_string(), "0.4 USN");
        assert_eq!(
            contract.get_pending_payouts()[0].amount.to_string(),
            "19.6 USN"
        );
        assert_eq!(contract.get_accrued_start_fees(), U128(0));
    }

    #[test]
    // Verifies the exact auction start fee and its accrual.
    fn requires_exact_start_fee_and_accrues_it_on_success() {
        testing_env!(get_context(accounts(1)).build());
        let mut contract = Contract::init(
            token_contract(),
            "USN".to_string(),
            18,
            "0.1 NEAR".parse().unwrap(),
            0,
        );
        let mut context = get_context(accounts(2));
        context.attached_deposit(NearToken::from_millinear(100));
        testing_env!(context.build());

        let _ = contract.start_new_auction(
            "1 hour".to_string(),
            "Paid lot".to_string(),
            ft_properties::FungibleToken::from_params_ft(1, 0, "USN".to_string()),
            Some(ft_properties::FungibleToken::from_params_ft(
                10,
                0,
                "USN".to_string(),
            )),
        );

        assert_eq!(
            contract.get_accrued_start_fees(),
            U128(NearToken::from_millinear(100).as_yoctonear())
        );
    }

    #[test]
    // Verifies that the contract owner can withdraw auction start fees.
    fn only_contract_owner_can_withdraw_start_fees() {
        let mut contract = new_contract();
        contract.accrued_start_fees = 100;
        testing_env!(get_context(accounts(1)).build());

        let _ = contract.withdraw_start_fees();

        assert_eq!(contract.get_accrued_start_fees(), U128(0));
    }

    #[test]
    #[should_panic(expected = "Only the contract owner can withdraw fees")]
    // Rejects auction start fee withdrawals by non-owners.
    fn rejects_start_fee_withdrawal_by_non_owner() {
        let mut contract = new_contract();
        contract.accrued_start_fees = 100;
        testing_env!(get_context(accounts(2)).build());

        let _ = contract.withdraw_start_fees();
    }

    #[test]
    #[should_panic(expected = "The current auction has not ended yet")]
    // Rejects starting a new round before the current one ends.
    fn cannot_start_next_auction_before_current_round_ends() {
        let mut contract = new_contract();
        let _ = contract.start_new_auction(
            "1 hour".to_string(),
            "Too soon".to_string(),
            ft_properties::FungibleToken::from_params_ft(1, 0, "USN".to_string()),
            Some(ft_properties::FungibleToken::from_params_ft(
                10,
                0,
                "USN".to_string(),
            )),
        );
    }

    #[test]
    #[should_panic(expected = "Claim fee must not exceed 100%")]
    // Rejects a commission greater than 100 percent.
    fn rejects_claim_fee_above_one_hundred_percent() {
        testing_env!(get_context(accounts(1)).build());
        let _ = Contract::init(
            token_contract(),
            "USN".to_string(),
            18,
            "0 NEAR".to_string(),
            10_001,
        );
    }

    #[test]
    // Verifies human-readable formatting of fractional FT amounts.
    fn formats_fractional_token_amounts() {
        assert_eq!(
            ft_properties::FungibleToken::from_params_ft(
                123_450_000_000_000_000_000,
                18,
                "USN".to_string()
            )
            .to_string(),
            "123.45 USN"
        );
    }

    #[test]
    // Verifies the auction end time is calculated from the round duration.
    fn init_sets_end_time_from_current_block_timestamp_and_duration() {
        let mut context = get_context(accounts(1));
        context.block_timestamp(5_000_000_000);
        testing_env!(context.build());

        let mut contract = Contract::init(
            token_contract(),
            "USN".to_string(),
            18,
            "0 NEAR".to_string(),
            0,
        );
        testing_env!(get_context(accounts(2)).build());
        let _ = contract.start_new_auction(
            "24 hours".to_string(),
            "Test lot".to_string(),
            ft_properties::FungibleToken::from_params_ft(1, 0, "USN".to_string()),
            Some(ft_properties::FungibleToken::from_params_ft(
                30,
                0,
                "USN".to_string(),
            )),
        );
        assert_eq!(
            contract.get_auction_time_remaining(),
            Some("1 day".to_string())
        );
    }
}
