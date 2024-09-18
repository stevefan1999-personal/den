use std::{cell::RefCell, rc::Rc};

use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::class::Trace;

#[derive(Trace, Derivative, From, Into, Deref, DerefMut)]
#[derivative(Debug, Clone)]
#[rquickjs::class(rename = "Connection")]
pub struct Connection {
    #[qjs(skip_trace)]
    conn: Rc<RefCell<Option<rusqlite::Connection>>>,
}

#[rquickjs::methods]
impl Connection {
    #[qjs(constructor)]
    pub fn new() {}

    #[qjs(static)]
    pub fn open_in_memory<'js>(ctx: rquickjs::Ctx<'js>) -> rquickjs::Result<Connection> {
        let conn = rusqlite::Connection::open_in_memory()
            .map_err(|e| rquickjs::Exception::throw_internal(&ctx, &format!("{e}")))?;
        Ok(Connection {
            conn: Rc::new(RefCell::new(Some(conn))),
        })
    }

    #[qjs(static)]
    pub fn open<'js>(path: String, ctx: rquickjs::Ctx<'js>) -> rquickjs::Result<Connection> {
        let conn = rusqlite::Connection::open(path)
            .map_err(|e| rquickjs::Exception::throw_internal(&ctx, &format!("{e}")))?;
        Ok(Connection {
            conn: Rc::new(RefCell::new(Some(conn))),
        })
    }

    // TODO: multi-parameters
    pub fn execute<'js>(self, sql: String, ctx: rquickjs::Ctx<'js>) -> rquickjs::Result<usize> {
        if let Some(conn) = self.conn.borrow().deref() {
            Ok(conn
                .execute(&sql, [])
                .map_err(|e| rquickjs::Exception::throw_internal(&ctx, &format!("{e}")))?)
        } else {
            Err(rquickjs::Exception::throw_internal(&ctx, "already closed"))
        }
    }

    pub fn close<'js>(self, ctx: rquickjs::Ctx<'js>) -> rquickjs::Result<()> {
        if let Some(conn) = self.conn.borrow_mut().take() {
            conn.close()
                .map_err(|(_, e)| rquickjs::Exception::throw_internal(&ctx, &format!("{e}")))?;

            Ok(())
        } else {
            Err(rquickjs::Exception::throw_internal(&ctx, "already closed"))
        }
    }
}

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "PascalCase"
)]
pub mod sqlite {
    pub use super::Connection;
}
