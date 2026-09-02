//! Canvas phase 0 for den: `ImageData`, `ImageBitmap` and `createImageBitmap`.
//!
//! Everything here is CPU pixel work on straight (non-premultiplied) RGBA8, so
//! the crate needs no image codec and no GPU. That is also why
//! `createImageBitmap` accepts only the two sources that need no decoder,
//! `ImageData` and `ImageBitmap`; `Blob` waits for a codec, and
//! `OffscreenCanvas` waits for a rendering context to put in it.

pub mod bitmap;

use rquickjs::{
    Coerced, Ctx, Error, Exception, FromJs as _, JsLifetime, Object, Result, TypedArray, U8Clamped,
    Value, atom::PredefinedAtom, class::Trace, prelude::*,
};

pub use crate::js_canvas_module as js_canvas;

/// Four bytes per pixel: the only layout phase 0 stores.
const CHANNELS: usize = 4;

/// The WebIDL conversions the canvas specs are written against. QuickJS hands
/// over raw `Value`s and rquickjs has no WebIDL layer, so overload resolution
/// has to do its own coercion.
struct WebIdl;

impl WebIdl {
    /// `unsigned long`: ToNumber, truncate toward zero, then modulo 2^32.
    fn unsigned_long<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<u32> {
        const MODULUS: f64 = u32::MAX as f64 + 1.0;
        let number = Coerced::<f64>::from_js(ctx, value)?.0;
        Ok(if number.is_finite() {
            number.trunc().rem_euclid(MODULUS) as u32
        } else {
            0
        })
    }

    /// `[EnforceRange] unsigned long`: where the plain conversion wraps, this
    /// one refuses. NaN and both infinities fail the same bounds test that an
    /// out-of-range magnitude does.
    fn enforced_unsigned_long<'js>(ctx: &Ctx<'js>, value: Value<'js>, what: &str) -> Result<u32> {
        let number = Coerced::<f64>::from_js(ctx, value)?.0.trunc();
        if !(0.0..=f64::from(u32::MAX)).contains(&number) {
            return Err(Exception::throw_type(
                ctx,
                &format!("{what} is out of range for an unsigned long"),
            ));
        }
        Ok(number as u32)
    }

    /// `long`: the same conversion reinterpreted as two's complement.
    fn long<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<i32> {
        Self::unsigned_long(ctx, value).map(|value| value as i32)
    }

    /// A dictionary argument: `undefined` and `null` mean "all defaults",
    /// anything else that is not an object is a TypeError.
    fn dictionary<'js>(ctx: &Ctx<'js>, value: Value<'js>, what: &str) -> Result<Object<'js>> {
        if value.is_undefined() || value.is_null() {
            return Object::new(ctx.clone());
        }
        value
            .into_object()
            .ok_or_else(|| Exception::throw_type(ctx, &format!("{what} is not an object")))
    }

    /// An enumeration member: absent takes the default, an unlisted string is
    /// a TypeError.
    ///
    /// Only `undefined` counts as absent. `null` is a present member, and
    /// WebIDL runs ToString over it first, so it arrives as `"null"` and is
    /// rejected like any other unlisted value.
    fn enumerated<'js, T: Copy>(
        ctx: &Ctx<'js>, options: &Object<'js>, key: &str, choices: &[(&str, T)], default: T,
    ) -> Result<T> {
        let value = options.get::<_, Value<'js>>(key)?;
        if value.is_undefined() {
            return Ok(default);
        }
        let name = den_util::coerce_string(ctx, value)?;
        choices
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map_or_else(
                || {
                    Err(Exception::throw_type(
                        ctx,
                        &format!("'{name}' is not a valid value for {key}"),
                    ))
                },
                |(_, value)| Ok(*value),
            )
    }
}

/// The `PredefinedColorSpace` members den can represent.
#[derive(Clone, Copy)]
pub enum ColorSpace {
    Srgb,
    DisplayP3,
}

