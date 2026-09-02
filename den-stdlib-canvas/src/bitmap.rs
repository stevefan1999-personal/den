//! `ImageBitmap` and the `createImageBitmap` factory.

use den_util::Probe as _;
use rquickjs::{
    Class, Ctx, Error, Exception, Function, JsLifetime, Object, Result, TypedArray, U8Clamped,
    Value,
    atom::PredefinedAtom,
    class::Trace,
    prelude::{Rest, This},
};

use crate::{CHANNELS, ColorSpace, ImageData, WebIdl};

/// `ImageBitmap`: an immutable straight-or-premultiplied RGBA8 rectangle.
///
/// The alpha state is tracked rather than guessed from the pixels — Deno
/// inspects the samples and can be wrong — so `premultiplyAlpha` is exact when
/// a bitmap is itself the source of another bitmap.
#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class]
pub struct ImageBitmap {
    #[qjs(skip_trace)]
    pixels:        Vec<u8>,
    #[qjs(get, enumerable)]
    width:         u32,
    #[qjs(get, enumerable)]
    height:        u32,
    #[qjs(skip_trace)]
    color_space:   ColorSpace,
    #[qjs(skip_trace)]
    premultiplied: bool,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl ImageBitmap {
    /// The interface has no constructor; the global exists only so that
    /// `instanceof` and feature detection work.
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    /// Detach the bitmap. The spec then reports zero for both dimensions, so
    /// dropping the pixels is all it takes.
    pub fn close(&mut self) { *self = Self::straight(Vec::new(), 0, 0, ColorSpace::Srgb); }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "ImageBitmap" }
}

impl ImageBitmap {
    /// Pixel read-back, hung off a registered symbol rather than a name:
    /// `ImageBitmap` has no spec-visible pixels until there is a rendering
    /// context to draw it into, yet the whole crop/resize/alpha pipeline is
    /// only observable through them. Deno keeps the same escape hatch under
    /// `Symbol.for("Deno_bitmapData")`.
    pub const BITMAP_DATA: &'static str = "den.bitmapData";

    pub(crate) const fn straight(
        pixels: Vec<u8>, width: u32, height: u32, color_space: ColorSpace,
    ) -> Self {
        Self {
            pixels,
            width,
            height,
            color_space,
            premultiplied: false,
        }
    }

