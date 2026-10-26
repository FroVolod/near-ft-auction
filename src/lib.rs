use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::json_types::{U64, U128};
use near_sdk::{
    AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue, Timestamp, env, near,
    require,
};

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    highest_bid: StoredBid,
    token_contract: AccountId,
    bid_step: u128,
    auction_end_time: Timestamp,
    auctioneer: AccountId,
    is_claimed: bool,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
struct StoredBid {
    bidder: AccountId,
    bid: u128,
}

#[near(serializers = [json])]
#[derive(Clone)]
pub struct Bid {
    pub bidder: AccountId,
    pub bid: U128,
}

#[near]
impl Contract {
    #[init]
    pub fn init(
        end_time: U64,
        auctioneer: AccountId,
        token_contract: AccountId,
        bid_step: U128,
    ) -> Self {
        require!(bid_step.0 > 0, "Bid step must be greater than zero");

        Self {
            highest_bid: StoredBid {
                bidder: env::current_account_id(),
                bid: 0,
            },
            token_contract,
            bid_step: bid_step.0,
            auction_end_time: end_time.into(),
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
            U128(self.highest_bid.bid),
        )
    }

    pub fn get_highest_bid(&self) -> Bid {
        Bid {
            bidder: self.highest_bid.bidder.clone(),
            bid: U128(self.highest_bid.bid),
        }
    }

    pub fn get_auction_end_time(&self) -> U64 {
        self.auction_end_time.into()
    }

    pub fn get_auctioneer(&self) -> AccountId {
        self.auctioneer.clone()
    }

    pub fn get_token_contract(&self) -> AccountId {
        self.token_contract.clone()
    }

    pub fn get_bid_step(&self) -> U128 {
        U128(self.bid_step)
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
        _msg: String,
    ) -> PromiseOrValue<U128> {
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
        let is_current_bidder = sender_id == last_bidder;
        let minimum_payment = if is_current_bidder {
            self.bid_step
        } else {
            last_bid
                .checked_add(self.bid_step)
                .expect("Minimum bid exceeds the maximum token amount")
        };
        require!(
            amount.0 >= minimum_payment,
            "Bid is below the minimum increment"
        );

        let new_bid = if is_current_bidder {
            last_bid
                .checked_add(amount.0)
                .expect("Bid exceeds the maximum token amount")
        } else {
            amount.0
        };
        self.highest_bid = StoredBid {
            bidder: sender_id,
            bid: new_bid,
        };

        if !is_current_bidder && last_bid > 0 {
            // This example does not recover the refund if the asynchronous token transfer fails.
            let _ = transfer_tokens(self.token_contract.clone(), last_bidder, U128(last_bid));
        }

        PromiseOrValue::Value(U128(0))
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
        Contract::init(U64::from(1000), accounts(2), token_contract(), U128(10))
    }

    fn receive_bid(contract: &mut Contract, bidder: AccountId, amount: u128) {
        let mut context = get_context(token_contract());
        context.block_timestamp(500);
        testing_env!(context.build());
        let _ = contract.ft_on_transfer(bidder, U128(amount), String::new());
    }

    #[test]
    fn initializes_with_zero_bid_and_configured_token() {
        let contract = new_contract();

        let highest_bid = contract.get_highest_bid();
        assert_eq!(highest_bid.bidder, env::current_account_id());
        assert_eq!(highest_bid.bid, U128(0));
        assert_eq!(contract.get_token_contract(), token_contract());
        assert_eq!(contract.get_bid_step(), U128(10));
    }

    #[test]
    fn accepts_first_bid_and_same_bidder_increment() {
        let mut contract = new_contract();
        let alice = accounts(3);

        receive_bid(&mut contract, alice.clone(), 20);
        assert_eq!(contract.get_highest_bid().bid, U128(20));

        receive_bid(&mut contract, alice.clone(), 10);
        let highest_bid = contract.get_highest_bid();
        assert_eq!(highest_bid.bidder, alice);
        assert_eq!(highest_bid.bid, U128(30));
    }

    #[test]
    fn accepts_higher_bid_from_new_bidder() {
        let mut contract = new_contract();
        let alice = accounts(3);
        let bob = accounts(4);

        receive_bid(&mut contract, alice, 20);
        receive_bid(&mut contract, bob.clone(), 30);

        let highest_bid = contract.get_highest_bid();
        assert_eq!(highest_bid.bidder, bob);
        assert_eq!(highest_bid.bid, U128(30));
    }

    #[test]
    #[should_panic(expected = "Only the configured token contract can submit bids")]
    fn rejects_bids_from_other_tokens() {
        let mut contract = new_contract();
        testing_env!(get_context(accounts(5)).block_timestamp(500).build());
        let _ = contract.ft_on_transfer(accounts(3), U128(20), String::new());
    }

    #[test]
    #[should_panic(expected = "Bid is below the minimum increment")]
    fn rejects_bid_below_minimum_increment() {
        let mut contract = new_contract();
        receive_bid(&mut contract, accounts(3), 20);
        receive_bid(&mut contract, accounts(4), 29);
    }

    #[test]
    #[should_panic(expected = "Auction has ended")]
    fn rejects_bids_after_auction_end() {
        let mut contract = new_contract();
        let mut context = get_context(token_contract());
        context.block_timestamp(1000);
        testing_env!(context.build());
        let _ = contract.ft_on_transfer(accounts(3), U128(20), String::new());
    }

    #[test]
    fn claim_transfers_winning_amount_to_auctioneer() {
        let mut contract = new_contract();
        let alice = accounts(3);
        receive_bid(&mut contract, alice, 20);

        let mut context = get_context(accounts(2));
        context.block_timestamp(1001);
        testing_env!(context.build());
        let _ = contract.claim();

        assert!(contract.is_already_claimed());
    }

    #[test]
    #[should_panic(expected = "Auction has not ended yet")]
    fn rejects_claim_before_auction_end() {
        let mut contract = new_contract();
        let mut context = get_context(accounts(2));
        context.block_timestamp(999);
        testing_env!(context.build());
        let _ = contract.claim();
    }

    #[test]
    #[should_panic(expected = "Auction has already been claimed")]
    fn rejects_second_claim() {
        let mut contract = new_contract();
        let mut context = get_context(accounts(2));
        context.block_timestamp(1001);
        testing_env!(context.build());
        let _ = contract.claim();
        let _ = contract.claim();
    }
}
