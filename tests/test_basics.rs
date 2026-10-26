use near_api::{AccountId, NearToken, Tokens};
use near_sdk::serde_json::json;

#[derive(near_sdk::serde::Deserialize)]
#[serde(crate = "near_sdk::serde")]
struct Bid {
    bidder: AccountId,
    bid: String,
}

async fn test_contract_on_sandbox(contract_wasm: Vec<u8>) -> testresult::TestResult<()> {
    let sandbox = near_sandbox::Sandbox::start_sandbox().await?;
    let sandbox_network =
        near_api::NetworkConfig::from_rpc_url("sandbox", sandbox.rpc_addr.parse()?);

    let auctioneer = create_subaccount(&sandbox, "auctioneer.sandbox").await?;
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
                "auction_duration": "24 hours",
                "auctioneer": auctioneer.account_id(),
                "token_contract": token_contract,
                "token_symbol": metadata.symbol,
                "token_decimals": metadata.decimals,
                "bid_step": format!("1 {}", metadata.symbol),
            }),
        )?
        .with_signer(signer.clone())
        .send_to(&sandbox_network)
        .await?
        .assert_success();

    let highest_bid: Bid = contract
        .call_function("get_highest_bid", ())
        .read_only()
        .fetch_from(&sandbox_network)
        .await?
        .data;
    assert_eq!(&highest_bid.bidder, contract.account_id());
    assert_eq!(highest_bid.bid, format!("0 {}", metadata.symbol));

    let bid_step: String = contract
        .call_function("get_bid_step", ())
        .read_only()
        .fetch_from(&sandbox_network)
        .await?
        .data;
    assert_eq!(bid_step, format!("1 {}", metadata.symbol));

    let time_remaining: String = contract
        .call_function("get_auction_time_remaining", ())
        .read_only()
        .fetch_from(&sandbox_network)
        .await?
        .data;
    assert!(
        time_remaining.contains("day")
            || time_remaining.contains("hour")
            || time_remaining.contains("minute"),
        "Expected readable time remaining, got {time_remaining}"
    );

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
