use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::json_types::U128;
use near_sdk::{
    AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue, Timestamp, env, near,
    require,
};

mod auction_time;
mod ft_properties;

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    highest_bid: StoredBid,
    token_contract: AccountId,
    bid_step: ft_properties::FungibleToken,
    max_bid_step: ft_properties::FungibleToken,
    auction_end_time: Timestamp,
    auctioneer: AccountId,
    is_claimed: bool,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
struct StoredBid {
    bidder: AccountId,
    bid: ft_properties::FungibleToken,
}

#[near(serializers = [json])]
#[derive(Clone)]
pub struct Bid {
    pub bidder: AccountId,
    pub bid: ft_properties::FungibleToken,
}

#[near]
impl Contract {
    #[init]
    pub fn init(
        auction_duration: String,
        auctioneer: AccountId,
        token_contract: AccountId,
        token_symbol: String,
        token_decimals: u8,
        bid_step: ft_properties::FungibleToken,
        max_bid_step: ft_properties::FungibleToken,
    ) -> Self {
        require!(!token_symbol.is_empty(), "Token symbol cannot be empty");
        let duration = auction_time::parse_duration(&auction_duration)
            .unwrap_or_else(|error| env::panic_str(&error));
        let auction_end_time = env::block_timestamp()
            .checked_add(duration)
            .unwrap_or_else(|| env::panic_str("Auction end time exceeds the maximum timestamp"));
        let bid_step = bid_step
            .normalize(token_decimals, &token_symbol)
            .unwrap_or_else(|error| env::panic_str(&error));
        require!(bid_step.amount() > 0, "Bid step must be greater than zero");
        let max_bid_step = max_bid_step
            .normalize(token_decimals, &token_symbol)
            .unwrap_or_else(|error| env::panic_str(&error));
        require!(
            max_bid_step.amount() >= bid_step.amount(),
            "Maximum bid step must not be less than the minimum bid step"
        );

        Self {
            highest_bid: StoredBid {
                bidder: env::current_account_id(),
                bid: bid_step.with_amount(0),
            },
            token_contract,
            bid_step,
            max_bid_step,
            auction_end_time,
            is_claimed: false,
            auctioneer,
        }
    }

    pub fn claim(&mut self) -> Promise {
        require!(
            env::block_timestamp() > self.auction_end_time,
            "Auction has not ended yet"
        );
        require!(!self.is_claimed, "Auction has already been claimed");
        self.is_claimed = true;

        // This example does not restore the claim if the asynchronous token transfer fails.
        transfer_tokens(
            self.token_contract.clone(),
            self.auctioneer.clone(),
            U128(self.highest_bid.bid.amount()),
        )
    }

    pub fn get_highest_bid(&self) -> Bid {
        Bid {
            bidder: self.highest_bid.bidder.clone(),
            bid: self.highest_bid.bid.clone(),
        }
    }

    pub fn get_auction_time_remaining(&self) -> String {
        auction_time::format_time_remaining(
            self.auction_end_time.saturating_sub(env::block_timestamp()),
        )
    }

    pub fn get_auctioneer(&self) -> AccountId {
        self.auctioneer.clone()
    }

    pub fn get_token_contract(&self) -> AccountId {
        self.token_contract.clone()
    }

    pub fn get_bid_step(&self) -> ft_properties::FungibleToken {
        self.bid_step.clone()
    }

    pub fn get_max_bid_step(&self) -> ft_properties::FungibleToken {
        self.max_bid_step.clone()
    }

    pub fn is_already_claimed(&self) -> bool {
        self.is_claimed
    }
}

#[near]
impl FungibleTokenReceiver for Contract {
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
        require!(
            env::block_timestamp() < self.auction_end_time,
            "Auction has ended"
        );

        let StoredBid {
            bidder: last_bidder,
            bid: last_bid,
        } = self.highest_bid.clone();
        let last_bid_amount = last_bid.amount();
        let bid_step_amount = self.bid_step.amount();
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

        let accepted_amount = if is_current_bidder {
            amount.0.min(self.max_bid_step.amount())
        } else {
            let maximum_bid = last_bid_amount
                .checked_add(self.max_bid_step.amount())
                .expect("Maximum bid exceeds the maximum token amount");
            amount.0.min(maximum_bid)
        };
        let refund_amount = amount.0 - accepted_amount;

