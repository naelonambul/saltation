use super::{Connector, DbConfig, DbError};

#[derive(Clone)]
pub(super) struct OracleConnector(pub(super) DbConfig);

impl Connector for OracleConnector {
    type Conn = oracle::Connection;
    type Error = DbError;

    fn open(&self) -> Result<Self::Conn, Self::Error> {
        let config = &self.0;
        let mut connector =
            oracle::Connector::new(&config.user, &config.password, &config.connect_string);
        connector.stmt_cache_size(config.stmt_cache_size);
        let conn = connector.connect()?;
        conn.set_call_timeout(Some(config.call_timeout))?;
        let zone: String = conn.query_row_as("SELECT SESSIONTIMEZONE FROM DUAL", &[])?;
        if zone != "+09:00" {
            let _ = conn.close();
            return Err(DbError::TimeZone(zone));
        }
        Ok(conn)
    }

    fn close(&self, conn: Self::Conn) {
        let _ = conn.close();
    }
}
