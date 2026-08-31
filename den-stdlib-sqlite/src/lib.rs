use std::{cell::RefCell, ops::Deref as _, rc::Rc};

use den_util::BufferSource;
use either::Either;
use rquickjs::{
    Array, BigInt, Ctx, Exception, FromJs as _, JsLifetime, Object, Result, TypedArray, Value,
    class::Trace, prelude::*,
};
use rusqlite::Statement;

#[derive(Trace, JsLifetime, Debug, Clone)]
#[rquickjs::class(rename = "Connection")]
pub struct Connection {
    #[qjs(skip_trace)]
    conn: Rc<RefCell<Option<rusqlite::Connection>>>,
}

#[rquickjs::methods]
impl Connection {
    // rquickjs only attaches `#[qjs(static)]` members to a class that
    // declares a constructor, and a `()` return makes `new Connection()`
    // throw: instances only come from `open`/`openInMemory`.
    #[expect(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub const fn new() {}

    #[qjs(static)]
    pub fn open_in_memory(ctx: Ctx<'_>) -> Result<Connection> {
        let conn = rusqlite::Connection::open_in_memory()
            .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
        Ok(Connection {
            conn: Rc::new(RefCell::new(Some(conn))),
        })
    }

    #[qjs(static)]
    pub fn open(path: String, ctx: Ctx<'_>) -> Result<Connection> {
        let conn = rusqlite::Connection::open(path)
            .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
        Ok(Connection {
            conn: Rc::new(RefCell::new(Some(conn))),
        })
    }

    pub fn execute<'js>(
        self, sql: String, Opt(params): Opt<Either<Array<'js>, Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<usize> {
        if let Some(conn) = self.conn.borrow().deref() {
            let mut stmt = prepare_and_bind(conn, &sql, params, &ctx)?;

            Ok(stmt
                .raw_execute()
                .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?)
        } else {
            Err(Exception::throw_internal(&ctx, "already closed"))
        }
    }

    pub fn query_rows<'js>(
        self, sql: String, Opt(params): Opt<Either<Array<'js>, Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Option<Array<'js>>> {
        if let Some(conn) = self.conn.borrow().deref() {
            let mut stmt = prepare_and_bind(conn, &sql, params, &ctx)?;
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

/// `execute` and `query_rows` share their first half: prepare `sql` on
/// `conn`, then bind `params` — array as positional, object as named — onto
/// the statement.
fn prepare_and_bind<'conn, 'js>(
    conn: &'conn rusqlite::Connection, sql: &str, params: Option<Either<Array<'js>, Object<'js>>>,
    ctx: &Ctx<'js>,
) -> Result<Statement<'conn>> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| Exception::throw_internal(ctx, &format!("{e}")))?;
    match params {
        Some(Either::Left(params)) => {
            bind_parameters_from_rquickjs_array(&mut stmt, params, ctx.clone())?;
        }
        Some(Either::Right(params)) => {
            bind_parameters_from_rquickjs_object(&mut stmt, params, ctx.clone())?;
        }
        None => {}
    }
    Ok(stmt)
}

fn bind_parameters_from_rquickjs_object<'js>(
    stmt: &mut Statement<'_>, params: Object<'js>, ctx: Ctx<'js>,
) -> Result<()> {
    if params.len() > stmt.parameter_count() {
        return Err(Exception::throw_internal(&ctx, "too many parameters"));
    }

    for param in params {
        let (key, value) = param?;
        let named_key = format!(":{}", key.to_string()?);
        let idx = stmt
            .parameter_index(&named_key)
            .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?
            .ok_or_else(|| {
                Exception::throw_internal(&ctx, &format!("no index for key {named_key}"))
            })?;

        bind_rusqlite_statement_index_to_rquickjs_value(stmt, idx, value, ctx.clone())?;
    }
    Ok(())
}

fn bind_parameters_from_rquickjs_array<'js>(
    stmt: &mut Statement<'_>, params: Array<'js>, ctx: Ctx<'js>,
) -> Result<()> {
    if params.len() > stmt.parameter_count() {
        return Err(Exception::throw_internal(&ctx, "too many parameters"));
    }

    // sqlite positional parameters are 1-based
    for (index, param) in (1..).zip(params.iter()) {
        bind_rusqlite_statement_index_to_rquickjs_value(stmt, index, param?, ctx.clone())?;
    }
    Ok(())
}

fn bind_rusqlite_statement_index_to_rquickjs_value<'js>(
    stmt: &mut Statement<'_>, index: usize, value: Value<'js>, ctx: Ctx<'js>,
) -> Result<()> {
    let bind = match value.type_of() {
        rquickjs::Type::Bool => {
            value
                .as_bool()
                .map(|value| stmt.raw_bind_parameter(index, value))
        }
        rquickjs::Type::Int => {
            value
                .as_int()
                .map(|value| stmt.raw_bind_parameter(index, value))
        }
        rquickjs::Type::BigInt => {
            value
                .as_big_int()
                .map(|value| value.clone().to_i64())
                .transpose()?
                .map(|value| stmt.raw_bind_parameter(index, value))
        }
        rquickjs::Type::Float => {
            value
                .as_float()
                .map(|value| stmt.raw_bind_parameter(index, value))
        }
        rquickjs::Type::String => {
            value
                .as_string()
                .map(rquickjs::String::to_string)
                .transpose()?
                .map(|value| stmt.raw_bind_parameter(index, value))
        }
        rquickjs::Type::Null => Some(stmt.raw_bind_parameter(index, rusqlite::types::Null)),
        rquickjs::Type::Object => {
            let bytes = BufferSource::from_js(&ctx, value)?.into_bytes();
            Some(stmt.raw_bind_parameter(index, bytes))
        }
        _ => {
            return Err(Exception::throw_type(
                &ctx,
                "SQLite parameters must be boolean, number, bigint, string, null, or BufferSource",
            ));
        }
    };
    bind.ok_or_else(|| Exception::throw_type(&ctx, "invalid SQLite parameter value"))?
        .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
    Ok(())
}

fn execute_stmt_and_collect_rows<'js>(
    stmt: &mut Statement<'_>, ctx: Ctx<'js>,
) -> Result<Option<Array<'js>>> {
    let row_collection = Array::new(ctx.clone())?;

    let column_count = stmt.column_count();
    let mut row_num = 0;

    let mut rows = stmt.raw_query();
    while let Some(row) = rows
        .next()
        .map_err(|error| Exception::throw_internal(&ctx, &error.to_string()))?
    {
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
    this: rusqlite::types::ValueRef<'_>, ctx: Ctx<'js>,
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
            TypedArray::<u8>::new_copy(ctx, as_blob).map(TypedArray::into_value)
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
