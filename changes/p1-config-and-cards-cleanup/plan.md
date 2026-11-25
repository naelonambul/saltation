# Change Plan: `p1-config-and-cards-cleanup`

## Goal

Close three P1 configuration and consistency items identified in the maintainer review without changing any API contract or spec behavior:

1. **Sync lab Oracle tablespace `MAXSIZE 30G` (`infra/lab/ansible/oracle-init.sql`):**
   - Replace `MAXSIZE UNLIMITED` on `saltation_data` with `MAXSIZE 30G`, matching `infra/oracle/setup/01_saltation.sql` (within Oracle 19c 8 KiB smallfile limit and the N-9 70 GB storage budget).
2. **Replace raw `/dev/urandom` reads with `rand::rng()` (`bank/src/features/cards/mod.rs`):**
   - Replace manual `std::fs::File::open("/dev/urandom")` rejection sampling in `random_number()` with `rand::rng().random_range(0..10u8)`, matching the CSPRNG already used in `auth` and `password_reset` while keeping the `io::Result<String>` signature intact for injected test hooks.
   - Add a unit test verifying `random_number()` generates valid 16-digit Luhn card numbers with `PREFIX`.
3. **Contract-to-bank `countries.json` sync test (`bank/src/features/cards/fx.rs`):**
   - Add a unit test comparing `include_str!("countries.json")` against `${CONTRACTS_DIR:-../contracts}/codes/countries.json` so contract and bank country lists cannot silently drift (works both locally and inside `scripts/dev/ci-rust.sh` where `CONTRACTS_DIR=/contracts`).

## Verification

- Local:
  - `cargo fmt --check`
  - `cargo clippy --lib --bins --tests -- -D warnings`
  - `cargo test --lib`
  - `python3 scripts/repo.py status --change p1-config-and-cards-cleanup`
  - `python3 scripts/repo.py verify --change p1-config-and-cards-cleanup --group core`
- CI (PR):
  - All required checks green (`rust`, `control-plane`, `status`, `identity`, `oracle`, `summary`).