    pub fn install_bitmap_data<'js>(ctx: &Ctx<'js>) -> Result<()> {
        let Some(prototype) = Class::<'js, Self>::prototype(ctx)? else {
            return Ok(());
        };
        let key: Value<'js> = ctx
            .globals()
            .get::<_, Object<'js>>("Symbol")?
            .get::<_, Function<'js>>("for")?
            .call((Self::BITMAP_DATA,))?;
        let read = Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, this: This<Class<'js, Self>>| {
                let pixels: Vec<U8Clamped> = this
                    .0
                    .try_borrow()?
                    .pixels
                    .iter()
                    .copied()
                    .map(U8Clamped)
                    .collect();
                TypedArray::new_copy(ctx, pixels)
            },
        )?;
        prototype.set(key, read)
    }

    /// The `createImageBitmap` factory. WebIDL turns every failure of a
    /// promise-returning operation into a rejection, argument conversion
    /// included, so the whole build runs inside a `Result` and any pending
    /// exception is moved onto the promise here.
    pub fn create_image_bitmap<'js>(ctx: &Ctx<'js>) -> Result<Function<'js>> {
        let factory = Function::new(ctx.clone(), |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
            let (promise, resolve, reject) = ctx.promise()?;
            match Self::build(&ctx, &args.0) {
                Ok(bitmap) => resolve.call::<_, ()>((bitmap,))?,
                Err(Error::Exception) => reject.call::<_, ()>((ctx.catch(),))?,
                Err(error) => return Err(error),
            }
            Ok(promise)
        })?;
        factory.set_name("createImageBitmap")?;
        factory.set_length(1)?;
        Ok(factory)
    }

    fn build<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> Result<Class<'js, Self>> {
        // Overload resolution by arity: the crop form carries four
        // coordinates, so three or four arguments match neither overload.
        let cropping = match args.len() {
            1 | 2 => false,
            5 | 6 => true,
            _ => {
                return Err(Exception::throw_type(
                    ctx,
                    "createImageBitmap takes 1, 2, 5 or 6 arguments",
                ));
            }
        };
        let arg = |index: usize| {
            args.get(index)
                .cloned()
                .unwrap_or_else(|| Value::new_undefined(ctx.clone()))
        };
        // Argument conversion first, left to right, then the algorithm's own
        // checks in order: empty source rectangle, empty resize target, and
        // only then whether the source is a type phase 0 can read.
        let rectangle = cropping
            .then(|| {
                Ok::<_, Error>([
                    i64::from(WebIdl::long(ctx, arg(1))?),
                    i64::from(WebIdl::long(ctx, arg(2))?),
                    i64::from(WebIdl::long(ctx, arg(3))?),
                    i64::from(WebIdl::long(ctx, arg(4))?),
                ])
            })
            .transpose()?;
        let options = BitmapOptions::parse(ctx, arg(if cropping { 5 } else { 1 }))?;
        if let Some([_, _, across, down]) = rectangle
            && (across == 0 || down == 0)
        {
            return Err(Exception::throw_range(
                ctx,
                "createImageBitmap source rectangle is empty",
            ));
        }
        options.reject_empty_resize(ctx)?;
        let source = Self::source(ctx, arg(0))?;
        // A negative extent means the rectangle grows the other way.
        let (left, top, width, height) = rectangle.map_or(
            (0, 0, source.width, source.height),
            |[left, top, across, down]| {
                (
                    left.min(left + across),
                    top.min(top + down),
                    across.unsigned_abs() as u32,
                    down.unsigned_abs() as u32,
                )
            },
        );
        let (output_width, output_height) = options.output_size(width, height);
        Class::instance(
            ctx.clone(),
            source
                .cropped(ctx, left, top, width, height)?
                .resized(ctx, output_width, output_height, options.quality)?
                .flipped(options.flip_y)
                .converted(options.convert_to_srgb)
                .with_alpha(options.premultiply_alpha),
        )
    }

    /// `ImageBitmapSource` narrowed to what phase 0 can read without a codec.
    fn source<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let detached = || {
            den_util::throw_dom_exception(
                ctx,
                "InvalidStateError",
                "the createImageBitmap source is detached",
            )
        };
        if let Some(data) = ctx.probe(|| Class::<'js, ImageData<'js>>::from_value(&value).ok()) {
            return data.try_borrow()?.snapshot().ok_or_else(detached);
        }
        if let Some(bitmap) = ctx.probe(|| Class::<'js, Self>::from_value(&value).ok()) {
            let bitmap = bitmap.try_borrow()?;
            return if bitmap.width == 0 || bitmap.height == 0 {
                Err(detached())
            } else {
                Ok(bitmap.clone())
            };
        }
        Err(Exception::throw_type(
            ctx,
            "createImageBitmap source must be an ImageData or an ImageBitmap",
        ))
    }

    /// Transparent-black surface, refusing sizes the host cannot back rather
    /// than aborting the process inside `Vec`'s allocator.
    fn surface(ctx: &Ctx<'_>, width: u32, height: u32) -> Result<Vec<u8>> {
        let too_large = || Exception::throw_range(ctx, "the requested image is too large");
        let length = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(CHANNELS))
            .ok_or_else(too_large)?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(length)
            .map_err(|_error| too_large())?;
        pixels.resize(length, 0);
        Ok(pixels)
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; CHANNELS] {
        let index = y as usize * self.width as usize + x as usize;
        self.pixels
            .as_chunks::<CHANNELS>()
            .0
            .get(index)
            .copied()
            .unwrap_or_default()
    }

    /// The spec crops against an infinite transparent-black surface, so rows
    /// and columns outside the source stay zeroed instead of clamping.
    fn cropped(&self, ctx: &Ctx<'_>, left: i64, top: i64, width: u32, height: u32) -> Result<Self> {
        if left == 0 && top == 0 && width == self.width && height == self.height {
            return Ok(self.clone());
        }
        let mut pixels = Self::surface(ctx, width, height)?;
        // The horizontally overlapping span is the same on every row, so each
        // row is one memcpy rather than a per-pixel bounds test.
        let first = (-left).clamp(0, i64::from(width));
        let last = (i64::from(self.width) - left).clamp(first, i64::from(width));
        let span = (last - first) as usize * CHANNELS;
        for row in 0..i64::from(height) {
            let source_row = top + row;
            if span == 0 || !(0..i64::from(self.height)).contains(&source_row) {
                continue;
            }
            let from = (source_row * i64::from(self.width) + left + first) as usize * CHANNELS;
            let into = (row * i64::from(width) + first) as usize * CHANNELS;
            if let (Some(source), Some(target)) = (
                self.pixels.get(from..from + span),
                pixels.get_mut(into..into + span),
            ) {
                target.copy_from_slice(source);
            }
        }
        Ok(Self {
            pixels,
            width,
            height,
            ..self.clone()
        })
    }

    fn resized(
        self, ctx: &Ctx<'_>, width: u32, height: u32, quality: ResizeQuality,
    ) -> Result<Self> {
        if width == self.width && height == self.height {
            return Ok(self);
        }
        let mut pixels = Self::surface(ctx, width, height)?;
        let scale_x = f64::from(self.width) / f64::from(width);
        let scale_y = f64::from(self.height) / f64::from(height);
        for (index, target) in pixels.as_chunks_mut::<CHANNELS>().0.iter_mut().enumerate() {
            // Sample at the centre of the output pixel mapped back into the
            // source, which is what keeps an integral rescale symmetric.
            let x = (index.rem_euclid(width as usize) as f64 + 0.5) * scale_x;
            let y = (index.div_euclid(width as usize) as f64 + 0.5) * scale_y;
            *target = match quality {
                ResizeQuality::Pixelated => {
                    self.pixel(
                        (x.floor().max(0.0) as u32).min(self.width.saturating_sub(1)),
                        (y.floor().max(0.0) as u32).min(self.height.saturating_sub(1)),
                    )
                }
                ResizeQuality::Smooth => self.sampled(x - 0.5, y - 0.5),
            };
        }
        Ok(Self {
            pixels,
            width,
            height,
            ..self
        })
    }

    /// Bilinear sample in straight RGBA.
    ///
    /// ponytail: no windowed filter, so a large downscale aliases; swap in
    /// Lanczos if `resizeQuality` ever has to mean more than "not pixelated".
    fn sampled(&self, x: f64, y: f64) -> [u8; CHANNELS] {
        let full = f64::from(u8::MAX);
        let (last_x, last_y) = (self.width.saturating_sub(1), self.height.saturating_sub(1));
        let left = (x.floor().max(0.0) as u32).min(last_x);
        let top = (y.floor().max(0.0) as u32).min(last_y);
        let (right, bottom) = ((left + 1).min(last_x), (top + 1).min(last_y));
        let (across, down) = (
            (x - x.floor()).clamp(0.0, 1.0),
            (y - y.floor()).clamp(0.0, 1.0),
        );
        let mix = |low: f64, high: f64, at: f64| low.mul_add(1.0 - at, high * at);

        let (top_left, top_right) = (self.pixel(left, top), self.pixel(right, top));
        let (low_left, low_right) = (self.pixel(left, bottom), self.pixel(right, bottom));
        let mut sample = [0; CHANNELS];
        for (channel, ((first, second), (third, fourth))) in sample.iter_mut().zip(
            top_left
                .iter()
                .zip(&top_right)
                .zip(low_left.iter().zip(&low_right)),
        ) {
            let upper = mix(f64::from(*first), f64::from(*second), across);
            let lower = mix(f64::from(*third), f64::from(*fourth), across);
            *channel = mix(upper, lower, down).round().clamp(0.0, full) as u8;
        }
        sample
    }

    fn flipped(self, flip_y: bool) -> Self {
        if !flip_y || self.width == 0 || self.height == 0 {
            return self;
        }
        let stride = self.width as usize * CHANNELS;
        let pixels = self
            .pixels
            .chunks_exact(stride)
            .rev()
            .flatten()
            .copied()
            .collect();
        Self { pixels, ..self }
    }

    /// `colorSpaceConversion: "default"` means "give me sRGB". With no codec
    /// there is no embedded ICC profile to honour, but an `ImageData` can
    /// still declare display-p3, and that is one real matrix away from sRGB.
    fn converted(mut self, to_srgb: bool) -> Self {
        if !to_srgb || self.color_space == ColorSpace::Srgb {
            return self;
        }
        for [red, green, blue, _] in self.pixels.as_chunks_mut::<CHANNELS>().0 {
            [*red, *green, *blue] = self.color_space.encode_as_srgb([*red, *green, *blue]);
        }
        self.color_space = ColorSpace::Srgb;
        self
    }

    fn with_alpha(mut self, request: PremultiplyAlpha) -> Self {
        let premultiplied = match request {
            PremultiplyAlpha::Default => self.premultiplied,
            PremultiplyAlpha::Premultiply => true,
            PremultiplyAlpha::None => false,
        };
        if premultiplied == self.premultiplied {
            return self;
        }
        let full = f64::from(u8::MAX);
        for [red, green, blue, alpha] in self.pixels.as_chunks_mut::<CHANNELS>().0 {
            if *alpha == 0 {
                continue;
            }
            let scale = if premultiplied {
                f64::from(*alpha) / full
            } else {
                full / f64::from(*alpha)
            };
            for channel in [red, green, blue] {
                *channel = (f64::from(*channel) * scale).round().clamp(0.0, full) as u8;
            }
        }
        self.premultiplied = premultiplied;
        self
    }
}

