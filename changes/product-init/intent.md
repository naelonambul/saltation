# Intent

## Problem

Fraud detection systems (FDS) and the MLOps pipelines behind them are hard to learn and hard to demonstrate without access to a real bank. Public fraud datasets are static tables: they carry no live event stream, no login, device or session context, no channel (app, ATM) detail, and no way to inject a new fraud pattern and watch a pipeline react to it. Real banking data cannot be used for privacy and legal reasons.

The owner is a data-engineering and MLOps bootcamp student building a personal portfolio project, "real-time financial fraud anomaly detection MLOps pipeline", and needs a realistic, continuously running source of banking activity, including fraud with known ground truth, to build that pipeline on.

## Proposed outcome

Saltation is a self-contained simulated bank with a fraud-detection platform built on top of it, developed incrementally with no fixed end date.

1. **A bank that produces realistic data.** A Rust banking server provides accounts, deposits, withdrawals, transfers, payees, authentication, and device and session tracking, across a mobile app channel and a simulated ATM channel. A customer-facing mobile app and an operator-facing admin web app sit on top of it.
2. **Persona-driven synthetic activity.** A synthetic generator drives the bank through its public interfaces as a population of personas: ordinary customers (for example office workers, students, self-employed, elderly) and fraud actors. Fraud actors play the scenarios below, interleaved with normal behaviour. The generator records ground truth (which events belong to which scenario) separately from the bank's own data.
3. **Data collection first.** Structured banking records are stored in a relational database, and unstructured logs (application, access, session and device events) are collected into a document store, so that a growing, labelled history accumulates. This is the first priority.
4. **Real-time detection, later.** Detection consumes bank activity as a stream, in real time but asynchronously, so it never blocks a transaction. It raises alerts that operators review in the admin web app. Rule-based and machine-learning detection, and the MLOps around the models (training, versioning, deployment, retraining), are built after data collection is in place.

### Fraud scenarios

All of these are in scope for the product. Each must be producible by the generator and, once detection exists, detectable.

| ID | Scenario | Characteristic signals |
|---|---|---|
| A | Account takeover | Login from a new device; failed-login bursts; password reset, then new payee, then high-value transfer; abrupt change of access location |
| B | Voice-phishing victim | The victim acts under a fraudster's instruction with valid credentials: unusually large outflows, often withdrawn as cash at ATMs, sometimes right after closing a deposit or taking a loan; transfers to never-seen payees |
| C | Money mule account | Inflows from many unrelated senders followed quickly by outflows, frequently ATM cash withdrawals; new or dormant accounts suddenly active; devices or IP addresses shared across accounts |
| D | Structuring | Repeated amounts just below a reporting threshold |
| E | Velocity and amount anomaly | Transaction frequency or amount far outside the customer's own baseline |
| F | Time and location anomaly | Activity at hours the customer never uses; access from unusual or foreign locations |

## Affected users and systems

- **Owner (developer and operator):** builds, deploys and runs the system; presents it as a portfolio project.
- **Simulated customers:** generator personas using the mobile and ATM channels. A human may also use the mobile app directly.
- **Fraud operators:** use the admin web app to inspect customers, accounts and transactions and, once detection exists, to review alerts.
- **Systems:** Rust banking server, mobile app, admin web app, synthetic generator, relational database, document store for logs, the OCI development host, and the VMware lab VMs.

## Constraints

- The banking server is written in Rust.
- Structured data lives in Oracle Database; unstructured logs live in MongoDB. Consolidating stores later (for example onto PostgreSQL) must remain possible.
- The primary environment is Docker Compose on an Oracle Cloud Infrastructure development host the owner reaches over SSH. The host is ARM64 (Ampere, 4 cores, about 24 GB RAM); the stack must fit within that host's resources.
- A secondary, occasional environment is a local lab: VMware Workstation Pro on an x86-64 Windows host with WSL, provisioned with Terraform and configured with Ansible. It is not required for every change, but design choices should avoid locking the stack to ARM64 (for example by preferring images available for both architectures). A move to Kubernetes comes later.
- All data is synthetic. No real personal or financial data is ever used.
- Detection is real-time and asynchronous: it processes events as they happen, and it must never block or delay a banking transaction.
- Ground-truth labels are kept apart from the bank's operational data, so detection cannot read them.
- One developer, no deadline; the product grows through successive changes.

## Out of scope

- Real money movement, real payment networks, interbank settlement, or integration with real banks.
- Real customer onboarding, identity verification, or regulatory reporting.
- Synchronous (inline) blocking of transactions by the detector.
- Production-grade security certification or compliance; the system is a demonstration, but it must not be insecure by design.
- Physical ATM hardware; the ATM is a simulated channel.

## Success criteria

Data foundation (first):

- The generator runs unattended for at least 24 hours against the deployed bank, producing normal activity for every persona type and every fraud scenario A to F.
- Each fraud scenario instance leaves a complete trail: its transactions in Oracle Database, and the related login, session, device and channel events in MongoDB.
- Every generated event can be joined to its ground-truth label by an identifier, without the label being visible to the bank.
- A customer can sign in, view balances and history, and transfer money through the mobile app; an operator can look up customers, accounts and transactions in the admin web app.
- The whole stack starts from the repository with one Docker Compose command on the development host.
- The stack does not depend on ARM64-only components, so it can later be stood up on x86-64 VMware lab VMs with Terraform and Ansible.

Detection (later):

- Real-time detection raises alerts for scenarios A to F within seconds of the triggering event, without affecting transaction latency, and operators can review alerts in the admin web app.
- Detection quality is measured against ground truth (for example precision and recall per scenario).
- Models are trained, versioned, deployed and retrained through an automated pipeline.

## Open questions

- Mobile app and admin web app technology stacks.
- Oracle Database flavour (for example Oracle Database Free in a container, which fits Docker Compose).
- Whether OCI is ever a deployment target, or only the development host.
- How events reach detection asynchronously (for example a message broker such as Kafka, or change data capture), and when it is introduced.
- Detection approach and MLOps tooling (rules, ML scoring, workflow orchestration, model registry), decided when detection is designed.
- Target generator volume (customers, transactions per minute) and how much history to retain.
- Authentication model for customers and operators.
- Concrete thresholds for scenarios D to F (for example the structuring threshold, modelled on the Korean currency transaction report limit).
