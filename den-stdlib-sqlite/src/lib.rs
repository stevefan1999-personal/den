use std::{cell::RefCell, sync::Arc};

use derivative::Derivative;
use derive_more::{
    derive::{Debug, Display, Error},
    Deref, DerefMut, From, Into,
};
use either::Either;
use rquickjs::{
    class::Trace, prelude::*, Array, BigInt, Ctx, Exception, JsLifetime, Object, Result, Value,
};
use rusqlite::Statement;

#[derive(Trace, JsLifetime, Derivative, From, Into, Deref, DerefMut)]
#[derivative(Debug, Clone)]
#[rquickjs::class(rename = "Connection")]
pub struct Connection {
    #[qjs(skip_trace)]
    conn: Arc<RefCell<Option<rusqlite::Connection>>>,
}

#[rquickjs::methods]
impl Connection {
    #[qjs(constructor)]
    pub fn new() {}

    #[qjs(static)]
    pub fn open_in_memory(ctx: Ctx<'_>) -> Result<Connection> {
        let conn = rusqlite::Connection::open_in_memory()
            .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
        Ok(Connection {
            conn: Arc::new(RefCell::new(Some(conn))),
        })
    }

    #[qjs(static)]
    pub fn open(path: String, ctx: Ctx<'_>) -> Result<Connection> {
        let conn = rusqlite::Connection::open(path)
            .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
        Ok(Connection {
            conn: Arc::new(RefCell::new(Some(conn))),
        })
    }

    pub fn execute<'js>(
        self,
        sql: String,
        Opt(params): Opt<Either<Array<'js>, Object<'js>>>,
        ctx: Ctx<'js>,
    ) -> Result<usize> {
        if let Some(conn) = self.conn.borrow().deref() {
            let stmt = conn.prepare(&sql);
            let mut stmt = stmt.map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
            match params {
                Some(Either::Left(params)) => {
                    bind_parameters_from_rquickjs_array(&mut stmt, params, ctx.clone())?;
                }
                Some(Either::Right(params)) => {
                    bind_parameters_from_rquickjs_object(&mut stmt, params, ctx.clone())?;
                }
                None => {}
            }

            Ok(stmt
                .raw_execute()
                .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?)
        } else {
            Err(Exception::throw_internal(&ctx, "already closed"))
        }
    }

    pub fn query_rows<'js>(
        self,
        sql: String,
        Opt(params): Opt<Either<Array<'js>, Object<'js>>>,
        ctx: Ctx<'js>,
    ) -> Result<Option<Array<'js>>> {
        if let Some(conn) = self.conn.borrow().deref() {
            let stmt = conn.prepare(&sql);
            let mut stmt = stmt.map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
            match params {
                Some(Either::Left(params)) => {
                    bind_parameters_from_rquickjs_array(&mut stmt, params, ctx.clone())?;
                }
                Some(Either::Right(params)) => {
                    bind_parameters_from_rquickjs_object(&mut stmt, params, ctx.clone())?;
                }
                None => {}
            }
            execute_stmt_and_collect_rows(&mut stmt, ctx)
        } else {
            Err(Exception::throw_internal(&ctx, "already closed"))
        }
    }

    pub fn close(self, ctx: Ctx<'_>) -> Result<()> {
        if let Some(conn) = self.conn.borrow_mut().take() {
            conn.close()
                .map_err(|(_, e)| Exception::throw_internal(&ctx, &format!("{e}")))?;

            Ok(())
        } else {
            Err(Exception::throw_internal(&ctx, "already closed"))
        }
    }
}

fn bind_parameters_from_rquickjs_object<'js>(
    stmt: &mut Statement<'_>,
    params: Object<'js>,
    ctx: Ctx<'js>,
) -> Result<()> {
    if params.len() > stmt.parameter_count() {
        return Err(Exception::throw_internal(&ctx, "too many parameters"));
    }

    for param in params.into_iter() {
        let (key, value) = param?;
        let named_key = format!(":{}", key.to_string()?);
        let idx = stmt
            .parameter_index(&named_key)
            .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?
            .ok_or(Exception::throw_internal(
                &ctx,
                &format!("no index for key {named_key}"),
            ))?;

        bind_rusqlite_statement_index_to_rquickjs_value(stmt, idx, value, ctx.clone())?;
    }
    Ok(())
}

