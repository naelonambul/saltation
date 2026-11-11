# Intent

## Problem

Fraud detection systems (FDS) and the MLOps pipelines behind them are hard to learn and hard to demonstrate without access to a real bank. Public fraud datasets are static tables: they carry no live event stream, no login, device or session context, no channel (app, ATM) detail, and no way to inject a new fraud pattern and watch a pipeline react to it. Real banking data cannot be used for privacy and legal reasons.

The owner is a data-engineering and MLOps bootcamp student building a personal portfolio project, "real-time financial fraud anomaly detection MLOps pipeline", and needs a realistic, continuously running source of banking activity, including fraud with known ground truth, to build that pipeline on.

## Proposed outcome

Saltation is a self-contained simulated bank with a fraud-detection platform built on top of it, developed incrementally with no fixed end date.

1. **A bank that produces realistic data.** A Rust banking server provides accounts, cash deposits and withdrawals, transfers, payees, simplified term deposits (open and early close) and simplified loans (disbursement), authentication, and device and session tracking. It serves two channels: a customer-facing mobile app and a simulated ATM. An analyst-facing admin web app sits alongside them.
2. **Persona-driven synthetic activity.** A synthetic generator drives the bank through its public channel interfaces as a population of personas: ordinary customers (for example office workers, students, self-employed, elderly) and fraud actors. It first backfills a history of weeks to months, so that customer baselines and dormant accounts exist, then keeps generating activity in real time. Fraud actors play the scenarios below, interleaved with normal behaviour and with benign lookalikes (legitimate activity that resembles fraud). The generator records ground truth separately from the bank's own data.
3. **Data collection first.** Structured banking records are stored in a relational database, and unstructured logs (application, access, session, device and channel events) are collected into a document store, so that a growing, labelled history accumulates. This is the first priority.
4. **Real-time detection, later.** Detection consumes bank activity as a stream, in real time but asynchronously, so it never blocks a transaction. It raises alerts that analysts review in the admin web app. An alert signals suspicion, not proven fraud. Rule-based and machine-learning detection, and the MLOps around the models (training, versioning, deployment, retraining), are built after data collection is in place.

### Fraud scenarios

All of these are in scope for the product. Each must be producible by the generator and, once detection exists, measurable.

| ID | Scenario | Characteristic signals |
|---|---|---|
| A | Account takeover | Login from a new device; failed-login bursts; password reset, then new payee, then high-value transfer; abrupt change of access location |
| B | Voice-phishing victim | The victim acts under a fraudster's instruction with valid credentials: unusually large outflows, mostly withdrawn as cash at ATMs, often right after early closing of a term deposit or taking a loan; transfers to never-seen payees, which are mule accounts (C) inside the bank |
| C | Money mule account | Inflows from voice-phishing victims (B) and other unrelated senders, followed quickly by ATM cash withdrawals; new or dormant accounts suddenly active; devices or IP addresses shared across mule accounts |
| D | Structuring | Cash deposits or withdrawals deliberately split across several transactions in a short window so that the daily cash total stays below a simulated currency-transaction reporting threshold |
| E | Velocity and amount anomaly | Standalone: transaction frequency or amount far outside the customer's own baseline, without the patterns of A to D |
| F | Time and location anomaly | Standalone: activity at hours the customer never uses, or access from unusual or foreign locations, without the patterns of A to D |

### Labelling

- Every fraud instance carries exactly one primary scenario label (A to F). Signals overlapping between scenarios are expected and allowed.
- Benign lookalikes (for example legitimate travel, a new phone, a one-off large purchase, a shared household network) carry their own label, so that false positives can be measured.
- All other activity is labelled normal.

## Affected users and systems

- **Owner (developer and operator):** builds, deploys and runs the system; presents it as a portfolio project.
- **Simulated customers:** generator personas using the mobile and ATM channels. A human may also use the mobile app directly.
- **Fraud analysts:** use the admin web app to inspect customers, accounts and transactions and, once detection exists, to review alerts.
- **Systems:** Rust banking server, mobile app, simulated ATM channel, admin web app, synthetic generator, relational database, document store for logs, the OCI development host, and the VMware lab VMs.

