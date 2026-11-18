//! Plan and correctness witnesses for sequence-driven history and latest-date reads.
use super::*;
use bank::shared::db::{DbError, Session};

const HISTORY_ACCOUNT: SqlStmt = sql_stmt!(
    "accounts.select_history_account",
    "../../src/features/accounts/sql/select_history_account.sql"
);

const OWNER: SqlStmt = sql_stmt!("test.accounts_tune_owner", "../sql/accounts_tune_owner.sql");
const PREVIOUS: SqlStmt = sql_stmt!(
    "test.accounts_tune_previous",
    "../sql/accounts_tune_previous.sql"
);
const PLAN: SqlStmt = sql_stmt!("test.accounts_tune_plan", "../sql/accounts_tune_plan.sql");
const ASC: SqlStmt = sql_stmt!(
    "accounts.select_transactions_asc",
    "../../src/features/accounts/sql/select_transactions_asc.sql"
);
const DESC: SqlStmt = sql_stmt!(
    "accounts.select_transactions_desc",
    "../../src/features/accounts/sql/select_transactions_desc.sql"
);
const BALANCE: SqlStmt = sql_stmt!(
    "accounts.select_balance",
    "../../src/features/accounts/sql/select_balance.sql"
);

type At = DateTime<FixedOffset>;
type HistoryRow = (i64, At, i64, i64, String, String, Option<String>);
type BalanceRow = (
    i64,
    String,
    i64,
    String,
    String,
    At,
    Option<String>,
    Option<String>,
);

#[tokio::test]
async fn latest_date_logical_reads_do_not_grow_with_account_depth() {
    let api = api().await;
    let customer = seed(&api, 2).await;
    let now = api.state.clock.state().await.unwrap().business_time;
    api.state
        .db
        .run(move |s| {
            let mut owners = Vec::new();
            let insert = sql_stmt!("test.accounts_tune_depth", "../sql/accounts_tune_depth.sql");
            for (account, depth) in customer.accounts.into_iter().zip([80_i64, 50_000_i64]) {
                let owner: (i64, i64, String) = s.query_row(&OWNER, &[&account])?;
                let request = new_id();
                s.execute(&INSERT_REQUEST, &[&request, &request, &now])?;
                s.execute(&insert, &[&owner.0, &request, &now, &depth])?;
                owners.push(owner);
            }
            for (_, customer_id, fin) in &owners {
                let _: BalanceRow = s.query_row(&BALANCE, &[fin, customer_id])?;
            }
            let (id, child): (String, i64) = s.query_row(&PREVIOUS, &[])?;
            let buffers = sql_stmt!(
                "test.accounts_tune_buffers",
                "../sql/accounts_tune_buffers.sql"
            );
            let mut costs = Vec::new();
            for (_, customer_id, fin) in &owners {
                let before: (u64, u64) = s.query_row(&buffers, &[&id, &child])?;
                let _: BalanceRow = s.query_row(&BALANCE, &[fin, customer_id])?;
                let after: (u64, u64) = s.query_row(&buffers, &[&id, &child])?;
                assert_eq!(
                    after.1 - before.1,
                    1,
                    "exactly one production execution measured"
                );
                costs.push(after.0 - before.0);
            }
            assert!(costs[0] > 0);
            assert_eq!(
                costs[0], costs[1],
                "account depth does not increase latest-date reads"
            );
            s.rollback()
        })
        .await
        .unwrap();
}

fn plan(s: &mut Session) -> Result<String, DbError> {
    let (id, child): (String, i64) = s.query_row(&PREVIOUS, &[])?;
    let lines: Vec<String> = s.query_all(&PLAN, &[&id, &child])?;
    let plan = lines.join("\n");
    assert!(!plan.contains("cannot fetch"), "{plan}");
    Ok(plan)
}

