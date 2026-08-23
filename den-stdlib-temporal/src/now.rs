use rquickjs::{Ctx, Result, Value, prelude::Opt};

use crate::convert::{optional_time_zone, unwrap_temporal};
use crate::instant::Instant;
use crate::plain_date::PlainDate;
use crate::plain_date_time::PlainDateTime;
use crate::plain_time::PlainTime;
use crate::zoned_date_time::ZonedDateTime;

/// Host clock for `Temporal.Now`. Not a JS constructor — the spec object is a
/// bag of functions with `@@toStringTag` `"Temporal.Now"`.
pub struct Now;

impl Now {
    pub fn instant(ctx: &Ctx<'_>) -> Result<Instant> {
        unwrap_temporal(ctx, temporal_rs::Temporal::local_now().instant()).map(Instant::wrap)
    }

    pub fn time_zone_id(ctx: &Ctx<'_>) -> Result<String> {
        let zone = unwrap_temporal(ctx, temporal_rs::Temporal::local_now().time_zone())?;
        unwrap_temporal(ctx, zone.identifier())
    }

    pub fn plain_date_iso(
        ctx: &Ctx<'_>,
        time_zone: Option<temporal_rs::TimeZone>,
    ) -> Result<PlainDate> {
        unwrap_temporal(
            ctx,
            temporal_rs::Temporal::local_now().plain_date_iso(time_zone),
        )
        .map(PlainDate::wrap)
    }

    pub fn plain_time_iso(
        ctx: &Ctx<'_>,
        time_zone: Option<temporal_rs::TimeZone>,
    ) -> Result<PlainTime> {
        unwrap_temporal(
            ctx,
            temporal_rs::Temporal::local_now().plain_time_iso(time_zone),
        )
        .map(PlainTime::wrap)
    }

    pub fn plain_date_time_iso(
        ctx: &Ctx<'_>,
        time_zone: Option<temporal_rs::TimeZone>,
    ) -> Result<PlainDateTime> {
        unwrap_temporal(
            ctx,
            temporal_rs::Temporal::local_now().plain_date_time_iso(time_zone),
        )
        .map(PlainDateTime::wrap)
    }

    pub fn zoned_date_time_iso(
        ctx: &Ctx<'_>,
        time_zone: Option<temporal_rs::TimeZone>,
    ) -> Result<ZonedDateTime> {
        unwrap_temporal(
            ctx,
            temporal_rs::Temporal::local_now().zoned_date_time_iso(time_zone),
        )
        .map(ZonedDateTime::wrap)
    }
}

#[rquickjs::function]
pub fn instant(ctx: Ctx<'_>) -> Result<Instant> {
    Now::instant(&ctx)
}

#[rquickjs::function(rename = "timeZoneId")]
pub fn time_zone_id(ctx: Ctx<'_>) -> Result<String> {
    Now::time_zone_id(&ctx)
}

#[rquickjs::function(rename = "plainDateISO")]
pub fn plain_date_iso<'js>(time_zone: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<PlainDate> {
    Now::plain_date_iso(&ctx, optional_time_zone(&ctx, time_zone)?)
}

#[rquickjs::function(rename = "plainTimeISO")]
pub fn plain_time_iso<'js>(time_zone: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<PlainTime> {
    Now::plain_time_iso(&ctx, optional_time_zone(&ctx, time_zone)?)
}

#[rquickjs::function(rename = "plainDateTimeISO")]
pub fn plain_date_time_iso<'js>(
    time_zone: Opt<Value<'js>>,
    ctx: Ctx<'js>,
) -> Result<PlainDateTime> {
    Now::plain_date_time_iso(&ctx, optional_time_zone(&ctx, time_zone)?)
}

#[rquickjs::function(rename = "zonedDateTimeISO")]
pub fn zoned_date_time_iso<'js>(
    time_zone: Opt<Value<'js>>,
    ctx: Ctx<'js>,
) -> Result<ZonedDateTime> {
    Now::zoned_date_time_iso(&ctx, optional_time_zone(&ctx, time_zone)?)
}
