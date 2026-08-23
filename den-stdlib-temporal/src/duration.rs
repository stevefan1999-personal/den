use rquickjs::{Ctx, JsLifetime, Result, Value, atom::PredefinedAtom, class::Trace, prelude::Opt};
use temporal_rs::options::ToStringRoundingOptions;

use crate::convert::{
    bag_value, ctor_integer_if_integral, ctor_integer_if_integral_i128, options_bag, ordering_i32,
    throw_value_of, to_duration, to_relative_to, to_rounding_options, to_string_rounding_options,
    to_unit, unwrap_temporal,
};

#[derive(Trace, JsLifetime, Clone, Copy)]
#[rquickjs::class(rename = "Duration", frozen)]
pub struct Duration {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::Duration,
}

impl Duration {
    pub(crate) fn wrap(inner: temporal_rs::Duration) -> Self {
        Self { inner }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Duration {
    #[qjs(constructor)]
    #[allow(clippy::too_many_arguments)]
    pub fn new<'js>(
        years: Opt<Value<'js>>,
        months: Opt<Value<'js>>,
        weeks: Opt<Value<'js>>,
        days: Opt<Value<'js>>,
        hours: Opt<Value<'js>>,
        minutes: Opt<Value<'js>>,
        seconds: Opt<Value<'js>>,
        milliseconds: Opt<Value<'js>>,
        microseconds: Opt<Value<'js>>,
        nanoseconds: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let inner = unwrap_temporal(
            &ctx,
            temporal_rs::Duration::new(
                ctor_integer_if_integral(&ctx, years)?,
                ctor_integer_if_integral(&ctx, months)?,
                ctor_integer_if_integral(&ctx, weeks)?,
                ctor_integer_if_integral(&ctx, days)?,
                ctor_integer_if_integral(&ctx, hours)?,
                ctor_integer_if_integral(&ctx, minutes)?,
                ctor_integer_if_integral(&ctx, seconds)?,
                ctor_integer_if_integral(&ctx, milliseconds)?,
                ctor_integer_if_integral_i128(&ctx, microseconds)?,
                ctor_integer_if_integral_i128(&ctx, nanoseconds)?,
            ),
        )?;
        Ok(Self::wrap(inner))
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        to_duration(&ctx, &item).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(
        one: Value<'js>,
        two: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<i32> {
        let left = to_duration(&ctx, &one)?;
        let right = to_duration(&ctx, &two)?;
        let bag = options_bag(&ctx, options)?;
        let relative_to = match bag_value(&bag, "relativeTo") {
            None => None,
            Some(value) => Some(to_relative_to(&ctx, value)?),
        };
        unwrap_temporal(&ctx, left.compare(&right, relative_to)).map(ordering_i32)
    }

    #[qjs(get)]
    pub fn years(&self) -> i64 {
        self.inner.years()
    }

    #[qjs(get)]
    pub fn months(&self) -> i64 {
        self.inner.months()
    }

    #[qjs(get)]
    pub fn weeks(&self) -> i64 {
        self.inner.weeks()
    }

    #[qjs(get)]
    pub fn days(&self) -> i64 {
        self.inner.days()
    }

    #[qjs(get)]
    pub fn hours(&self) -> i64 {
        self.inner.hours()
    }

    #[qjs(get)]
    pub fn minutes(&self) -> i64 {
        self.inner.minutes()
    }

    #[qjs(get)]
    pub fn seconds(&self) -> i64 {
        self.inner.seconds()
    }

    #[qjs(get)]
    pub fn milliseconds(&self) -> i64 {
        self.inner.milliseconds()
    }

    #[qjs(get)]
    pub fn microseconds(&self) -> f64 {
        self.inner.microseconds() as f64
    }

    #[qjs(get)]
    pub fn nanoseconds(&self) -> f64 {
        self.inner.nanoseconds() as f64
    }

    #[qjs(get)]
    pub fn sign(&self) -> i8 {
        self.inner.sign() as i8
    }

    #[qjs(get)]
    pub fn blank(&self) -> bool {
        self.inner.is_zero()
    }

    pub fn negated(&self) -> Self {
        Self::wrap(self.inner.negated())
    }

    pub fn abs(&self) -> Self {
        Self::wrap(self.inner.abs())
    }

    pub fn add<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let other = to_duration(&ctx, &other)?;
        unwrap_temporal(&ctx, self.inner.add(&other)).map(Self::wrap)
    }

    pub fn subtract<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let other = to_duration(&ctx, &other)?;
        unwrap_temporal(&ctx, self.inner.subtract(&other)).map(Self::wrap)
    }

    pub fn round<'js>(&self, options: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let bag = if options.is_string() {
            let mut map = indexmap::IndexMap::new();
            map.insert("smallestUnit".to_string(), options);
            map
        } else {
            options_bag(&ctx, Opt(Some(options)))?
        };
        let rounding = to_rounding_options(&ctx, &bag)?;
        let relative_to = match bag_value(&bag, "relativeTo") {
            None => None,
            Some(value) => Some(to_relative_to(&ctx, value)?),
        };
        unwrap_temporal(&ctx, self.inner.round(rounding, relative_to)).map(Self::wrap)
    }

    pub fn total<'js>(&self, options: Value<'js>, ctx: Ctx<'js>) -> Result<f64> {
        let (unit, relative_to) = if options.is_string() {
            (to_unit(&ctx, &options)?, None)
        } else {
            let bag = options_bag(&ctx, Opt(Some(options)))?;
            let unit = match bag_value(&bag, "unit") {
                Some(value) => to_unit(&ctx, value)?,
                None => {
                    return Err(rquickjs::Exception::throw_range(&ctx, "unit is required"));
                }
            };
            let relative_to = match bag_value(&bag, "relativeTo") {
                None => None,
                Some(value) => Some(to_relative_to(&ctx, value)?),
            };
            (unit, relative_to)
        };
        unwrap_temporal(&ctx, self.inner.total(unit, relative_to)).map(|total| total.as_inner())
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let rounding = to_string_rounding_options(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.as_temporal_string(rounding))
    }

    pub fn to_json(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(
            &ctx,
            self.inner
                .as_temporal_string(ToStringRoundingOptions::default()),
        )
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.Duration"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Temporal.Duration"
    }
}
