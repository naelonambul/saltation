//! The bank's supported bind and owned row types. Driver conversions stay here.
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use oracledb::{DbType, Error, OracleTimestamp, Row, ToDbValue};

fn kst() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).expect("valid offset")
}

/// A value supported by the bank's SQL adapter. SQL BOOLEAN is deliberately absent.
pub trait ToSql {
    #[doc(hidden)]
    fn bind_value(&self) -> Box<dyn ToDbValue + '_>;
    #[doc(hidden)]
    fn null_type() -> &'static DbType
    where
        Self: Sized;
}

macro_rules! binds {
    ($($ty:ty => $db:expr),* $(,)?) => {$(
        impl ToSql for $ty {
            fn bind_value(&self) -> Box<dyn ToDbValue + '_> { Box::new(self.clone()) }
            fn null_type() -> &'static DbType { $db }
        }
    )*};
}
binds! {
    i32 => oracledb::DB_TYPE_NUMBER,
    i64 => oracledb::DB_TYPE_NUMBER,
    u32 => oracledb::DB_TYPE_NUMBER,
    u64 => oracledb::DB_TYPE_NUMBER,
    f64 => oracledb::DB_TYPE_BINARY_DOUBLE,
    String => oracledb::DB_TYPE_VARCHAR,
    &str => oracledb::DB_TYPE_VARCHAR,
}

impl ToSql for NaiveDate {
    fn bind_value(&self) -> Box<dyn ToDbValue + '_> {
        Box::new(OracleTimestamp::from(
            self.and_hms_opt(0, 0, 0).expect("midnight"),
        ))
    }
    fn null_type() -> &'static DbType {
        oracledb::DB_TYPE_TIMESTAMP
    }
}

impl ToSql for NaiveDateTime {
    fn bind_value(&self) -> Box<dyn ToDbValue + '_> {
        Box::new(OracleTimestamp::from(*self))
    }
    fn null_type() -> &'static DbType {
        oracledb::DB_TYPE_TIMESTAMP
    }
}

impl ToSql for DateTime<FixedOffset> {
    fn bind_value(&self) -> Box<dyn ToDbValue + '_> {
        // A non-UTC OracleTimestamp binds TIMESTAMP, letting the checked KST
        // session supply the stored offset when Oracle converts it to TSTZ.
        Box::new(OracleTimestamp::from(
            self.with_timezone(&kst()).naive_local(),
        ))
    }
    fn null_type() -> &'static DbType {
        oracledb::DB_TYPE_TIMESTAMP
    }
}

impl<T: ToSql> ToSql for Option<T> {
    fn bind_value(&self) -> Box<dyn ToDbValue + '_> {
        match self {
            Some(value) => value.bind_value(),
            None => Box::new(T::null_type()),
        }
    }
    fn null_type() -> &'static DbType {
        T::null_type()
    }
}

impl<T: ToSql> ToSql for &T {
    fn bind_value(&self) -> Box<dyn ToDbValue + '_> {
        (*self).bind_value()
    }
    fn null_type() -> &'static DbType {
        T::null_type()
    }
}

pub(super) fn with_binds<T>(params: &[&dyn ToSql], f: impl FnOnce(&[&dyn ToDbValue]) -> T) -> T {
    let values: Vec<_> = params.iter().map(|v| v.bind_value()).collect();
    let refs: Vec<&dyn ToDbValue> = values.iter().map(|v| v.as_ref()).collect();
    f(&refs)
}

/// An owned column value supported by the bank's row conversion.
pub trait ColumnValue: Sized {
    fn read(row: &Row, index: usize) -> Result<Self, Error>;
}

macro_rules! columns {
    ($($ty:ty),* $(,)?) => {$(
        impl ColumnValue for $ty {
            fn read(row: &Row, index: usize) -> Result<Self, Error> { row.get(index) }
        }
        impl ColumnValue for Option<$ty> {
            fn read(row: &Row, index: usize) -> Result<Self, Error> { row.get(index) }
        }
    )*};
}
columns!(i32, i64, u32, u64, f64, String);