#[tokio::test]
async fn sequence_pages_keep_exact_time_filters_without_sorting_the_window() {
    let api = api().await;
    let customer = seed(&api, 1).await;
    let account = customer.accounts[0].clone();
    let now = api.state.clock.state().await.unwrap().business_time;
    api.state
        .db
        .run(move |s| {
            let (account_id, customer_id, fin): (i64, i64, String) =
                s.query_row(&OWNER, &[&account])?;
            let request = new_id();
            s.execute(&INSERT_REQUEST, &[&request, &request, &now])?;
            let mut balance = 0_i64;
            // Time and sequence deliberately disagree: bounds must not assume
            // monotonic timestamps or skip the exact predicate after the seek.
            for seq in 1..=80_i64 {
                let amount = if seq % 2 == 1 { 10_000_i64 } else { -3_000_i64 };
                balance += amount;
                let at = now - Duration::days(if seq % 2 == 0 { 45 } else { 0 });
                let inout = if amount > 0 { "입금" } else { "출금" };
                s.execute(
                    &INSERT_ROW,
                    &[
                        &account_id,
                        &seq,
                        &amount,
                        &balance,
                        &inout,
                        &format!("Tune {seq}"),
                        &request,
                        &at,
                        &at,
                    ],
                )?;
            }
            s.execute(&SET_BALANCE, &[&balance, &81_i64, &now, &account])?;
            s.commit()?;
            for old in [false, true] {
                let from = now - Duration::days(if old { 60 } else { 1 });
                let to = now + Duration::days(if old { -30 } else { 1 });
                let bounds: (i64, String, i64, Option<i64>, Option<i64>) =
                    s.query_row(&HISTORY_ACCOUNT, &[&from, &to, &fin, &customer_id])?;
                let (first, last) = bounds.3.zip(bounds.4).expect("window has rows");
                for descending in [false, true] {
                    for deep in [false, true] {
                        let cursor = match (descending, deep) {
                            (true, false) => i64::MAX,
                            (false, false) => 0,
                            (_, true) => 40,
                        };
                        let statement = if descending { DESC } else { ASC };
                        let bounded_cursor = if descending {
                            cursor.min(last + 1)
                        } else {
                            cursor.max(first - 1)
                        };
                        let opposite_bound = if descending { first } else { last };
                        let rows: Vec<HistoryRow> = s.query_all_prefetched(
                            &statement,
                            &[
                                &account_id,
                                &from,
                                &to,
                                &"A",
                                &"A",
                                &"A",
                                &bounded_cursor,
                                &opposite_bound,
                                &26_i64,
                            ],
                            27,
                        )?;
                        let shape = plan(s)?;
                        assert!(shape.contains("ACCOUNT_TRANSACTION_SEQ_UK"), "{shape}");
                        assert!(!shape.contains("WINDOW SORT"), "{shape}");
                        assert!(!shape.contains("SORT ORDER BY"), "{shape}");
                        let mut expected: Vec<i64> = (1..=80_i64)
                            .filter(|seq| {
                                (seq % 2 == 0) == old
                                    && if descending {
                                        *seq < cursor
                                    } else {
                                        *seq > cursor
                                    }
                            })
                            .collect();
                        if descending {
                            expected.reverse();
                        }
                        expected.truncate(26);
                        assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), expected);
                    }
                }
            }
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn latest_business_date_uses_one_index_entry() {
    let api = api().await;
    let customer = seed(&api, 1).await;
    let account = customer.accounts[0].clone();
    let now = api.state.clock.state().await.unwrap().business_time;
    insert_rows(&api, &account, 80, now).await;
    api.state
        .db
        .run(move |s| {
            let (_, customer_id, fin): (i64, i64, String) = s.query_row(&OWNER, &[&account])?;
            let row: BalanceRow = s.query_row(&BALANCE, &[&fin, &customer_id])?;
            assert_eq!(row.6, Some(now.format("%Y%m%d").to_string()));
            let shape = plan(s)?;
            assert!(shape.contains("ACCOUNT_TRANSACTION_DATE_IX"), "{shape}");
            assert!(
                shape.contains("(MIN/MAX)") || shape.contains("INDEX RANGE SCAN DESCENDING"),
                "{shape}"
            );
            assert!(
                !shape
                    .lines()
                    .any(|line| line.contains("TABLE ACCESS")
                        && line.contains("ACCOUNT_TRANSACTION")),
                "{shape}"
            );
            Ok(())
        })
        .await
        .unwrap();
}
