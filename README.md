# Basic Auction Contract 

This directory contains a Rust contract that is used as part of the [Basic Auction Tutorial](https://docs.near.org/tutorials/auction/basic-auction).

The contract is a simple auction where you can place bids, view the highest bid, and claim the tokens at the end of the auction.

This repo showcases the basic anatomy of a smart contract application on NEAR Protocol that showcases how to store data in a contract, how to update the state, and then how to read it.
There are also unit tests, integration tests, and CI/CD pipelines preconfigured for your reference.

---

## How to Build Locally?

Install [`cargo-near`](https://github.com/near/cargo-near) and run:

```bash
cargo near build
```

## How to Test Locally?

```bash
cargo test
```

## How to Deploy?

Deployment is automated with GitHub Actions CI/CD pipeline.
To deploy manually, install [`cargo-near`](https://github.com/near/cargo-near) and run:

If you deploy for debugging purposes:

```bash
cargo near deploy build-non-reproducible-wasm
```

If you deploy production ready smart contract:

```bash
cargo near deploy build-reproducible-wasm
```

## Initialize the contract

```bash
# on Linux / Windows WSL
TWO_MINUTES_FROM_NOW=$(date -d '+2 minutes' +%s000000000)
# on MacOS
TWO_MINUTES_FROM_NOW=$(date -v+2M +%s000000000)

near contract call-function as-transaction '<CONTRACT_ACCOUNT_ID>' init json-args '{"end_time": "'$TWO_MINUTES_FROM_NOW'", "auctioneer": "<AUCTIONEER_ACCOUNT_ID>", "token_contract": "usdn.testnet", "token_symbol": "USN", "token_decimals": 18, "bid_step": "1 USN"}' prepaid-gas '30.0 Tgas' attached-deposit '0 NEAR'
```

`token_contract` is the NEP-141 token contract allowed to submit bids. `token_symbol` and `token_decimals` must match that contract's `ft_metadata`; fetch them with `near_api::Tokens::ft_metadata` before initializing the auction. Provide `bid_step` as a human-readable decimal amount followed by the token symbol (for example, `"1 USN"`). The contract validates the symbol and converts the amount to the token's smallest units using `token_decimals`. The `get_highest_bid` and `get_bid_step` view methods return formatted values such as `10 USN`.

The contract account and every bidder/auctioneer must be registered with the USN contract before they can receive USN. Send a bid using `ft_transfer_call` on `usdn.testnet` (not a NEAR deposit):

```bash
near contract call-function as-transaction usdn.testnet ft_transfer_call json-args '{"receiver_id": "<CONTRACT_ACCOUNT_ID>", "amount": "<AMOUNT_IN_SMALLEST_USN_UNITS>", "msg": ""}' prepaid-gas '100.0 Tgas' attached-deposit '1 yoctoNEAR'
```

The contract refunds the previous bidder and transfers the winning USN to the auctioneer when `claim` is called after the auction ends.

## Useful Links

- [cargo-near](https://github.com/near/cargo-near) - NEAR smart contract development toolkit for Rust
- [near CLI](https://near.cli.rs) - Interact with NEAR blockchain from command line
- [NEAR Rust SDK Documentation](https://docs.near.org/sdk/rust/introduction)
- [NEAR Documentation](https://docs.near.org)
- [NEAR StackOverflow](https://stackoverflow.com/questions/tagged/nearprotocol)
- [NEAR Discord](https://near.chat)
- [NEAR Telegram Developers Community Group](https://t.me/neardev)
- NEAR DevHub: [Telegram](https://t.me/neardevhub), [Twitter](https://twitter.com/neardevhub)
