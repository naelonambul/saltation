# Implementation Plan

## Summary

The first part of stage 4 (card channel) of the root spec's delivery table. It builds the parts of the card channel that do not process payments:
- the card and card-authorization schema;
- debit card seeding with synthetic card numbers;
- the card network client (scope `card`) and its token;
- the fixed-rate conversion and the validation of an authorization message as pure functions.

**Excluded, by owner decision (2026-11-17: "결제 처리 제외하고 하자"):**
- the `POST /card/v1/authorize` route;
- posting a card payment to the ledger (`card_payment` transfer and transaction row);
- card events in the outbox (`card.authorized`, `card.declined`);
- the dormancy rule for card payments (F-11);
- the card network OpenAPI document.

These touch the posting path, the commit path and the request outcome that the `oracle-tuning` change is rewriting. A later change adds them on top of `oracle-tuning`. Apart from two named exceptions, this change makes no request-path change to any existing route: `POST /internal/v1/seed/counterparty_clients` accepts scope `card`, and the client token endpoint issues a `card` token to a card client (step 3).

Spec rules implemented here, quoted from the root `spec.md`:
- F-40: "A debit card is bound to one demand deposit account and has a synthetic 16-digit card number, unique in the bank, with a prefix no real issuer uses, and a status (active, blocked). Cards are created by seeding (F-29). There is no PIN, security code, expiry date, card limit or card management by the customer."
- F-41, in part: the card network is "a counterparty client authenticated with client credentials (scope `card`; seeded like the inbound-deposit clients of F-15, which have scope `oob`)".
- F-42: the currencies, fixed rates and message validity rules.
- Data: `CARD` (card ID; card number unique, account, status, issued at) and `CARD_AUTHORIZATION` (request; card (null when unknown), card number as sent, KRW amount, currency, original amount, merchant name, merchant category code, country, entry mode, latitude and longitude (null when online)); "cards by card number and by account" indexes.
- Security: "Card numbers are synthetic, with a prefix no real issuer uses, and are stored and logged in full; no real card data is ever accepted."

## Files and components that change

- `bank/migrations/0017_cards.sql` (new): `card`, `card_authorization`, their indexes, and `card_payment` added to the `transfer.kind` check so the later payment change needs no table rebuild.
- `bank/src/shared/db/migrations.rs`: one added `migration!(17, "0017_cards")` line, because `MIGRATIONS` lists every migration explicitly. This is the only file under `bank/src/shared/**` this change edits; `oracle-tuning` also appends to this list, and the second change to merge renumbers (see Risks).
- `bank/src/features/cards/**` (new): card number generation, `POST /internal/v1/seed/cards` (exposed as `cards::seed_router`, because a feature must not import another feature), `fx.rs` with the conversion and the authorization-message validation, and an embedded list of ISO 3166-1 alpha-2 codes. No customer or card network route.
- `bank/src/features/mod.rs`: declares `cards`.
- `bank/src/features/internal/**`: `scope = "card"` accepted by `seed/counterparty_clients`.
- `bank/src/features/inbound/**`: the token endpoint issues a `card`-scope token to a card client. Inbound deposits keep requiring `oob`.
- `contracts/codes/countries.json` (new): the officially assigned ISO 3166-1 alpha-2 codes. The bank's build context holds only `bank/`, so the bank embeds the same list in `features/cards`, and `bank/tests/cards.rs` asserts the two lists are equal, as the bank already mirrors `contracts/codes/institutions.json`.
- `contracts/openapi/internal.yaml`, `contracts/openapi/inbound.yaml`: the seed route and the token scope values.
- `bank/tests/cards.rs` (new), `bank/tests/inbound.rs`, `bank/tests/migrations.rs`, `bank/tests/sql/**`: tests.
- `scripts/dev/it-oracle.sh`: runs `cards` with the other integration tests.
- `docs/**`: only if a document lists the seed routes or client scopes.

- `bank/src/app.rs`: mounts `cards::seed_router` on the internal listener next to the other internal routes, as `password_reset::sms_router` is mounted. No other listener change.

## Order of work

