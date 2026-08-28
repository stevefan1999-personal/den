use rquickjs::{Ctx, Result, Value, prelude::Opt};

use crate::{
    convert::{optional_time_zone, unwrap_temporal},
    instant::Instant,
    plain_date::PlainDate,
    plain_date_time::PlainDateTime,
    plain_time::PlainTime,
    zoned_date_time::ZonedDateTime,
};

#[rquickjs::function]
pub fn instant(ctx: Ctx<'_>) -> Result<Instant> {
    unwrap_temporal(&ctx, temporal_rs::Temporal::local_now().instant()).map(Instant::wrap)
}

#[rquickjs::function(rename = "timeZoneId")]
pub fn time_zone_id(ctx: Ctx<'_>) -> Result<String> {
    let zone = unwrap_temporal(&ctx, temporal_rs::Temporal::local_now().time_zone())?;
    unwrap_temporal(&ctx, zone.identifier())
}

#[rquickjs::function(rename = "plainDateISO")]
pub fn plain_date_iso<'js>(time_zone: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<PlainDate> {
    let time_zone = optional_time_zone(&ctx, time_zone)?;
    unwrap_temporal(
        &ctx,
        temporal_rs::Temporal::local_now().plain_date_iso(time_zone),
    )
    .map(PlainDate::wrap)
}

#[rquickjs::function(rename = "plainTimeISO")]
pub fn plain_time_iso<'js>(time_zone: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<PlainTime> {
    let time_zone = optional_time_zone(&ctx, time_zone)?;
    unwrap_temporal(
        &ctx,
        temporal_rs::Temporal::local_now().plain_time_iso(time_zone),
    )
    .map(PlainTime::wrap)
}

#[rquickjs::function(rename = "plainDateTimeISO")]
pub fn plain_date_time_iso<'js>(
    time_zone: Opt<Value<'js>>, ctx: Ctx<'js>,
) -> Result<PlainDateTime> {
    let time_zone = optional_time_zone(&ctx, time_zone)?;
    unwrap_temporal(
        &ctx,
        temporal_rs::Temporal::local_now().plain_date_time_iso(time_zone),
    )
    .map(PlainDateTime::wrap)
}

#[rquickjs::function(rename = "zonedDateTimeISO")]
pub fn zoned_date_time_iso<'js>(
    time_zone: Opt<Value<'js>>, ctx: Ctx<'js>,
) -> Result<ZonedDateTime> {
    let time_zone = optional_time_zone(&ctx, time_zone)?;
    unwrap_temporal(
        &ctx,
        temporal_rs::Temporal::local_now().zoned_date_time_iso(time_zone),
    )
    .map(ZonedDateTime::wrap)
}