impl ColumnValue for NaiveDate {
    fn read(row: &Row, index: usize) -> Result<Self, Error> {
        Ok(NaiveDateTime::try_from(row.get::<OracleTimestamp>(index)?)?.date())
    }
}
impl ColumnValue for Option<NaiveDate> {
    fn read(row: &Row, index: usize) -> Result<Self, Error> {
        row.get::<Option<OracleTimestamp>>(index)?
            .map(|ts| Ok(NaiveDateTime::try_from(ts)?.date()))
            .transpose()
    }
}
impl ColumnValue for NaiveDateTime {
    fn read(row: &Row, index: usize) -> Result<Self, Error> {
        NaiveDateTime::try_from(row.get::<OracleTimestamp>(index)?)
    }
}
impl ColumnValue for Option<NaiveDateTime> {
    fn read(row: &Row, index: usize) -> Result<Self, Error> {
        row.get::<Option<OracleTimestamp>>(index)?
            .map(NaiveDateTime::try_from)
            .transpose()
    }
}
fn timestamp(ts: OracleTimestamp) -> Result<DateTime<FixedOffset>, Error> {
    let wall = NaiveDateTime::try_from(ts)?;
    if ts.is_utc() {
        Ok(wall.and_utc().with_timezone(&kst()))
    } else {
        Ok(kst()
            .from_local_datetime(&wall)
            .single()
            .expect("fixed offset"))
    }
}
impl ColumnValue for DateTime<FixedOffset> {
    fn read(row: &Row, index: usize) -> Result<Self, Error> {
        timestamp(row.get(index)?)
    }
}
impl ColumnValue for Option<DateTime<FixedOffset>> {
    fn read(row: &Row, index: usize) -> Result<Self, Error> {
        row.get::<Option<OracleTimestamp>>(index)?
            .map(timestamp)
            .transpose()
    }
}

/// Converts a row to an owned scalar or tuple.
pub trait RowValue: Sized {
    fn from_row(row: &Row) -> Result<Self, Error>;
}
impl<T: ColumnValue> RowValue for T {
    fn from_row(row: &Row) -> Result<Self, Error> {
        T::read(row, 0)
    }
}
macro_rules! tuple {
    ($($ty:ident : $index:tt),+) => {
        impl<$($ty: ColumnValue),+> RowValue for ($($ty,)+) {
            fn from_row(row: &Row) -> Result<Self, Error> { Ok(($($ty::read(row, $index)?,)+)) }
        }
    };
}
tuple!(A: 0);
tuple!(A: 0, B: 1);
tuple!(A: 0, B: 1, C: 2);
tuple!(A: 0, B: 1, C: 2, D: 3);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4, F: 5);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4, F: 5, G: 6);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4, F: 5, G: 6, H: 7);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4, F: 5, G: 6, H: 7, I: 8);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4, F: 5, G: 6, H: 7, I: 8, J: 9);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4, F: 5, G: 6, H: 7, I: 8, J: 9, K: 10);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4, F: 5, G: 6, H: 7, I: 8, J: 9, K: 10, L: 11);

/// The driver validates positional counts before executing. Only repeated SQL
/// binds have more positions than distinct names; PL/SQL already deduplicates.
/// Keep the ordinary positional path and retry that validation failure by name.
pub(super) fn with_statement_binds<T>(
    conn: &oracledb::Connection,
    sql: &str,
    params: &[&dyn ToSql],
    positional: impl FnOnce(oracledb::Statement, &[&dyn ToDbValue]) -> Result<T, Error>,
    named: impl FnOnce(oracledb::Statement, &[(&str, &dyn ToDbValue)]) -> Result<T, Error>,
) -> Result<T, Error> {
    with_binds(params, |values| {
        let statement = conn.statement(sql)?.build()?;
        match positional(statement, values) {
            Err(error) if matches!(error.kind(), oracledb::ErrorKind::WrongNumPositionalBinds(expected, actual) if expected > actual) =>
            {
                let statement = conn.statement(sql)?.build()?;
                if statement.bind_names().len() != values.len() {
                    return Err(error);
                }
                // Quoting preserves the driver's normalized names, including
                // case-sensitive quoted bind identifiers.
                let names: Vec<_> = statement
                    .bind_names()
                    .iter()
                    .map(|name| format!("\"{name}\""))
                    .collect();
                let binds: Vec<_> = names
                    .iter()
                    .zip(values)
                    .map(|(name, value)| (name.as_str(), *value))
                    .collect();
                named(statement, &binds)
            }
            result => result,
        }
    })
}