1. **Schema** (`bank/migrations/0017_cards.sql`, `bank/src/shared/db/migrations.rs`, `bank/tests/migrations.rs`, `bank/tests/sql/**`).
   - `card`: `card_id` identity primary key; `card_no VARCHAR2(16) NOT NULL`, unique, checked as 16 digits starting with the bank's prefix, which the owner fixes before this step (Open questions); `account_id NOT NULL` foreign key to `account`; `status NOT NULL` `ACTIVE` or `BLOCKED`; `issued_at TIMESTAMP(6) WITH TIME ZONE NOT NULL` (business time, no default); `recorded_at TIMESTAMP(6) WITH TIME ZONE DEFAULT SYSTIMESTAMP NOT NULL` (F-28). Index `card_account_ix (account_id)`.
   - `card_authorization`, all `NOT NULL` unless stated: `request_id VARCHAR2(26)` primary key and foreign key to `transaction_request`; `card_id` nullable foreign key to `card`; `card_no_sent VARCHAR2(16)`; `krw_amount NUMBER(15,0)` between 1 and 999,999,999,999; `currency` in (`KRW`, `USD`, `EUR`, `JPY`, `CNY`); `original_amount NUMBER(15,0)` > 0; `merchant_name VARCHAR2(60 CHAR)` (the length of `account_transaction.counterparty_name`, where a payment's merchant name goes); `mcc CHAR(4)` of digits; `country CHAR(2)`; `entry_mode` in (`in_person`, `online`); `latitude NUMBER(8,6)` within -90..90 and `longitude NUMBER(9,6)` within -180..180, both null for `online` and both present for `in_person`; `recorded_at` as on `card`. Index `card_authorization_card_ix (card_id)`.
   - `transfer.kind`: the inline check in `0004_ledger.sql` is unnamed, so a PL/SQL block finds it in `user_constraints` of type `C` on `TRANSFER` whose `search_condition_vc` mentions `KIND` (the `LONG` `search_condition` cannot be filtered in SQL), drops it, and adds a named `transfer_kind_ck` with every existing kind plus `card_payment`.
   - `MIGRATIONS` gains entry 17.
   - Proof: `scripts/dev/it-oracle.sh migrations` passes on a fresh schema and on the CI schema. `migrations.rs` moves its applied-version assertion from `1..=16` to `1..=17` and adds assertions for the two tables, the unique card number, both indexes, the coordinate check and the named kind check with all eight kinds. Its other assertions are unchanged.
2. **Card numbers and seeding** (`features/cards/**`, `features/mod.rs`, `app.rs`, `contracts/codes/countries.json`, `contracts/openapi/internal.yaml`, `bank/tests/cards.rs`, `scripts/dev/it-oracle.sh`).
   - Card number: the fixed prefix (see Open questions), random digits from the operating system's random source, and a Luhn check digit. Uniqueness comes from the unique index: a collision retries with a new number, at most 5 times, then fails the batch.
   - `cards::seed_router(numbers)` takes the number source as a parameter; `app.rs` passes the random source, and `bank/tests/cards.rs` builds the router with a fixed sequence to force collisions.
   - `POST /internal/v1/seed/cards` (internal listener only, like the other seed routes): body `{"cards": [{"account_no": "...", "status": "ACTIVE" | "BLOCKED"}]}`, at most the existing seed batch maximum. Every account must exist and be a demand account, or the whole batch is refused as invalid. Response `{"cards": [{"account_no", "card_no", "status"}]}` in request order. One transaction for the batch. Like every `/internal/**` route, it writes no outbox event (`outcome/mod.rs` and `bank/tests/outcomes.rs` exclude internal routes).
   - Proof: new `cards.rs` tests: numbers are 16 digits, start with the prefix and pass Luhn; 1,000 seeded cards are unique; a batch mixing a valid account with a term, loan or unknown account, or with an unknown status, is refused and writes no row; a blocked card is stored blocked; with a fixed number sequence, a collision retries and a sixth consecutive collision fails the batch, rolling back cards already inserted earlier in the same batch; the route is absent on the public listener; no outbox row is written. `cards` runs as its own `it-oracle.sh` target and inside `all` (the target `repo.py verify --full` runs); `scripts/dev/it-oracle.sh cards` passes.
3. **Card network client** (`features/internal/**`, `features/inbound/**`, `contracts/openapi/inbound.yaml`, `bank/tests/inbound.rs`).
   - `seed/counterparty_clients` accepts `scope` `oob` or `card`.
   - The token endpoint issues a token whose scope is the client's stored scope, when the request asks for that scope. The response's `scope` field echoes it.
   - Inbound deposits keep rejecting a token without `oob`.
   - Proof: new `inbound.rs` tests: a card client gets a `card` token; that token gets 403 on the inbound deposit route; a card client asking for `oob` is refused. The existing invalid-seed test lists `("scope", "card")` as invalid, which the spec now makes valid; that case changes to `("scope", "other")`, so the test still proves an invalid scope is refused. Every other `inbound.rs` test passes unchanged.
4. **Conversion and message validation** (`features/cards/fx.rs`, unit tests in the same file).
   - `krw_amount(currency, amount)`: KRW 1 per won; USD 1,300 per dollar; EUR 1,400 per euro; JPY 900 per 100 yen, computed as `amount / 100 * 900` and only for multiples of 100; CNY 200 per yuan. Integer arithmetic with overflow checks. The result must be within F-10 (1 to 999,999,999,999).
   - `validate(message)` checks, and returns the first problem: `idempotency_key` 1 to 40 printable ASCII characters (`transaction_request.idem_key` is `VARCHAR2(40)` in bytes); `card_no` 16 digits with the bank's prefix and a valid Luhn check digit, so no real card number is ever accepted (Security); a number of that form that the bank never issued is a decline for an unknown card in the later payment change, not a malformed message; `amount` a JSON string of a positive integer; `currency` in the set; the KRW range; `merchant_name` 1 to 60 characters; `mcc` four digits; `country` an officially assigned ISO 3166-1 alpha-2 code from `contracts/codes/countries.json`; `entry_mode`; coordinates present exactly for `in_person`, JSON strings of decimal degrees with at most six decimals, within -90..90 and -180..180. On success it returns typed values (amounts and coordinates parsed, KRW amount computed) and leaves every text attribute byte for byte as sent. Nothing calls it yet.
   - Proof: `cargo test --lib cards` (run in the `bank-dev` container as the other checks) asserts `USD 1 -> 1,300`, `EUR 1 -> 1,400`, `JPY 100 -> 900`, `JPY 150` invalid, `CNY 1 -> 200`, `KRW 1 -> 1`, the F-10 upper limit and one above it per currency, overflow, every invalid field, an unassigned country such as `XX`, a card number with another prefix or a bad check digit, a multibyte idempotency key, and coordinates for both entry modes.
5. **Full verification.** On the OCI host (`salt-remote`): `python3 scripts/repo.py verify --change card-foundation --full`, then `scripts/dev/it-oracle.sh migrations` and `e2e`.

## Tests and proof

- No existing test is weakened or removed. `migrations.rs` gains assertions; its existing counts change only where this change adds an index, and each such change is named in the commit.
- New tests are listed per step. The suite runs with `scripts/dev/it-oracle.sh all` on the OCI host; the Mac mini's Oracle is reserved for `oracle-tuning` measurements until that change's step 2 capture is done.
- Card numbers in tests are generated by the bank. No test uses a real-issuer number.

## Risks and mitigations

- **Migration number collision with `oracle-tuning`**, which will add its own migrations and `MIGRATIONS` entries. Whichever change merges second renumbers its new migration file, its `MIGRATIONS` entry and the `migrations.rs` range before its pull request; migrations are forward-only and neither change edits an applied one.
- **Overlap with `oracle-tuning`'s write scope.** Apart from the one `MIGRATIONS` line, this change edits none of `shared/**`, `ledger`, `transfers`, `accounts`, `outbox`, `auth`, `atm`, `password_reset`, `payees` or `daily_close`. `bank/tests/migrations.rs` and `scripts/dev/it-oracle.sh` are shared files; conflicts there are line-level and resolved at rebase.
- **A prefix that a real issuer uses.** Major industry identifier 0 is assigned to ISO/TC 68 and other industry assignments, not to card issuers; the repository cannot prove that no issuer number starts with `0000`, so the prefix is the owner's decision (Open questions).
- **Schema for payments decided before the payment code exists.** `card_authorization` follows the spec's data section column for column; the later payment change may add columns by a new migration, never by editing this one.

## Rollback or recovery

Code reverts with the change's squash commit. The tables are new and empty outside seeding; a new forward migration drops them if the change is undone.

## Open questions

- **Card number prefix.** Proposed: `0000`, which falls in major industry identifier 0 (ISO/TC 68 and other industry assignments), a range assigned to no card issuer, followed by 11 random digits and a Luhn check digit. The owner confirms or picks another prefix before step 1, because the schema checks it.
