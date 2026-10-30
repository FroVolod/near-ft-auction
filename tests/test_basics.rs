use near_api::{AccountId, NearToken, Tokens};
use near_sdk::serde_json::json;

#[derive(near_sdk::serde::Deserialize)]
#[serde(crate = "near_sdk::serde")]
struct Bid {
    bidder: AccountId,
    bid: String,
}

#[derive(near_sdk::serde::Deserialize)]
#[serde(crate = "near_sdk::serde")]
struct Auction {
    description: String,
    auctioneer: AccountId,
    highest_bid: Bid,
    bid_step: String,
    max_bid_step: Option<String>,
    auction_start_time: String,
    auction_end_time: String,
    time_remaining: String,
    is_claimed: bool,
}

async fn test_contract_on_sandbox(contract_wasm: Vec<u8>) -> testresult::TestResult<()> {
    let sandbox = near_sandbox::Sandbox::start_sandbox().await?;
    let sandbox_network =
        near_api::NetworkConfig::from_rpc_url("sandbox", sandbox.rpc_addr.parse()?);

    let alice = create_subaccount(&sandbox, "alice.sandbox").await?;
    let contract = create_subaccount(&sandbox, "contract.sandbox")
        .await?
        .as_contract();
    let token_contract: AccountId = "usdn.testnet".parse().unwrap();
    let metadata = Tokens::ft_metadata(token_contract.clone())
        .fetch_from_testnet()
        .await?
        .data;

    let signer = near_api::Signer::from_secret_key(
        near_sandbox::config::DEFAULT_GENESIS_ACCOUNT_PRIVATE_KEY
            .parse()
            .unwrap(),
    )?;
    near_api::Contract::deploy(contract.account_id().clone())
        .use_code(contract_wasm)
        .with_init_call(
            "init",
            json!({
                "token_contract": token_contract,
                "token_symbol": metadata.symbol,
                "token_decimals": metadata.decimals,
                "auction_start_fee": "0.1 NEAR",
                "claim_fee_basis_points": 200,
            }),
        )?
        .with_signer(signer.clone())
        .send_to(&sandbox_network)
        .await?
        .assert_success();

    contract
        .call_function(
            "start_new_auction",
            json!({
                "auction_duration": "24 hours",
                "description": "Sandbox test lot",
                "bid_step": format!("1 {}", metadata.symbol),
            }),
        )
        .transaction()
        .deposit(NearToken::from_millinear(100))
        .with_signer(
            near_sandbox::config::DEFAULT_GENESIS_ACCOUNT
                .as_str()
                .parse()
                .unwrap(),
            signer.clone(),
        )
        .send_to(&sandbox_network)
        .await?
        .assert_success();

    let highest_bid: Option<Bid> = contract
        .call_function("get_highest_bid", ())
        .read_only()
        .fetch_from(&sandbox_network)
        .await?
        .data;
    let highest_bid = highest_bid.expect("start_new_auction should create a highest bid");
    assert_eq!(&highest_bid.bidder, contract.account_id());
    assert_eq!(highest_bid.bid, format!("0 {}", metadata.symbol));

    let bid_step: Option<String> = contract
        .call_function("get_bid_step", ())
        .read_only()
        .fetch_from(&sandbox_network)
        .await?
        .data;
    assert_eq!(bid_step, Some(format!("1 {}", metadata.symbol)));

    let max_bid_step: Option<String> = contract
        .call_function("get_max_bid_step", ())
        .read_only()
        .fetch_from(&sandbox_network)
        .await?
        .data;
    assert_eq!(max_bid_step, None);

    let time_remaining: Option<String> = contract
        .call_function("get_auction_time_remaining", ())
        .read_only()
        .fetch_from(&sandbox_network)
        .await?
        .data;
    let time_remaining = time_remaining.expect("auction should have a remaining time");
    assert!(
        time_remaining.contains("day")
            || time_remaining.contains("hour")
            || time_remaining.contains("minute"),
        "Expected readable time remaining, got {time_remaining}"
    );

    let auction: Option<Auction> = contract
        .call_function("get_auction_status", ())
        .read_only()
        .fetch_from(&sandbox_network)
        .await?
        .data;
    let auction = auction.expect("get_auction_status should return the active auction");
    assert_eq!(auction.description, "Sandbox test lot");
    assert_eq!(
        auction.auctioneer.as_str(),
        near_sandbox::config::DEFAULT_GENESIS_ACCOUNT.as_str()
    );
    assert_eq!(&auction.highest_bid.bidder, contract.account_id());
    assert_eq!(auction.highest_bid.bid, format!("0 {}", metadata.symbol));
    assert_eq!(auction.bid_step, format!("1 {}", metadata.symbol));
    assert_eq!(auction.max_bid_step, None);
    assert!(auction.auction_start_time.ends_with('Z'));
    assert!(auction.auction_start_time.contains('T'));
    assert!(auction.auction_end_time.ends_with('Z'));
    assert!(auction.auction_end_time.contains('T'));
    assert!(auction.auction_start_time < auction.auction_end_time);
    assert!(!auction.time_remaining.is_empty());
    assert!(!auction.is_claimed);

    let auctioneer_history: near_sdk::serde_json::Value = contract
        .call_function(
            "get_auctioneer_history",
            json!({
                "auctioneer_id": near_sandbox::config::DEFAULT_GENESIS_ACCOUNT.as_str(),
                "from_index": 0,
                "limit": 10,
            }),
        )
        .read_only()
        .fetch_from(&sandbox_network)
        .await?
        .data;
    assert_eq!(auctioneer_history["auction_count"], 0);
    assert_eq!(
        auctioneer_history["total_winning_amount"],
        "0 ".to_string() + &metadata.symbol
    );
    assert_eq!(auctioneer_history["auctions"].as_array().unwrap().len(), 0);

    let configured_token: AccountId = contract
        .call_function("get_token_contract", ())
        .read_only()
        .fetch_from(&sandbox_network)
        .await?
        .data;
    assert_eq!(configured_token.as_str(), "usdn.testnet");

    // A bidder cannot invoke the NEP-141 receiver method directly.
    contract
        .call_function(
            "ft_on_transfer",
            json!({
                "sender_id": alice.account_id(),
                "amount": "200",
                "msg": "",
            }),
        )
        .transaction()
        .with_signer(alice.account_id().clone(), signer)
        .send_to(&sandbox_network)
        .await?
        .assert_failure();

    Ok(())
}

async fn create_subaccount(
    sandbox: &near_sandbox::Sandbox,
    name: &str,
) -> testresult::TestResult<near_api::Account> {
    let account_id: AccountId = name.parse().unwrap();
    sandbox
        .create_account(account_id.clone())
        .initial_balance(NearToken::from_near(10))
        .send()
        .await?;
    Ok(near_api::Account(account_id))
}

#[tokio::test]
async fn test_contract_initializes_with_configured_token() -> testresult::TestResult<()> {
    let contract_wasm_path = cargo_near_build::build_with_cli(Default::default())?;
    let contract_wasm = std::fs::read(contract_wasm_path)?;

    test_contract_on_sandbox(contract_wasm).await
}
