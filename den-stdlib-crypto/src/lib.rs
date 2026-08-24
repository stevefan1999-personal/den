use den_util::BufferSource;
use rquickjs::{ArrayBuffer, Ctx, Exception, Object, Result, TypedArray, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};
use uuid::Uuid;

pub fn get_random_values<'js>(array: Object<'js>, ctx: Ctx<'js>) -> Result<Object<'js>> {
    {
        let array = if let Ok(array) = TypedArray::<u8>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<u16>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<u32>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<u64>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<i8>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<i16>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<i32>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<i64>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Some(array) = ArrayBuffer::from_object(array.clone()) {
            Ok(array)
        } else {
            Err(Exception::throw_type(&ctx, "not a typed array"))
        }?;

        // `as_raw` is the only mutable view rquickjs 0.12 offers: `as_bytes` hands back
        // a shared `&[u8]`, so writing through it would mean casting away its
        // immutability. It returns `None` for a detached buffer, which JS can
        // trigger at will.
        let Some(raw) = array.as_raw() else {
            return Err(Exception::throw_type(&ctx, "array buffer is detached"));
        };
        // SAFETY: `raw` is QuickJS's own live allocation for this buffer. Nothing else
        // aliases it here — no JS runs between `as_raw` and the end of the
        // fill, so the buffer cannot be detached or resized underneath us.
        let dest = unsafe { core::slice::from_raw_parts_mut(raw.ptr.as_ptr(), raw.len) };
        rand::fill(dest);
    }
    Ok(array)
}

pub fn random_uuid() -> String { Uuid::new_v4().to_string() }

/// The four hash algorithms Web Crypto requires of `SubtleCrypto.digest`.
///
/// SHA-1 lives in its own crate: `sha2` does not implement it, and the Web
/// Crypto spec still lists it even though it is broken for collision
/// resistance.
#[derive(Clone, Copy)]
enum DigestAlgorithm {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl DigestAlgorithm {
    fn parse(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("SHA-1") {
            Some(Self::Sha1)
        } else if name.eq_ignore_ascii_case("SHA-256") {
            Some(Self::Sha256)
        } else if name.eq_ignore_ascii_case("SHA-384") {
            Some(Self::Sha384)
        } else if name.eq_ignore_ascii_case("SHA-512") {
            Some(Self::Sha512)
        } else {
            None
        }
    }

    /// AlgorithmIdentifier is a name string or a dictionary with a `name`
    /// field. Anything else, including a missing or unrecognised name, is a
    /// `NotSupportedError` — the spec's "normalize an algorithm" failure,
    /// reported as a rejected promise rather than a sync throw because this
    /// runs inside the async `digest` body.
    fn from_algorithm(ctx: &Ctx<'_>, algorithm: Value<'_>) -> Result<Self> {
        let raw_name = Self::raw_name(algorithm)?;
        let name = raw_name.as_deref().unwrap_or("undefined");
        match raw_name.as_deref().and_then(Self::parse) {
            Some(algorithm) => Ok(algorithm),
            None => {
                Err(den_util::throw_dom_exception(
                    ctx,
                    "NotSupportedError",
                    &format!("Unrecognized algorithm name: {name}"),
                ))
            }
        }
    }

    fn raw_name(algorithm: Value<'_>) -> Result<Option<String>> {
        if let Some(name) = algorithm.as_string() {
            return Ok(Some(name.to_string()?));
        }
        let Some(object) = algorithm.as_object() else {
            return Ok(None);
        };
        let name: Value<'_> = object.get("name")?;
        if name.is_undefined() || name.is_null() {
            return Ok(None);
        }
        match name.as_string() {
            Some(name) => Ok(Some(name.to_string()?)),
            None => Ok(None),
        }
    }

    fn hash(self, data: &[u8]) -> Vec<u8> {
        match self {
            Self::Sha1 => Sha1::digest(data).to_vec(),
            Self::Sha256 => Sha256::digest(data).to_vec(),
            Self::Sha384 => Sha384::digest(data).to_vec(),
            Self::Sha512 => Sha512::digest(data).to_vec(),
        }
    }
}

/// `SubtleCrypto.digest` — async so the JS return is a `Promise<ArrayBuffer>`,
/// even though the hash itself is computed inline.
#[rquickjs::function]
pub async fn digest<'js>(
    algorithm: Value<'js>, data: BufferSource, ctx: Ctx<'js>,
) -> Result<ArrayBuffer<'js>> {
    let algorithm = DigestAlgorithm::from_algorithm(&ctx, algorithm)?;
    // `new_copy`, never `new`: `new` lends QuickJS the Rust allocation plus a
    // free hook it runs twice on detach, so `(await digest(...)).transfer()`
    // would abort the process.
    ArrayBuffer::new_copy(ctx, algorithm.hash(data.bytes()))
}

#[rquickjs::module]
pub mod crypto {
    use indexmap::indexmap;
    use rquickjs::{Ctx, IntoJs, Object, Result, module::Exports};

    #[rquickjs::function]
    #[qjs(rename = "getRandomValues")]
    pub fn get_random_values<'js>(array: Object<'js>, ctx: Ctx<'js>) -> Result<Object<'js>> {
        crate::get_random_values(array, ctx)
    }

    #[rquickjs::function]
    #[qjs(rename = "randomUUID")]
    pub fn random_uuid() -> String { crate::random_uuid() }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        let subtle = indexmap! {
            "digest" => super::js_digest.into_js(ctx)?,
        };
        ctx.globals().set("crypto", indexmap! {
            "getRandomValues" => js_get_random_values.into_js(ctx)?,
            "randomUUID" => js_random_uuid.into_js(ctx)?,
            "subtle" => subtle.into_js(ctx)?,
        })?;
        Ok(())
    }
}