/// `resizeQuality` is a hint the spec lets a user agent collapse; den keeps
/// the one distinction that is visible in the output pixels.
#[derive(Clone, Copy)]
enum ResizeQuality {
    Pixelated,
    Smooth,
}

#[derive(Clone, Copy)]
enum PremultiplyAlpha {
    Default,
    Premultiply,
    None,
}

struct BitmapOptions {
    flip_y:            bool,
    premultiply_alpha: PremultiplyAlpha,
    convert_to_srgb:   bool,
    resize_width:      Option<u32>,
    resize_height:     Option<u32>,
    quality:           ResizeQuality,
}

impl BitmapOptions {
    fn parse<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let options = WebIdl::dictionary(ctx, value, "createImageBitmap options")?;
        Ok(Self {
            // "none" is the HTML spec's current spelling of "from-image";
            // Deno still rejects it.
            flip_y:            WebIdl::enumerated(
                ctx,
                &options,
                "imageOrientation",
                &[("from-image", false), ("none", false), ("flipY", true)],
                false,
            )?,
            premultiply_alpha: WebIdl::enumerated(
                ctx,
                &options,
                "premultiplyAlpha",
                &[
                    ("default", PremultiplyAlpha::Default),
                    ("premultiply", PremultiplyAlpha::Premultiply),
                    ("none", PremultiplyAlpha::None),
                ],
                PremultiplyAlpha::Default,
            )?,
            convert_to_srgb:   WebIdl::enumerated(
                ctx,
                &options,
                "colorSpaceConversion",
                &[("default", true), ("none", false)],
                true,
            )?,
            resize_width:      Self::size(ctx, &options, "resizeWidth")?,
            resize_height:     Self::size(ctx, &options, "resizeHeight")?,
            quality:           WebIdl::enumerated(
                ctx,
                &options,
                "resizeQuality",
                &[
                    ("pixelated", ResizeQuality::Pixelated),
                    ("low", ResizeQuality::Smooth),
                    ("medium", ResizeQuality::Smooth),
                    ("high", ResizeQuality::Smooth),
                ],
                ResizeQuality::Smooth,
            )?,
        })
    }

    fn size<'js>(ctx: &Ctx<'js>, options: &Object<'js>, key: &str) -> Result<Option<u32>> {
        options
            .get::<_, Option<Value<'js>>>(key)?
            .filter(|given| !given.is_undefined())
            .map(|given| WebIdl::unsigned_long(ctx, given))
            .transpose()
    }

    fn reject_empty_resize(&self, ctx: &Ctx<'_>) -> Result<()> {
        if self.resize_width == Some(0) || self.resize_height == Some(0) {
            return Err(den_util::throw_dom_exception(
                ctx,
                "InvalidStateError",
                "createImageBitmap resize target is empty",
            ));
        }
        Ok(())
    }

    /// One given dimension fixes the other by aspect ratio; neither keeps the
    /// cropped size.
    fn output_size(&self, width: u32, height: u32) -> (u32, u32) {
        let scaled = |extent: u32, from: u32, to: u32| {
            (u64::from(extent) * u64::from(to)).div_ceil(u64::from(from)) as u32
        };
        match (self.resize_width, self.resize_height) {
            (Some(across), Some(down)) => (across, down),
            (Some(across), None) => (across, scaled(height, width, across)),
            (None, Some(down)) => (scaled(width, height, down), down),
            (None, None) => (width, height),
        }
    }
}