fn bind_parameters_from_rquickjs_array<'js>(
    stmt: &mut Statement<'_>,
    params: Array<'js>,
    ctx: Ctx<'js>,
) -> Result<()> {
    if params.len() > stmt.parameter_count() {
        return Err(Exception::throw_internal(&ctx, "too many parameters"));
    }

    let mut i = 1;
    for param in params.iter() {
        bind_rusqlite_statement_index_to_rquickjs_value(stmt, i, param?, ctx.clone())?;
        i += 1;
    }
    Ok(())
}

fn bind_rusqlite_statement_index_to_rquickjs_value<'js>(
    stmt: &mut Statement<'_>,
    index: usize,
    value: Value<'js>,
    ctx: Ctx<'js>,
) -> Result<()> {
    match value.type_of() {
        rquickjs::Type::Bool => stmt.raw_bind_parameter(index, value.as_bool().unwrap()),
        rquickjs::Type::Int => {
            if let Some(val) = value.as_big_int() {
                stmt.raw_bind_parameter(index, val.clone().to_i64()?)
            } else if let Some(val) = value.as_int() {
                stmt.raw_bind_parameter(index, val)
            } else {
                unimplemented!("missing case for number")
            }
        }
        rquickjs::Type::Float => stmt.raw_bind_parameter(index, value.as_float().unwrap()),
        rquickjs::Type::String => {
            stmt.raw_bind_parameter(index, value.as_string().unwrap().to_string()?)
        }
        rquickjs::Type::Null => stmt.raw_bind_parameter(index, rusqlite::types::Null),
        _ => stmt.raw_bind_parameter(index, value.get::<Coerced<String>>()?.0),
    }
    .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
    Ok(())
}

fn execute_stmt_and_collect_rows<'js>(
    stmt: &mut Statement<'_>,
    ctx: Ctx<'js>,
) -> Result<Option<Array<'js>>> {
    let row_collection = Array::new(ctx.clone())?;

    let column_count = stmt.column_count();
    let mut row_num = 0;

    let mut rows = stmt.raw_query();
    while let Ok(Some(row)) = rows.next() {
        let values = Array::new(ctx.clone())?;
        for i in 0..column_count {
            let ctx = ctx.clone();
            let this = row
                .get_ref(i)
                .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
            values.set(i, convert_rusqlite_to_rquickjs_value(this, ctx)?)?;
        }
        row_collection.set(row_num, values)?;
        row_num += 1;
    }
    Ok(if row_num == 0 {
        None
    } else {
        Some(row_collection)
    })
}

fn convert_rusqlite_to_rquickjs_value<'js>(
    this: rusqlite::types::ValueRef<'_>,
    ctx: Ctx<'js>,
) -> Result<Value<'js>> {
    match this.data_type() {
        rusqlite::types::Type::Null => Ok(Value::new_null(ctx)),
        rusqlite::types::Type::Integer => {
            let as_i64 = this
                .as_i64()
                .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
            if let Ok(Ok(as_i32)) = as_i64.try_into().map(|x: i32| x.into_js(&ctx)) {
                Ok(as_i32)
            } else {
                Ok(BigInt::from_i64(ctx, as_i64)?.into_value())
            }
        }
        rusqlite::types::Type::Real => {
            let as_f64 = this
                .as_f64()
                .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
            Ok(Value::new_float(ctx, as_f64))
        }
        rusqlite::types::Type::Text => {
            let as_str = this
                .as_str()
                .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
            as_str.into_js(&ctx)
        }
        rusqlite::types::Type::Blob => {
            let as_blob = this
                .as_blob()
                .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
            as_blob.into_js(&ctx)
        }
    }
}

#[derive(Error, Display, From, Debug)]
enum QueryRowError {
    Sql(rusqlite::Error),
    FromSql(rusqlite::types::FromSqlError),
    Rquickjs(rquickjs::Error),
}

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "PascalCase"
)]
pub mod sqlite {
    pub use super::Connection;
}
