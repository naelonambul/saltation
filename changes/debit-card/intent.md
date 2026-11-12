# Intent

## Problem

Fraud detection systems (FDS) and the MLOps pipelines behind them are hard to learn and hard to demonstrate without access to a real bank. Public fraud datasets are static tables: they carry no live event stream, no login, device or session context, no channel (app, ATM) detail, and no way to inject a new fraud pattern and watch a pipeline react to it. Real banking data cannot be used for privacy and legal reasons.

The owner is a data-engineering and MLOps bootcamp student building a personal portfolio project on the theme "real-time financial fraud anomaly detection MLOps pipeline". It is not coursework or a classroom submission. The owner needs a realistic, continuously running source of banking activity, including fraud with known ground truth, to build that pipeline on.

## Proposed outcome

Saltation is a self-contained simulated bank with a fraud-detection platform built on top of it, developed incrementally with no fixed end date.

1. **A bank that produces realistic data.** A Rust banking server provides accounts, cash deposits and withdrawals, transfers, payees, simplified term deposits (open, early close and maturity, returning principal only) and simplified loans (disbursement and repayment, with no interest), debit card payments, authentication, and device and session tracking. A debit card is bound to one account, and a payment leaves that account at once; there are no credit cards. Simulated external counterparties (for example employers and merchants) are the source and destination of funds that enter or leave the bank. It serves three channels: a customer-facing mobile app, a simulated ATM and a simulated card network. The simulated ATM is an interface driven by the generator, with no human-facing screen; its cash transactions are recorded like any other transaction, and its channel events in the logs. The simulated card network is one interface, also driven by the generator, that sends card payments the way a single international card scheme would. The bank keeps no merchant register: it records the merchant's name, category, country and, for a payment made in person, the geographic coordinates of the place of payment, exactly as the network sends them. An analyst-facing admin web app sits alongside the channels.
2. **Persona-driven synthetic activity.** A synthetic generator creates customers and accounts through a seeding interface that is not a customer channel, then drives the bank through its public channel interfaces as a population of personas: ordinary customers (for example office workers, students, self-employed, elderly) and fraud actors. It first backfills about one year of history, so that customer baselines and dormant accounts exist, then keeps generating activity in real time. The bank runs on a simulated clock that the generator can advance during the backfill only, so that historical activity carries historical timestamps and passes through the same code paths as live activity. Scenario frequencies are grounded in published fraud statistics and scaled by a configurable amplification factor that is recorded with each run. Fraud actors play the scenarios below, interleaved with normal behaviour and with benign lookalikes (legitimate activity that resembles fraud). The generator records ground truth separately from the bank's own data.
3. **Data collection first.** Structured banking records are stored in a relational database, and unstructured logs (application, access, session, device and channel events) are collected into a document store, so that a growing, labelled history accumulates. This is the first priority.
4. **Real-time detection, later.** Detection consumes bank activity as a stream, in real time but asynchronously, so a transaction never waits for it. It raises alerts that analysts review in the admin web app. An alert signals suspicion, not proven fraud. Rule-based and machine-learning detection, and the MLOps around the models (training, versioning, deployment, retraining), are built after data collection is in place.

### Fraud scenarios

All of these are in scope for the product. Each must be producible by the generator and, once detection exists, measurable.

