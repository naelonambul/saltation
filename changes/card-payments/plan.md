# Implement debit card payments

## Summary

Complete stage 4 on top of `card-foundation` and `oracle-tuning`. A simulated card network submits a single-message debit authorization on the generator listener. The bank stores well-formed approvals and declines, debits an approved payment immediately, and publishes the outcome through the existing transactional outbox. Apply F-10, F-11, F-18–23, F-31, F-35–36a and F-40–43 of the root spec.

Use the existing card and authorization tables and the existing `card_payment` transfer constraint from migration `0020`. Stage 5 owns the analyst lookups and stage 6 owns generator card scenarios. Holds, refunds, reversals, customer card management and changes to other routes are outside this change.

## Files and components that change

| Path | Change |
| --- | --- |
| `bank/src/app.rs`, `bank/src/features/cards/**` | Generator-only route and card-specific authorization, SQL and response handling. Reuse the existing card validation and FX conversion. |
| `bank/src/shared/ledger/mod.rs` | Add the card movement shape to the shared writer. |
| `bank/src/features/daily_close/sql/transfers.sql` | Accept one correctly signed card leg in the F-21 audit. |
| `bank/src/shared/outcome/mod.rs` | Classify card requests and record unauthorized attempts and idempotency conflicts through the shared outcome mechanism. Card conflict records leave BANK_TRAN_ID null and do not claim an idempotency key. |
| `contracts/openapi/card.yaml` | Define the card network operation, bearer scope, request and responses. |
| `bank/tests/**`, `scripts/dev/it-oracle.sh` | Database and HTTP witnesses, and registration in the existing Oracle integration runner. |

## Order of work

1. **Write the contract and listener wiring** (`contracts/openapi/card.yaml`, `bank/src/app.rs`, `bank/src/features/cards/**`, `bank/tests/card_payments.rs`, `scripts/dev/it-oracle.sh`).
   - Mount `POST /card/v1/authorize` only on the generator listener. Use `AuthClient` and require scope `card`. Document how to obtain the bearer token with the existing client-credentials operation.
   - Use the established response envelope and codes. Approvals use HTTP 200. Business declines use HTTP 422 with a stable reason for unknown card, blocked card, closed account or insufficient balance. Malformed requests use 400, a missing or invalid token 401, the wrong scope 403, an idempotency conflict 409, and retryable Busy 503 (`A0013`) when the operation or required outbox write cannot proceed. Include the KRW amount on approval and business decline.
   - Use the existing F-42 rules and synthetic-number validation. Accept an unknown number as a decline when it is a well-formed synthetic number. Keep supplied merchant attributes unchanged. Bind coordinates as exact decimal values using their validated millionths and store original currency and original amount in their existing columns. Retain the original text values in the business event. Malformed requests produce the generic request outcome without creating a payment attempt.
   - Proof: `scripts/dev/it-oracle.sh card-payments` exercises the real routers, contract schemas, token and listener isolation, malformed inputs and all supported currencies.

2. **Implement one debit and reconcile it** (`bank/src/shared/ledger/mod.rs`, `bank/src/features/daily_close/sql/transfers.sql`, `bank/tests/card_payments.rs`, `bank/tests/common/daily_close.rs` as needed).
   - Extend the shared ledger to require exactly one negative leg equal to the KRW amount. Route card approval through that writer. Use the merchant name as counterparty name and leave counterparty bank and account null. Leave mobile and ATM limit counters untouched.
   - Use the existing customer-activity bit so a card approval updates last activity and reactivates a dormant account through the existing status-history and event path. Declines and replays leave both unchanged.
   - Make the daily-close audit require one card leg with the exact negative amount. Exercise both a valid and an invalid card shape through the audit.
   - Proof: `scripts/dev/it-oracle.sh card-payments` checks the signed ledger row, account sequence, balance, channel, dormant-to-active history and unaffected transfer and cash counters. `scripts/dev/it-oracle.sh backfill` verifies that daily close accepts a valid debit and flags an invalid card shape.

