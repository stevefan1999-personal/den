//! Canvas phase 0 for den: `ImageData`, `ImageBitmap` and `createImageBitmap`.
//!
//! Everything here is CPU pixel work on straight (non-premultiplied) RGBA8, so
//! the crate needs no image codec and no GPU. That is also why
//! `createImageBitmap` accepts only the two sources that need no decoder,
//! `ImageData` and `ImageBitmap`; `Blob` waits for a codec, and
//! `OffscreenCanvas` waits for a rendering context to put in it.

pub mod bitmap;

use rquickjs::{
    Coerced, Ctx, Exception, FromJs as _, JsLifetime, Object, Result, TypedArray, U8Clamped, Value,
    atom::PredefinedAtom, class::Trace, prelude::*,
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
    fn enumerated<T: Copy>(
        ctx: &Ctx<'_>, options: &Object<'_>, key: &str, choices: &[(&str, T)], default: T,
    ) -> Result<T> {
        let Some(Coerced(name)) = options.get::<_, Option<Coerced<String>>>(key)? else {
            return Ok(default);
        };
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
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorSpace {
    #[default]
    Srgb,
    DisplayP3,
}

impl ColorSpace {
    const CHOICES: [(&'static str, Self); 2] =
        [("srgb", Self::Srgb), ("display-p3", Self::DisplayP3)];
    /// Display P3 to sRGB in linear light. Both spaces share the sRGB transfer
    /// function, so only the primaries differ and one matrix covers it.
    const DISPLAY_P3_TO_SRGB: [[f64; 3]; 3] = [
        [1.224_940_176_280_575_6, -0.224_940_176_280_575_6, 0.0],
        [-0.042_056_954_789_627_8, 1.042_056_954_789_628, 0.0],
        [
            -0.019_637_554_781_998_8,
            -0.078_636_077_217_415_4,
            1.098_273_631_999_414_4,
        ],
    ];
    const TRANSFER_EXPONENT: f64 = 2.4;
    const TRANSFER_OFFSET: f64 = 0.055;
    const TRANSFER_SLOPE: f64 = 12.92;
    const TRANSFER_THRESHOLD: f64 = 0.040_45;

    const fn name(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::DisplayP3 => "display-p3",
        }
    }

    fn to_linear(value: f64) -> f64 {
        if value <= Self::TRANSFER_THRESHOLD {
            value / Self::TRANSFER_SLOPE
        } else {
            ((value + Self::TRANSFER_OFFSET) / (1.0 + Self::TRANSFER_OFFSET))
                .powf(Self::TRANSFER_EXPONENT)
        }
    }

    fn to_encoded(value: f64) -> f64 {
        if value <= Self::TRANSFER_THRESHOLD / Self::TRANSFER_SLOPE {
            value * Self::TRANSFER_SLOPE
        } else {
            (1.0 + Self::TRANSFER_OFFSET).mul_add(
                value.powf(Self::TRANSFER_EXPONENT.recip()),
                -Self::TRANSFER_OFFSET,
            )
        }
    }

    /// Convert one encoded RGB triple from `self` into encoded sRGB.
    fn encode_as_srgb(self, rgb: [u8; 3]) -> [u8; 3] {
        if self == Self::Srgb {
            return rgb;
        }
        let full = f64::from(u8::MAX);
        let linear = rgb.map(|channel| Self::to_linear(f64::from(channel) / full));
        Self::DISPLAY_P3_TO_SRGB.map(|row| {
            let mixed: f64 = row
                .iter()
                .zip(linear)
                .map(|(weight, channel)| weight * channel)
                .sum();
            (Self::to_encoded(mixed.clamp(0.0, 1.0)) * full).round() as u8
        })
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
    #[qjs(get, enumerable)]
    data:        TypedArray<'js, U8Clamped>,
    #[qjs(get, enumerable)]
    width:       u32,
    #[qjs(get, enumerable)]
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

        let length = data.len();
        if length == 0 {
            return Err(Exception::throw_type(
                &ctx,
                "ImageData source data has zero elements",
            ));
        }
        if !length.is_multiple_of(CHANNELS) {
            return Err(Exception::throw_type(
                &ctx,
                "ImageData source data length is not a multiple of 4",
            ));
        }
        if width == 0 {
            return Err(Exception::throw_range(
                &ctx,
                "ImageData source width is zero",
            ));
        }
        if height == Some(0) {
            return Err(Exception::throw_range(
                &ctx,
                "ImageData source height is zero",
            ));
        }
        let pixels = length.div_euclid(CHANNELS);
        if !pixels.is_multiple_of(width as usize) {
            return Err(Exception::throw_range(
                &ctx,
                "ImageData source data length is not a multiple of (4 * width)",
            ));
        }
        let derived = pixels.div_euclid(width as usize) as u32;
        if height.is_some_and(|given| given != derived) {
            return Err(Exception::throw_range(
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

    #[qjs(get, enumerable)]
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

    fn allocated(ctx: &Ctx<'js>, width: u32, height: u32, color_space: ColorSpace) -> Result<Self> {
        if width == 0 {
            return Err(Exception::throw_range(
                ctx,
                "ImageData source width is zero",
            ));
        }
        if height == 0 {
            return Err(Exception::throw_range(
                ctx,
                "ImageData source height is zero",
            ));
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
        self.data.as_bytes().map(|bytes| {
            bitmap::ImageBitmap::straight(bytes.to_vec(), self.width, self.height, self.color_space)
        })
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
