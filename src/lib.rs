use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::json_types::U128;
use near_sdk::store::LookupMap;
use near_sdk::{
    AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError, PromiseOrValue, Timestamp,
    env, near, require,
};

mod auction;
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
        auction::start_new_auction(self, auction_duration, description, bid_step, max_bid_step)
    }

    /// Finalizes the current auction and initiates the proceeds transfer to its auctioneer.
    pub fn claim(&mut self) -> PromiseOrValue<bool> {
        let promise = auction::finalize_current_auction(self);
        match promise {
            Some(promise) => PromiseOrValue::Promise(promise),
            None => PromiseOrValue::Value(true),
        }
    }

    /// Retries a payout that previously failed.
    pub fn retry_pending_payout(&mut self, payout_id: u64) -> Promise {
        auction::retry_pending_payout(self, payout_id)
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
        auction::receive_bid(self, sender_id, amount, msg)
    }
}

impl Contract {
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
mod tests;