        let new_bid = if is_current_bidder {
            last_bid_amount
                .checked_add(accepted_amount)
                .expect("Bid exceeds the maximum token amount")
        } else {
            accepted_amount
        };
        self.highest_bid = StoredBid {
            bidder: sender_id,
            bid: self.bid_step.with_amount(new_bid),
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

    fn token_contract() -> AccountId {
        "usdn.testnet".parse().unwrap()
    }

    fn get_context(predecessor_account_id: AccountId) -> VMContextBuilder {
        let mut builder = VMContextBuilder::new();
        builder
            .current_account_id(accounts(0))
            .signer_account_id(predecessor_account_id.clone())
            .predecessor_account_id(predecessor_account_id);
        builder
    }

    fn new_contract() -> Contract {
        testing_env!(get_context(accounts(1)).build());
        Contract::init(
            "1 second".to_string(),
            accounts(2),
            token_contract(),
            "USN".to_string(),
            18,
            ft_properties::FungibleToken::from_params_ft(1, 0, "USN".to_string()),
            ft_properties::FungibleToken::from_params_ft(30, 0, "USN".to_string()),
        )
    }

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
    fn initializes_with_zero_bid_and_configured_token() {
        let contract = new_contract();

        let highest_bid = contract.get_highest_bid();
        assert_eq!(highest_bid.bidder, env::current_account_id());
        assert_eq!(highest_bid.bid.to_string(), "0 USN");
        assert_eq!(contract.get_token_contract(), token_contract());
        assert_eq!(contract.get_bid_step().to_string(), "1 USN");
        assert_eq!(contract.get_max_bid_step().to_string(), "30 USN");
    }

    #[test]
    fn accepts_first_bid_and_same_bidder_increment() {
        let mut contract = new_contract();
        let alice = accounts(3);

        assert_eq!(
            receive_bid(&mut contract, alice.clone(), 20_000_000_000_000_000_000),
            0
        );
        assert_eq!(contract.get_highest_bid().bid.to_string(), "20 USN");

        assert_eq!(
            receive_bid(&mut contract, alice.clone(), 10_000_000_000_000_000_000),
            0
        );
        let highest_bid = contract.get_highest_bid();
        assert_eq!(highest_bid.bidder, alice);
        assert_eq!(highest_bid.bid.to_string(), "30 USN");
    }

    #[test]
    fn accepts_higher_bid_from_new_bidder() {
        let mut contract = new_contract();
        let alice = accounts(3);
        let bob = accounts(4);

        let _ = receive_bid(&mut contract, alice, 20_000_000_000_000_000_000);
        let _ = receive_bid(&mut contract, bob.clone(), 30_000_000_000_000_000_000);

        let highest_bid = contract.get_highest_bid();
        assert_eq!(highest_bid.bidder, bob);
        assert_eq!(highest_bid.bid.to_string(), "30 USN");
    }

    #[test]
    #[should_panic(expected = "Only the configured token contract can submit bids")]
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
    fn refunds_bid_below_minimum_increment_without_changing_highest_bid() {
        let mut contract = new_contract();
        let alice = accounts(3);
        let bob = accounts(4);
        let _ = receive_bid(&mut contract, alice.clone(), 20_000_000_000_000_000_000);

        let refund = receive_bid(&mut contract, bob, 20_900_000_000_000_000_000);
        assert_eq!(refund, 20_900_000_000_000_000_000);
        let highest_bid = contract.get_highest_bid();
        assert_eq!(highest_bid.bidder, alice);
        assert_eq!(highest_bid.bid.to_string(), "20 USN");
    }

    #[test]
    fn caps_highest_bid_and_refunds_payment_excess() {
        let context = get_context(accounts(1));
        testing_env!(context.build());
        let mut contract = Contract::init(
            "1 second".to_string(),
            accounts(2),
            token_contract(),
            "USN".to_string(),
            18,
            ft_properties::FungibleToken::from_params_ft(3, 0, "USN".to_string()),
            ft_properties::FungibleToken::from_params_ft(10, 0, "USN".to_string()),
        );

        let alice = accounts(3);
        let bob = accounts(4);
        let carol = accounts(5);
        let usn = 1_000_000_000_000_000_000;

        assert_eq!(receive_bid(&mut contract, alice, 5 * usn), 0);
        assert_eq!(contract.get_highest_bid().bid.to_string(), "5 USN");

        assert_eq!(receive_bid(&mut contract, bob, 7 * usn), 7 * usn);
        assert_eq!(contract.get_highest_bid().bid.to_string(), "5 USN");

        assert_eq!(receive_bid(&mut contract, carol, 25 * usn), 10 * usn);
        assert_eq!(contract.get_highest_bid().bid.to_string(), "15 USN");
    }

    #[test]
    #[should_panic(expected = "Auction has ended")]
    fn rejects_bids_after_auction_end() {
        let mut contract = new_contract();
        let mut context = get_context(token_contract());
        context.block_timestamp(1_000_000_000);
        testing_env!(context.build());
        let _ = contract.ft_on_transfer(accounts(3), U128(20), String::new());
    }

    #[test]
    fn claim_transfers_winning_amount_to_auctioneer() {
        let mut contract = new_contract();
        let alice = accounts(3);
        let _ = receive_bid(&mut contract, alice, 20_000_000_000_000_000_000);

        let mut context = get_context(accounts(2));
        context.block_timestamp(1_001_000_000);
        testing_env!(context.build());
        let _ = contract.claim();

        assert!(contract.is_already_claimed());
    }

    #[test]
    #[should_panic(expected = "Auction has not ended yet")]
    fn rejects_claim_before_auction_end() {
        let mut contract = new_contract();
        let mut context = get_context(accounts(2));
        context.block_timestamp(999_000_000);
        testing_env!(context.build());
        let _ = contract.claim();
    }

    #[test]
    #[should_panic(expected = "Auction has already been claimed")]
    fn rejects_second_claim() {
        let mut contract = new_contract();
        let mut context = get_context(accounts(2));
        context.block_timestamp(1_001_000_000);
        testing_env!(context.build());
        let _ = contract.claim();
        let _ = contract.claim();
    }

    #[test]
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
    fn init_sets_end_time_from_current_block_timestamp_and_duration() {
        let mut context = get_context(accounts(1));
        context.block_timestamp(5_000_000_000);
        testing_env!(context.build());

        let contract = Contract::init(
            "24 hours".to_string(),
            accounts(2),
            token_contract(),
            "USN".to_string(),
            18,
            ft_properties::FungibleToken::from_params_ft(1, 0, "USN".to_string()),
            ft_properties::FungibleToken::from_params_ft(30, 0, "USN".to_string()),
        );

        assert_eq!(contract.get_auction_time_remaining(), "1 day");
    }
}