impl ColorSpace {
    const CHOICES: [(&'static str, Self); 2] =
        [("srgb", Self::Srgb), ("display-p3", Self::DisplayP3)];

    const fn name(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::DisplayP3 => "display-p3",
        }
    }
}

/// `ImageData`: a `Uint8ClampedArray` of straight RGBA8 plus its dimensions.
///
/// The array is held as a JS handle rather than as Rust bytes so that
/// `imageData.data` keeps returning the very same object, which is what makes
/// writes through it visible to the next read.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct ImageData<'js> {
    #[qjs(get, enumerable, configurable)]
    data:        TypedArray<'js, U8Clamped>,
    #[qjs(get, enumerable, configurable)]
    width:       u32,
    #[qjs(get, enumerable, configurable)]
    height:      u32,
    #[qjs(skip_trace)]
    color_space: ColorSpace,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> ImageData<'js> {
    /// Real WebIDL overload resolution between
    /// `(sw, sh, settings?)` and `(data, sw, sh?, settings?)`: the
    /// distinguishing argument is the first one, and only an actual
    /// `Uint8ClampedArray` selects the data overload. Deno keys off the
    /// argument *count* instead, which lets a plain `Uint8Array` through.
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<Self> {
        if args.0.len() < 2 {
            return Err(Exception::throw_type(
                &ctx,
                "ImageData requires at least 2 arguments",
            ));
        }
        let arg = |index: usize| {
            args.0
                .get(index)
                .cloned()
                .unwrap_or_else(|| Value::new_undefined(ctx.clone()))
        };
        let takes_data = arg(0)
            .as_object()
            .is_some_and(Object::is_typed_array::<U8Clamped>);
        if !takes_data {
            return Self::allocated(
                &ctx,
                WebIdl::unsigned_long(&ctx, arg(0))?,
                WebIdl::unsigned_long(&ctx, arg(1))?,
                Self::settings_color_space(&ctx, arg(2))?,
            );
        }

        let data = TypedArray::<U8Clamped>::from_value(arg(0))?;
        let width = WebIdl::unsigned_long(&ctx, arg(1))?;
        let height = match arg(2) {
            given if given.is_undefined() => None,
            given => Some(WebIdl::unsigned_long(&ctx, given)?),
        };
        let color_space = Self::settings_color_space(&ctx, arg(3))?;

        // The view's own byte count, not `data.length`: that property is a
        // configurable accessor on %TypedArray%.prototype, so script can
        // replace it, and rquickjs asserts the result is an integer — a
        // non-integer return panics the host. One byte per element here, so
        // the byte length *is* the element count, and a detached buffer
        // reports none and falls into the zero-elements check below.
        let length = data.as_bytes().map_or(0, <[u8]>::len);
        // HTML reports each of these as a DOMException rather than as a plain
        // ECMAScript error: a bad buffer is an "InvalidStateError", a bad
        // dimension an "IndexSizeError".
        let buffer_fault =
            |message: &str| den_util::throw_dom_exception(&ctx, "InvalidStateError", message);
        if length == 0 {
            return Err(buffer_fault("ImageData source data has zero elements"));
        }
        if !length.is_multiple_of(CHANNELS) {
            return Err(buffer_fault(
                "ImageData source data length is not a multiple of 4",
            ));
        }
        if width == 0 {
            return Err(Self::bad_dimension(&ctx, "ImageData source width is zero"));
        }
        if height == Some(0) {
            return Err(Self::bad_dimension(&ctx, "ImageData source height is zero"));
        }
        let pixels = length.div_euclid(CHANNELS);
        if !pixels.is_multiple_of(width as usize) {
            return Err(Self::bad_dimension(
                &ctx,
                "ImageData source data length is not a multiple of (4 * width)",
            ));
        }
        let derived = pixels.div_euclid(width as usize) as u32;
        if height.is_some_and(|given| given != derived) {
            return Err(Self::bad_dimension(
                &ctx,
                "ImageData source data length is not equal to (4 * width * height)",
            ));
        }
        Ok(Self {
            data,
            width,
            height: derived,
            color_space,
        })
    }

    #[qjs(get, enumerable, configurable)]
    pub const fn color_space(&self) -> &'static str { self.color_space.name() }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "ImageData" }
}

impl<'js> ImageData<'js> {
    fn settings_color_space(ctx: &Ctx<'js>, value: Value<'js>) -> Result<ColorSpace> {
        let settings = WebIdl::dictionary(ctx, value, "ImageData settings")?;
        WebIdl::enumerated(
            ctx,
            &settings,
            "colorSpace",
            &ColorSpace::CHOICES,
            ColorSpace::Srgb,
        )
    }

    /// An "IndexSizeError" DOMException: how HTML reports every dimension
    /// fault in either `ImageData` overload.
    fn bad_dimension(ctx: &Ctx<'_>, message: &str) -> Error {
        den_util::throw_dom_exception(ctx, "IndexSizeError", message)
    }

    fn allocated(ctx: &Ctx<'js>, width: u32, height: u32, color_space: ColorSpace) -> Result<Self> {
        if width == 0 {
            return Err(Self::bad_dimension(ctx, "ImageData source width is zero"));
        }
        if height == 0 {
            return Err(Self::bad_dimension(ctx, "ImageData source height is zero"));
        }
        let length = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(CHANNELS))
            .ok_or_else(|| Exception::throw_range(ctx, "ImageData is too large"))?;
        // QuickJS owns the zero-filled allocation: it honours the runtime
        // memory limit and reports exhaustion as a JS error, where a Rust-side
        // `Vec` of the same size would abort the process. The `TypedArray`
        // conversion re-brands the result, so a replaced global cannot smuggle
        // some other array class in as `imageData.data`.
        let data = den_util::construct(ctx, "Uint8ClampedArray", (length,))?;
        Ok(Self {
            data,
            width,
            height,
            color_space,
        })
    }

    /// Straight RGBA8 copy of the pixels, or `None` once the backing buffer
    /// has been detached.
    fn snapshot(&self) -> Option<bitmap::ImageBitmap> {
        self.data
            .as_bytes()
            .map(|bytes| bitmap::ImageBitmap::straight(bytes.to_vec(), self.width, self.height))
    }
}

#[rquickjs::module]
pub mod canvas_module {
    use den_util::ConstructorInstaller as _;
    use rquickjs::{
        Ctx, Result, Value,
        module::{Declarations, Exports},
    };

    use super::{ImageData, bitmap::ImageBitmap};

    const EXPORTS: [&str; 3] = ["ImageData", "ImageBitmap", "createImageBitmap"];

    #[qjs(declare)]
    pub fn declare(declarations: &Declarations) -> Result<()> {
        for name in EXPORTS {
            declarations.declare(name)?;
        }
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let globals = ctx.globals();
        globals.install_constructor::<ImageData>(2)?;
        globals.install_constructor::<ImageBitmap>(0)?;
        ImageBitmap::install_bitmap_data(ctx)?;
        globals.set("createImageBitmap", ImageBitmap::create_image_bitmap(ctx)?)?;
        // Re-export what was installed rather than minting fresh constructors:
        // `den:canvas` and the globals have to name the same objects, or
        // `instanceof` disagrees with itself across the two.
        for name in EXPORTS {
            exports.export(name, globals.get::<_, Value<'js>>(name)?)?;
        }
        Ok(())
    }
}
