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

`init` configures the FT and the contract's fees. The account that calls `init` becomes the contract owner and can withdraw accrued fees. Provide `auction_start_fee` as a human-readable NEAR amount, such as `"0.1 NEAR"`; the contract parses it into a `NearToken`. `claim_fee_basis_points` uses basis points (`200` means 2%).

```bash
near contract call-function as-transaction '<CONTRACT_ACCOUNT_ID>' init json-args '{"token_contract": "usdn.testnet", "token_symbol": "USN", "token_decimals": 18, "auction_start_fee": "0.1 NEAR", "claim_fee_basis_points": 200}' prepaid-gas '30.0 Tgas' attached-deposit '0 NEAR'
```

The token metadata must match the configured NEP-141 contract's `ft_metadata`; clients can fetch it with `near_api::Tokens::ft_metadata`. No auction is active immediately after `init`.

## Start an auction

The caller of `start_new_auction` becomes the auctioneer. The call starts the first auction or, once the current auction has ended, claims its winning bid and starts the next round. The new auctioneer must attach exactly the configured `auction_start_fee`. The lot description is required and limited to 256 bytes.

```bash
near contract call-function as-transaction '<CONTRACT_ACCOUNT_ID>' start_new_auction json-args '{"auction_duration": "24 hours", "description": "A unique digital artwork", "bid_step": "1 USN"}' prepaid-gas '50.0 Tgas' attached-deposit '0.1 NEAR'
```

`auction_duration` is a positive integer followed by seconds, minutes, hours, or days. The contract calculates the end time from the current block timestamp. Provide `bid_step` as a human-readable decimal amount with the token symbol. `max_bid_step` is optional; when provided, it caps the increase above the current highest bid and must be at least `bid_step`. When omitted, there is no maximum increase. A new bidder must submit at least the current highest bid plus `bid_step`; a lower payment is returned in full. For example, with a highest bid of `5 USN`, a `7 USN` payment is refunded in full when the bid step is `3 USN`, while a `25 USN` payment raises the highest bid to `15 USN` (the previous `5 USN` plus the `10 USN` maximum increase) and returns `10 USN` if `max_bid_step` is `"10 USN"`.

Use the `get_auction_status` view method to fetch the active round in one response: lot description, auctioneer, highest bid, bid steps, UTC ISO 8601 start and end times, readable time remaining, and claim status. It returns `null` before the first auction starts. The individual auction view methods remain available.

The contract account, bidders, auctioneers, and the owner (if withdrawing the FT commission) must be registered with the FT contract before they can receive tokens. Send bids using `ft_transfer_call` on the configured token contract, not as a NEAR deposit:

```bash
near contract call-function as-transaction usdn.testnet ft_transfer_call json-args '{"receiver_id": "<CONTRACT_ACCOUNT_ID>", "amount": "<AMOUNT_IN_SMALLEST_USN_UNITS>", "msg": ""}' prepaid-gas '100.0 Tgas' attached-deposit '1 yoctoNEAR'
```

After an auction ends, `claim` sends the auctioneer the winning bid less the configured commission. `start_new_auction` performs this claim automatically for the previous round. Start fees accrue in NEAR, and claim commissions accrue in the configured FT. Only the contract owner can withdraw them with `withdraw_start_fees` and `withdraw_claim_fees`.

An auctioneer can query their completed-round statistics and history with `get_auctioneer_history(auctioneer_id, from_index, limit)`. The response includes the number of completed auctions, total winning bids, total paid out, the amount still pending, and a page of auction records with winning bidder/bid and payout status. Records are paginated in completion order; `limit` is capped at 50, and `next_index` indicates the next page. For example, start with `from_index: 0` and `limit: 20`, then use the returned `next_index`.

If a token payout fails (for example, because its recipient is not registered), it remains listed by `get_pending_payouts` and in the auctioneer's history with `Pending` status. After resolving the cause, anyone can retry it with `retry_pending_payout(payout_id)`. Unused amounts from `ft_on_transfer` are returned by the FT contract according to NEP-141 transfer-call resolution.

## Useful Links

- [cargo-near](https://github.com/near/cargo-near) - NEAR smart contract development toolkit for Rust
- [near CLI](https://near.cli.rs) - Interact with NEAR blockchain from command line
- [NEAR Rust SDK Documentation](https://docs.near.org/sdk/rust/introduction)
- [NEAR Documentation](https://docs.near.org)
- [NEAR StackOverflow](https://stackoverflow.com/questions/tagged/nearprotocol)
- [NEAR Discord](https://near.chat)
- [NEAR Telegram Developers Community Group](https://t.me/neardev)
- NEAR DevHub: [Telegram](https://t.me/neardevhub), [Twitter](https://twitter.com/neardevhub)