3. **Implement authorization and replay atomically** (`bank/src/features/cards/**`, `bank/src/shared/outcome/mod.rs`, `bank/tests/card_payments.rs` and supporting test SQL).
   - Capture the operation's business time once. Claim the request before side effects with the existing unique request and idempotency constraints, scope `client:<counterparty_client_id>`, and a SHA-256 hash over explicit validated fields in a fixed order. Normalize integer amounts and coordinate values for hashing, retain merchant text exactly, and keep an operation discriminator. Reuse the shared request-ID ownership and idempotency-conflict semantics. The card key is up to 40 bytes and belongs in IDEM_KEY. Leave BANK_TRAN_ID null on card claims and rejections, including conflicts, because it is a 20-byte Open Banking field. Preserve existing callers' BANK_TRAN_ID behavior.
   - On duplicate, roll back the attempted insert, read the committed result, and compare hashes and request-ID ownership before replaying. Return the original business result without another authorization, ledger movement or card business event. Use the current request ID for the standard response envelope and request-completion event. A different hash or an ID already owned by another key returns a recorded conflict without modifying the original authorization.
   - Lock the matching card, then the account through the shared ledger lock, and evaluate the card status, account status and available balance under the locks. Only an active card linked to an active or dormant account with sufficient funds is approved. Lock ordering is request claim, card, account, then any needed daily-total row.
   - Insert exactly one `CARD_AUTHORIZATION` for an executed business decision. Unknown cards retain the number as sent and have no card or account association. Save the response data for replay. Record exactly one `card.authorized` or `card.declined` event with the authorization attributes, alongside `request.completed`. The request, authorization, approval ledger changes and both events commit together. Reuse the tuned outbox/completion path to avoid a second completion on successful commit. Route failures through existing rollback/error mapping, including retryable outbox exhaustion. Record 401/403 through shared rejected-attempt recording. Record a generic completion for malformed input without storing card payment attributes.
   - Use channel `CARD`. Preserve generator network context and request correlation. The token identifies the card client. It creates no customer session or device association. For an unknown card the business events have no customer or account attribution. Card payloads may include the validated synthetic number and merchant data. Credentials and arbitrary unvalidated request bodies never go into events.
   - Proof: `scripts/dev/it-oracle.sh card-payments` verifies approved and all four declined outcomes, exact persisted attributes, one debit under concurrent duplicates, independent keys across clients, conflict handling and replay of approval and decline. Use a valid 40-byte key for the first claim, an identical replay and a same-key/different-hash conflict. Assert HTTP 409, a separate unclaimed rejection with BANK_TRAN_ID null, and unchanged original request and ledger rows. Exercise simultaneous distinct debits that together exceed the balance. Inject an outbox insert failure and assert a retryable HTTP 503 matching the OpenAPI response schema, no partial payment state, and one successful debit when the exact request retries after recovery.

4. **Verify the full change**.
   - Run `python3 scripts/repo.py status --change card-payments` and `python3 scripts/repo.py verify --change card-payments --full` on OCI. Check the new target directly and verify the contract against responses from the running routers. Use the existing CI suite for cross-feature regressions.

## Tests and proof

Register the card payment target with the existing `it-oracle.sh all` integration coverage. Assert persisted state and outbox effects by request ID, including the absence of card business effects for auth errors, validation errors, conflicts, replay and failed commits. Confirm one request completion per HTTP call, using distinct request IDs for the concurrency fixtures except where explicitly testing reused IDs. Verify decline amounts and reason codes on first send and replay. Use fresh test schema state and synthetic cards issued by the existing seed route.

## Risks and mitigations

- **Duplicate or partial debits.** Rely on the request uniqueness constraint and the locked balance, and keep ledger, authorization and outbox writes in one commit.
- **Incorrect day rollover or transfer limits.** Reuse the ledger activity transition, keep card amounts out of transfer and cash limits, and extend the close audit without weakening existing cases.
- **Exposure on the wrong listener or attribution to a customer device.** Route only on the generator listener, require the card scope and distinguish simulated client context from customer identity.
- **Oracle text and numeric conversion.** Use exact decimal binds for coordinates and preserve supplied merchant text. Cover multibyte merchant names and both coordinate boundaries through the HTTP-to-Oracle path.

## Rollback or recovery

Before accepting payments, revert the change to remove the route and restore the earlier behavior. After payments have been recorded, keep the card ledger kind and close-audit support and disable the route by a forward change. Existing ledger, authorization and outbox history remains intact.

## Open questions

- None.
