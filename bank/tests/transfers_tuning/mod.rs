//! Both daily-total keys must be access predicates under the real adapter.
use super::*;

const OWNER: SqlStmt = sql_stmt!("test.ledger_tune_owner", "../sql/accounts_tune_owner.sql");
const PREVIOUS: SqlStmt = sql_stmt!(
    "test.ledger_tune_previous",
    "../sql/accounts_tune_previous.sql"
);
const ACCESS: SqlStmt = sql_stmt!("test.ledger_tune_access", "../sql/ledger_tune_access.sql");
const ADD_DAILY: SqlStmt = sql_stmt!(
    "ledger.add_daily",
    "../../src/shared/ledger/sql/add_daily.sql"
);

#[tokio::test]
async fn daily_totals_match_both_index_keys_without_converting_the_column() {
    let api = api().await;
    let customer = common::customer(&api, 0).await;
    let date = api
        .state
        .clock
        .state()
        .await
        .unwrap()
        .business_time
        .date_naive();
    api.state
        .db
        .run(move |s| {
            let (account_id, _, _): (i64, i64, String) =
                s.query_row(&OWNER, &[&customer.account])?;
            bank::shared::ledger::lock_accounts(s, &[account_id])?;
            s.execute(
                &sql_stmt!(
                    "test.ledger_tune_calendar",
                    "../sql/ledger_tune_calendar.sql"
                ),
                &[],
            )?;
            s.execute(&ADD_DAILY, &[&account_id, &date, &1_i64, &0_i64, &0_i64])?;
            let (id, child): (String, i64) = s.query_row(&PREVIOUS, &[])?;
            let predicates: Vec<(Option<String>, Option<String>)> =
                s.query_all(&ACCESS, &[&id, &child])?;
            assert!(
                predicates
                    .iter()
                    .any(|(access, _)| access.as_ref().is_some_and(|text| {
                        text.contains("ACCOUNT_ID") && text.contains("BUSINESS_DATE")
                    })),
                "{predicates:?}"
            );
            assert!(
                !predicates.iter().any(|(access, filter)| access
                    .iter()
                    .chain(filter.iter())
                    .any(|text| text.contains("INTERNAL_FUNCTION"))),
                "{predicates:?}"
            );
            let stored: chrono::NaiveDate = s.query_row(
                &sql_stmt!("test.ledger_tune_date", "../sql/ledger_tune_date.sql"),
                &[&account_id, &date],
            )?;
            assert_eq!(
                stored, date,
                "non-Gregorian session keeps the same stored date"
            );
            s.execute(
                &sql_stmt!(
                    "test.ledger_tune_calendar_reset",
                    "../sql/ledger_tune_calendar_reset.sql"
                ),
                &[],
            )?;
            s.rollback()
        })
        .await
        .unwrap();
}
