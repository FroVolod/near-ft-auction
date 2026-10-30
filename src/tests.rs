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
