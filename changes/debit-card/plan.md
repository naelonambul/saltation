# Implementation Plan

## Summary

This change makes debit card payments and overseas card misuse (scenario G) part of the product baseline. It replaces the root `intent.md` and `spec.md` with this packet's approved copies and changes nothing else: no code, contract, migration or check.

The card channel itself is built later, as stage 4 of the new Delivery table (after ATM, before the admin interface), in its own change with its own plan. Scenario G and the card lookalikes are generated in stage 6, and the admin and mobile screens in stage 7.

What the baseline gains, in short: a debit card bound to one demand account; one card network interface on the generator listener that sends single-message payments with merchant name, category, country, entry mode and, in person, coordinates; fixed conversion rates for USD, EUR, JPY and CNY with whole-unit amounts; a `card_payment` ledger kind; stored declines; coordinates on ATM terminals; scenario G and two new lookalikes.

## Files and components that change

<!-- Must be covered by change.json write_scope. -->

| Path | Change |
|---|---|
| `intent.md` | replaced whole by this packet's approved `intent.md` |
| `spec.md` | replaced whole by this packet's approved `spec.md` |

## Order of work

1. Replace the root `intent.md` and `spec.md` with the approved packet copies.
   - Proof: `python3 scripts/repo.py status --change debit-card` passes, and `git diff origin/main -- intent.md spec.md` equals the difference between the root files on `main` and the packet copies.
2. Run `python3 scripts/repo.py verify --change debit-card --full`, push, and open the pull request with `Change-ID: debit-card`.

## Tests and proof

The full verify and the CI `summary` check. There is no behaviour to test in this change.

## Risks and mitigations

- **The open stage-2 draft `ledger-products`** (plan not yet approved) was written against the earlier spec. After this merges it is refreshed onto `main`, and its plan is revised where the new spec touches stage 2: counterparty clients carry a scope (`oob` or `card`), and the ledger's kind checks leave room for `card_payment`. Nothing else in stage 2 changes.
- **Scope growth.** The spec lists what is excluded (credit cards, limits, refunds, merchant register, card credentials); the stage-4 plan is held to it.

## Rollback or recovery

Revert the squash commit; no data or code depends on it yet.

## Open questions

- None.
