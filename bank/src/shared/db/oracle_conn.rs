use super::{Connector, DbConfig, DbError};

#[derive(Clone)]
pub(super) struct OracleConnector(pub(super) DbConfig);

impl Connector for OracleConnector {
    type Conn = oracledb::Connection;
    type Error = DbError;

    fn open(&self) -> Result<Self::Conn, Self::Error> {
        let config = &self.0;
        let connector = oracledb::Config::default()
            .set_credentials(&config.user, &config.password)
            .set_stmtcachesize(config.stmt_cache_size as usize)
            .set_transport_connect_timeout(Some(std::time::Duration::from_secs(2)))
            .set_connect_string(&config.connect_string)?;
        let mut conn = oracledb::connect(connector)?;
        conn.set_call_timeout(Some(config.call_timeout))?;
        let zone: String = conn
            .query_row("SELECT SESSIONTIMEZONE FROM DUAL", &[])?
            .get(0)?;
        if zone != "+09:00" {
            let _ = conn.close();
            return Err(DbError::TimeZone(zone));
        }
        Ok(conn)
    }

    fn close(&self, mut conn: Self::Conn) {
        let _ = conn.close();
    }
}