| ID | Scenario | Characteristic signals |
|---|---|---|
| A | Account takeover | Login from a new device; failed-login bursts; password reset, then new payee, then high-value transfer; abrupt change of access location |
| B | Voice-phishing victim | The victim acts under a fraudster's instruction with valid credentials: unusually large outflows, mostly withdrawn as cash at ATMs, often right after early closing of a term deposit or taking a loan; transfers to never-seen payees, which are mule accounts (C) inside the bank |
| C | Money mule account | Inflows from voice-phishing victims (B) and other unrelated senders, followed quickly by ATM cash withdrawals; new or dormant accounts suddenly active; devices or IP addresses shared across mule accounts |
| D | Structuring | A large cash sum deliberately split across several days and several accounts (including accounts held in other people's names), so that each account's daily cash total stays just below a simulated daily currency-transaction reporting threshold |
| E | Velocity and amount anomaly | Standalone: transaction frequency or amount far outside the customer's own baseline, not generated as part of an A to D or G instance |
| F | Time and location anomaly | Standalone: activity at hours the customer never uses, or access from unusual or foreign locations, not generated as part of an A to D or G instance |
| G | Card misuse abroad | A customer's debit card is used in a foreign country, in person or online, without the customer: the place of payment is inconsistent with where the customer's own activity is; payments in foreign currency at merchants the customer never used, sometimes small test payments before large ones |

### Labelling

- Every fraud instance carries exactly one primary scenario label (A to G). Signals overlapping between scenarios are expected and allowed.
- Labels attach per account involved in an event. A voice-phishing victim's transfer into a mule account is labelled B on the sender's side and C on the receiver's side.
- Benign lookalikes (for example legitimate travel, card payments during a real trip abroad, online shopping at foreign merchants, a new phone, a one-off large purchase, a shared household network) carry their own label, so that false positives can be measured.
- All other generated activity is labelled normal. Sessions in which a human uses the mobile app directly are marked as such and excluded from labelled datasets.

## Affected users and systems

- **Owner (developer and operator):** builds, deploys and runs the system; presents it as a portfolio project.
- **Simulated customers:** generator personas using the mobile, ATM and card channels. A human may also use the mobile app directly.
- **Fraud analysts:** use the admin web app to inspect customers, accounts and transactions and, once detection exists, to review alerts.
- **Systems:** Rust banking server, mobile app, simulated ATM channel, simulated card network, admin web app, synthetic generator, relational database, document store for logs, the OCI development host, and the VMware lab VMs.

## Constraints

- The banking server is written in Rust.
- Structured data lives in Oracle Database, run as a container built from Oracle's official container build scripts (Oracle Database 19c Enterprise Edition, which supports both ARM64 and x86-64 and has no edition data-size cap). Unstructured logs live in MongoDB for now; MongoDB is planned to be replaced later by PostgreSQL (JSONB), with the migration timed and justified by measurements on the accumulated data, so the log format must stay portable and must not depend on MongoDB-only features.
- The primary environment is Docker Compose on an Oracle Cloud Infrastructure development host the owner reaches over SSH. The host is an OCI Always Free Ampere A1 instance: ARM64, 4 vCPU, 24 GB RAM, 50 GB boot volume plus 150 GB block volume. Every service runs under explicit CPU and memory limits, including the Oracle Database memory (SGA and PGA) and the MongoDB cache. Saltation shares the host with other workloads already running there, and must leave them healthy.
- A secondary, occasional environment is a local lab: VMware Workstation Pro on an x86-64 Windows host with WSL, provisioned with Terraform and configured with Ansible. It is not required for every change, but nothing may lock the stack to ARM64.
- All data is synthetic. No real personal or financial data is ever used.
- All money is Korean won in whole-won integer amounts. The bank computes no interest on any product, so no fractional amounts arise. A card payment in a foreign currency is converted at a fixed rate (1 US dollar = 1,300 won, 1 euro = 1,400 won, 100 yen = 900 won, 1 Chinese yuan = 200 won) and is accepted only in whole dollars, whole euros, whole yuan or multiples of 100 yen, so the won amount is always exact. No other currency is accepted. Dollars can be used in any country of the world; euros are used in euro-area countries, yen in Japan, yuan in China and won in Korea, so a payment anywhere else abroad is in dollars. The original currency and amount are kept as attributes of the payment; there are no exchange-rate movements.
- Detection is real-time and asynchronous: it processes events as they happen, and a banking transaction never waits for a detection decision or for the detector to be available.
- Ground-truth labels live apart from the bank's operational data. The bank and online detection cannot read them; offline training and evaluation can. Identifiers that link events to labels are opaque and reveal nothing about the label.
- One developer, no deadline; the product grows through successive changes.

## Out of scope

- Real money movement, real payment networks, interbank settlement, or integration with real banks.
- Real customer onboarding, identity verification, credit assessment, or regulatory reporting.
- Synchronous (inline) blocking of transactions by the detector.
- Production-grade security certification or compliance.
- Physical ATM hardware; the ATM is a simulated channel.
- Credit cards, card billing, card limits, refunds and chargebacks, merchant registration or management, and card credentials (PIN, security code, expiry).
- Kubernetes, for now.

## Success criteria

Data foundation (first):

- After backfilling about one year of history, the generator runs for at least 24 continuous hours against the Docker Compose stack on the development host, using only the bank's public mobile, ATM and card interfaces, with no manual intervention or service restart. In that run every persona type produces normal activity, every scenario A to G occurs at least a spec-defined number of times, and benign lookalikes occur.
- Every scenario instance has all spec-defined expected records, including rejected attempts such as failed logins: transactions in Oracle Database and login, session, device and channel events in MongoDB, with none missing.
- Every generated activity event joins, through opaque identifiers, to exactly one label per account involved: normal, benign lookalike, or a primary scenario A to G.
- Through the mobile app a customer can sign in, view balances and history, manage payees, transfer money, open and early-close a term deposit, and take and repay a loan. Through the simulated ATM channel the generator deposits and withdraws cash; those transactions are stored in Oracle Database, their channel events in the logs, and stored data distinguishes the channel. Through the simulated card network the generator pays with debit cards at home and abroad; each payment is stored with its merchant name and category, country, original currency and amount and, when made in person, its coordinates, and declined payments are stored too. Through the admin web app an analyst can look up customers, accounts and transactions.
- After documented prerequisites, one Docker Compose command starts all server-side services on the development host. Installing the mobile app is separate.
- During the backfill and the 24-hour run no Saltation container is killed for lack of memory or restarted, and the host's other workloads stay healthy.
- The backfilled year plus a spec-defined period of live data fits within a spec-defined share of the host's block volume, and the spec defines retention or archival beyond that.
- Every container image is published for, or (for Oracle Database) buildable from documented steps on, both linux/arm64 and linux/amd64, and the Rust services build for both.

Detection (later):

- Detection never gates a transaction. Under the spec-defined workload, the p95 delay from the last event a scenario needs to the alert, and the difference in p95 transaction latency between detection running and detection stopped, meet spec-defined bounds.
- On held-out labelled runs that include benign lookalikes, detection meets spec-defined per-scenario precision and recall targets and an alert-volume limit for analysts.
- An automated pipeline completes an initial training-to-serving cycle and a retraining on newer data, with traceable dataset, model and deployed-version identities.

## Open questions

- Mobile app and admin web app technology stacks.
- Oracle Database container details: obtaining the 19c installation binaries for ARM64 and x86-64, image build time and size, and memory sizing. The owner confirms at download that the license shown permits this use: development and demonstration of a personal portfolio project, with no production, commercial or classroom use.
- Whether OCI is ever a deployment target, or only the development host.
- How events reach detection in real time (for example a message broker such as Kafka, or change data capture), and when it is introduced.
- Detection approach and MLOps tooling (rules, ML scoring, workflow orchestration, model registry), and where model training runs. Deferred until detection is designed; data collection comes first.
- Generator volume (customers, transactions per minute), scenario frequencies and the default amplification factor, sized to what the share of the host's CPU, memory and disk given to Saltation sustains, and how long the backfill may take.
- Authentication model for customers and analysts.
- Concrete thresholds, for example the simulated reporting threshold for D (modelled on the Korean currency transaction report, which aggregates daily cash transactions).
