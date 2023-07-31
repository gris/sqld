use crate::database::{Database, InjectableDatabase};
use crate::error::Error;

use super::connection::WriteProxyConnection;
use super::WaitFrameNoCb;

pub struct WriteProxyDatabase<RDB, WDB> {
    read_db: RDB,
    write_db: WDB,
    wait_frame_no_cb: WaitFrameNoCb,
}

impl<RDB, WDB> WriteProxyDatabase<RDB, WDB> {
    pub fn new(read_db: RDB, write_db: WDB, wait_frame_no_cb: WaitFrameNoCb) -> Self {
        Self {
            read_db,
            write_db,
            wait_frame_no_cb,
        }
    }
}

impl<RDB, WDB> Database for WriteProxyDatabase<RDB, WDB>
where
    RDB: Database,
    WDB: Database,
    WDB::Connection: Clone + Send + 'static,
{
    type Connection = WriteProxyConnection<RDB::Connection, WDB::Connection>;
    /// Create a new connection to the database
    fn connect(&self) -> Result<Self::Connection, Error> {
        Ok(WriteProxyConnection {
            read_conn: self.read_db.connect()?,
            write_conn: self.write_db.connect()?,
            wait_frame_no_cb: self.wait_frame_no_cb.clone(),
            state: Default::default(),
        })
    }
}

impl<RDB, WDB> InjectableDatabase for WriteProxyDatabase<RDB, WDB>
where
    RDB: InjectableDatabase,
{
    fn injector(&self) -> crate::Result<Box<dyn crate::database::Injector + Send + 'static>> {
        self.read_db.injector()
    }
}
