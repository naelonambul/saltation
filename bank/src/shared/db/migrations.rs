//! Numbered plain-SQL migrations from `bank/migrations/`. A file may hold
//! several statements separated by a line containing only `/`. Oracle DDL
//! commits implicitly, so a failed migration can leave earlier statements of
//! the same file applied; migrations are written to be re-runnable where that
//! matters, and development databases are recreated rather than repaired.

use super::{DbError, Session, SqlStmt};
use crate::sql_stmt;

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

macro_rules! migration {
    ($version:literal, $name:literal) => {
        Migration {
            version: $version,
            name: $name,
            sql: include_str!(concat!("../../../migrations/", $name, ".sql")),
        }
    };
}

pub const MIGRATIONS: &[Migration] = &[
    migration!(1, "0001_reference"),
    migration!(2, "0002_customers"),
    migration!(3, "0003_accounts"),
    migration!(4, "0004_ledger"),
    migration!(5, "0005_reference_data"),
    migration!(6, "0006_transfer_indexes"),
    migration!(7, "0007_parties"),
    migration!(8, "0008_payees_reset"),
    migration!(9, "0009_products"),
    migration!(10, "0010_rollover"),
    migration!(11, "0011_performance"),
];

const CREATE_TABLE: SqlStmt = sql_stmt!(
    "db.create_migration_table",
    "sql/create_migration_table.sql"
);
const SELECT_APPLIED: SqlStmt = sql_stmt!(
    "db.select_applied_versions",
    "sql/select_applied_versions.sql"
);
const INSERT_APPLIED: SqlStmt = sql_stmt!(
    "db.insert_applied_version",
    "sql/insert_applied_version.sql"
);

/// Splits a migration file into statements on lines that contain only `/`.
pub fn statements(sql: &'static str) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    for line in sql.split_inclusive('\n') {
        if line.trim() == "/" {
            out.push(sql[start..offset].trim());
            start = offset + line.len();
        }
        offset += line.len();
    }
    out.push(sql[start..].trim());
    out.into_iter()
        .filter(|s| !s.is_empty() && !is_comment_only(s))
        .collect()
}

fn is_comment_only(s: &str) -> bool {
    s.lines()
        .all(|l| l.trim().is_empty() || l.trim_start().starts_with("--"))
}

pub(super) fn apply(session: &mut Session) -> Result<usize, DbError> {
    session.execute(&CREATE_TABLE, &[])?;
    let applied: Vec<i64> = session.query_all(&SELECT_APPLIED, &[])?;
    let mut count = 0;
    for m in MIGRATIONS.iter().filter(|m| !applied.contains(&m.version)) {
        for sql in statements(m.sql) {
            session.execute(&SqlStmt { name: m.name, sql }, &[])?;
        }
        session.execute(&INSERT_APPLIED, &[&m.version, &m.name])?;
        session.commit()?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_slash_lines_and_drops_comment_only_parts() {
        let parts = statements(
            "-- header\nCREATE TABLE a (x NUMBER)\n/\nBEGIN\n  NULL;\nEND;\n/\n-- trailing comment\n",
        );
        assert_eq!(
            parts,
            vec![
                "-- header\nCREATE TABLE a (x NUMBER)",
                "BEGIN\n  NULL;\nEND;"
            ]
        );
    }

    #[test]
    fn versions_are_unique_and_ascending() {
        let versions: Vec<i64> = MIGRATIONS.iter().map(|m| m.version).collect();
        let mut sorted = versions.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(versions, sorted);
    }
}