## Constraints

- The banking server is written in Rust.
- Structured data lives in Oracle Database; unstructured logs live in MongoDB. Consolidating stores later (for example onto PostgreSQL) must remain possible.
- The primary environment is Docker Compose on an Oracle Cloud Infrastructure development host the owner reaches over SSH. The host is ARM64 (Ampere, 4 cores, about 24 GB RAM). Every service runs under explicit CPU and memory limits, including the Oracle Database memory and the MongoDB cache, and the chosen Oracle edition's resource and data-size limits apply (Oracle Database Free: about 2 CPU threads, 2 GB memory, 12 GB user data).
- A secondary, occasional environment is a local lab: VMware Workstation Pro on an x86-64 Windows host with WSL, provisioned with Terraform and configured with Ansible. It is not required for every change, but nothing may lock the stack to ARM64.
- All data is synthetic. No real personal or financial data is ever used.
- Detection is real-time and asynchronous: it processes events as they happen, and it must never block or delay a banking transaction.
- Ground-truth labels live apart from the bank's operational data. The bank and online detection cannot read them; offline training and evaluation can. Identifiers that link events to labels are opaque and reveal nothing about the label.
- One developer, no deadline; the product grows through successive changes.

## Out of scope

- Real money movement, real payment networks, interbank settlement, or integration with real banks.
- Real customer onboarding, identity verification, credit assessment, or regulatory reporting.
- Synchronous (inline) blocking of transactions by the detector.
- Production-grade security certification or compliance.
- Physical ATM hardware; the ATM is a simulated channel.
- Kubernetes, for now.

## Success criteria

Data foundation (first):

- After backfilling a history of spec-defined length, the generator runs for at least 24 continuous hours against the Docker Compose stack on the development host, using only the bank's public mobile and ATM interfaces, with no manual intervention or service restart. In that run every persona type produces normal activity, every scenario A to F occurs at least a spec-defined number of times, and benign lookalikes occur.
- Every scenario instance has all spec-defined expected records, including rejected attempts such as failed logins: transactions in Oracle Database and login, session, device and channel events in MongoDB, with none missing.
- Every generated activity event joins, through opaque identifiers, to exactly one label: normal, benign lookalike, or a primary scenario A to F.
- Through the mobile app a customer can sign in, view balances and history, manage payees, transfer money, open and early-close a term deposit, and take a loan. Through the simulated ATM channel a customer can deposit and withdraw cash, and stored data distinguishes the channel. Through the admin web app an analyst can look up customers, accounts and transactions.
- After documented prerequisites, one Docker Compose command starts all server-side services on the development host. Installing the mobile app is separate.
- At steady state every service stays within its configured CPU and memory limit, and retained structured data stays within the chosen Oracle edition's limits for the spec-defined retention period.
- Every container image is available for both linux/arm64 and linux/amd64, and the Rust services build for both.

Detection (later):

- Detection never gates a transaction. Under the spec-defined workload, the p95 delay from the last event a scenario needs to the alert, and the p95 transaction-latency overhead with detection running or stopped, meet spec-defined bounds.
- On held-out labelled runs that include benign lookalikes, detection meets spec-defined per-scenario precision and recall targets and an alert-volume limit for analysts.
- An automated pipeline completes an initial training-to-serving cycle and a retraining on newer data, with traceable dataset, model and deployed-version identities.

## Open questions

- Mobile app and admin web app technology stacks.
- Whether the simulated ATM needs a human-usable interface, or is an API surface driven by the generator.
- Oracle Database edition (Oracle Database Free in a container fits Docker Compose and supports ARM64) and the retention period its data limit allows.
- Whether OCI is ever a deployment target, or only the development host.
- How events reach detection in real time (for example a message broker such as Kafka, or change data capture), and when it is introduced.
- Detection approach and MLOps tooling (rules, ML scoring, workflow orchestration, model registry), and whether model training may run on the same host as the bank.
- Generator volume (customers, transactions per minute), backfill length, and scenario frequencies.
- Authentication model for customers and analysts.
- Concrete thresholds, for example the simulated reporting threshold for D (modelled on the Korean currency transaction report, which aggregates daily cash transactions).
