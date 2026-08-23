//! ECMAScript `Temporal` built-ins, wrapping [`temporal_rs`].
//!
//! `evaluate` installs `globalThis.Temporal`. There is no `Temporal.Calendar`
//! constructor in the Stage 4 spec; calendars are identifier strings on the
//! date types (`calendarId`).

mod convert;
mod duration;
mod instant;
mod now;
mod plain_date;
mod plain_date_time;
mod plain_month_day;
mod plain_time;
mod plain_year_month;
mod shape;
mod zoned_date_time;

pub use duration::Duration;
pub use instant::Instant;
pub use now::Now;
pub use plain_date::PlainDate;
pub use plain_date_time::PlainDateTime;
pub use plain_month_day::PlainMonthDay;
pub use plain_time::PlainTime;
pub use plain_year_month::PlainYearMonth;
pub use zoned_date_time::ZonedDateTime;

#[rquickjs::module(rename_types = "PascalCase")]
pub mod temporal {
    use rquickjs::{Ctx, Exception, Function, Object, Result, class::JsClass, module::Exports};

    pub use crate::{
        Duration, Instant, PlainDate, PlainDateTime, PlainMonthDay, PlainTime, PlainYearMonth,
        ZonedDateTime,
    };

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        let namespace = Object::new(ctx.clone())?;
        let interfaces = [
            ("Instant", Instant::constructor(ctx)?),
            ("Duration", Duration::constructor(ctx)?),
            ("PlainDate", PlainDate::constructor(ctx)?),
            ("PlainTime", PlainTime::constructor(ctx)?),
            ("PlainDateTime", PlainDateTime::constructor(ctx)?),
            ("PlainYearMonth", PlainYearMonth::constructor(ctx)?),
            ("PlainMonthDay", PlainMonthDay::constructor(ctx)?),
            ("ZonedDateTime", ZonedDateTime::constructor(ctx)?),
        ];
        let interface_names = Vec::from_iter(interfaces.iter().map(|(name, _)| *name));
        for (name, constructor) in interfaces {
            let constructor = constructor.ok_or_else(|| {
                Exception::throw_internal(ctx, &format!("Temporal.{name} has no constructor"))
            })?;
            namespace.set(name, constructor)?;
        }

        let now = Object::new(ctx.clone())?;
        now.set("instant", crate::now::js_instant)?;
        now.set("timeZoneId", crate::now::js_time_zone_id)?;
        now.set("plainDateISO", crate::now::js_plain_date_iso)?;
        now.set("plainTimeISO", crate::now::js_plain_time_iso)?;
        now.set("plainDateTimeISO", crate::now::js_plain_date_time_iso)?;
        now.set("zonedDateTimeISO", crate::now::js_zoned_date_time_iso)?;

        ctx.eval::<Function, _>(crate::shape::DEFINE_INTERFACE_SHAPE)?
            .call::<_, ()>((namespace.clone(), now, interface_names, ctx.globals()))?;
        Ok(())
    }
}
