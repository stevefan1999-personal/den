//! Navigator / NavigatorUAData. Host strings come from OS, arch, crate
//! version and `uname`; the JS-visible shape matches txiki.js with the brand
//! set to `den`.

use std::num::NonZero;

use indexmap::indexmap;
use rquickjs::{
    Array, Class, Ctx, Exception, IntoJs, JsLifetime, Object, Result, Value, atom::PredefinedAtom,
    class::Trace, object::Property,
};

use crate::events::freeze;

/// Snapshot of the process, taken when the navigator object is built.
#[derive(Clone)]
pub struct HostInfo {
    os: String,
    arch: String,
    version: String,
    hardware_concurrency: u32,
    bitness: String,
    kernel_release: String,
}

unsafe impl<'js> JsLifetime<'js> for HostInfo {
    type Changed<'to> = HostInfo;
}

impl HostInfo {
    fn capture() -> Self {
        let (machine, release) = Self::uname();
        Self {
            os: Self::os().to_owned(),
            arch: if machine.is_empty() {
                Self::arch().to_owned()
            } else {
                machine
            },
            version: Self::version().to_owned(),
            hardware_concurrency: Self::hardware_concurrency(),
            bitness: Self::bitness().to_owned(),
            kernel_release: release,
        }
    }

    fn os() -> &'static str {
        std::env::consts::OS
    }

    fn arch() -> &'static str {
        std::env::consts::ARCH
    }

    fn version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn hardware_concurrency() -> u32 {
        std::thread::available_parallelism()
            .map(NonZero::get)
            .unwrap_or(1) as u32
    }

    fn bitness() -> &'static str {
        match std::mem::size_of::<usize>() {
            8 => "64",
            4 => "32",
            _ => "",
        }
    }

    fn uname() -> (String, String) {
        #[cfg(unix)]
        {
            Self::unix_uname()
        }
        #[cfg(windows)]
        {
            (Self::arch().to_owned(), "10.0.0".to_owned())
        }
        #[cfg(not(any(unix, windows)))]
        {
            (Self::arch().to_owned(), "0.0.0".to_owned())
        }
    }

    #[cfg(unix)]
    fn unix_uname() -> (String, String) {
        // SAFETY: `utsname` is POD of NUL-terminated char arrays. `uname`
        // fills it on success; a failed call leaves the zeroed buffers, which
        // we then discard in favour of the `std::env` fallback.
        let mut name = unsafe { std::mem::zeroed::<libc::utsname>() };
        if unsafe { libc::uname(&mut name) } != 0 {
            return (Self::arch().to_owned(), "0.0.0".to_owned());
        }
        (Self::c_chars(&name.machine), Self::c_chars(&name.release))
    }

    #[cfg(unix)]
    fn c_chars(buf: &[libc::c_char]) -> String {
        let bytes: Vec<u8> = buf
            .iter()
            .map(|&c| c as u8)
            .take_while(|&b| b != 0)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn ua_platform(&self) -> String {
        match self.os.as_str() {
            "macos" => "macOS".to_owned(),
            "windows" => "Windows".to_owned(),
            "linux" => "Linux".to_owned(),
            "freebsd" => "FreeBSD".to_owned(),
            "openbsd" => "OpenBSD".to_owned(),
            other => other.to_owned(),
        }
    }

    fn navigator_platform(&self) -> String {
        let machine = self.arch.as_str();
        match self.os.as_str() {
            "macos" => "MacIntel".to_owned(),
            "windows" => "Win32".to_owned(),
            "linux" => match machine {
                "x86" | "i686" | "i386" => "Linux i686".to_owned(),
                "x86_64" => "Linux x86_64".to_owned(),
                _ => format!("Linux {machine}"),
            },
            "freebsd" => match machine {
                "i386" => "FreeBSD i386".to_owned(),
                "amd64" | "x86_64" => "FreeBSD amd64".to_owned(),
                _ => format!("FreeBSD {machine}"),
            },
            "openbsd" => match machine {
                "i386" => "OpenBSD i386".to_owned(),
                "amd64" | "x86_64" => "OpenBSD amd64".to_owned(),
                _ => format!("OpenBSD {machine}"),
            },
            other => format!("{other} {machine}"),
        }
    }

    fn architecture(&self) -> &str {
        match self.arch.as_str() {
            "x86_64" | "amd64" | "x86" | "i686" | "i386" => "x86",
            "arm64" | "aarch64" | "arm" => "arm",
            other => other,
        }
    }

    fn platform_version(&self) -> String {
        let mut parts = self.kernel_release.split('.');
        let major = parts.next().unwrap_or("0");
        let minor = parts.next().unwrap_or("0");
        let patch = parts.next().unwrap_or("0");
        format!("{major}.{minor}.{patch}")
    }

    fn major_version(&self) -> &str {
        self.version.split('.').next().unwrap_or(&self.version)
    }
}

fn brand_entry<'js>(ctx: &Ctx<'js>, brand: &str, version: &str) -> Result<Object<'js>> {
    let entry = Object::new(ctx.clone())?;
    entry.set("brand", brand)?;
    entry.set("version", version)?;
    freeze(ctx, entry.as_value())?;
    Ok(entry)
}

fn frozen_brands<'js>(ctx: &Ctx<'js>, brand: &str, version: &str) -> Result<Array<'js>> {
    let brands = Array::new(ctx.clone())?;
    brands.set(0, brand_entry(ctx, brand, version)?)?;
    freeze(ctx, brands.as_value())?;
    Ok(brands)
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct NavigatorUAData<'js> {
    brands: Array<'js>,
    mobile: bool,
    platform: String,
    #[qjs(skip_trace)]
    host: HostInfo,
}

impl<'js> NavigatorUAData<'js> {
    fn from_host(ctx: &Ctx<'js>, host: HostInfo) -> Result<Self> {
        let brands = frozen_brands(ctx, "den", host.major_version())?;
        let platform = host.ua_platform();
        Ok(Self {
            brands,
            mobile: false,
            platform,
            host,
        })
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> NavigatorUAData<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>) -> Result<Self> {
        Self::from_host(&ctx, HostInfo::capture())
    }

    #[qjs(get)]
    pub fn brands(&self) -> Array<'js> {
        self.brands.clone()
    }

    #[qjs(get)]
    pub fn mobile(&self) -> bool {
        self.mobile
    }

    #[qjs(get)]
    pub fn platform(&self) -> String {
        self.platform.clone()
    }

    pub fn get_high_entropy_values(
        &self,
        ctx: Ctx<'js>,
        hints: Value<'js>,
    ) -> Result<rquickjs::Promise<'js>> {
        let (promise, resolve, reject) = ctx.promise()?;
        if hints.as_array().is_none() {
            let _ = Exception::throw_type(&ctx, "hints must be an array");
            let _ = reject.call::<_, ()>((ctx.catch(),));
            return Ok(promise);
        }
        let mut result = indexmap! {
            "brands" => self.brands.clone().into_js(&ctx)?,
            "mobile" => self.mobile.into_js(&ctx)?,
            "platform" => self.platform.clone().into_js(&ctx)?,
        };
        let hints = hints.as_array().expect("checked");
        for index in 0..hints.len() {
            let hint: String = match hints.get(index) {
                Ok(hint) => hint,
                Err(_) => continue,
            };
            match hint.as_str() {
                "architecture" => {
                    result.insert("architecture", self.host.architecture().into_js(&ctx)?);
                }
                "bitness" => {
                    result.insert("bitness", self.host.bitness.clone().into_js(&ctx)?);
                }
                "fullVersionList" => {
                    result.insert(
                        "fullVersionList",
                        frozen_brands(&ctx, "den", &self.host.version)?.into_js(&ctx)?,
                    );
                }
                "model" => {
                    result.insert("model", "".into_js(&ctx)?);
                }
                "platformVersion" => {
                    result.insert(
                        "platformVersion",
                        self.host.platform_version().into_js(&ctx)?,
                    );
                }
                "wow64" => {
                    result.insert("wow64", false.into_js(&ctx)?);
                }
                "formFactors" => {
                    let factors = Array::new(ctx.clone())?;
                    factors.set(0, "Desktop")?;
                    freeze(&ctx, factors.as_value())?;
                    result.insert("formFactors", factors.into_js(&ctx)?);
                }
                _ => {}
            }
        }
        let _ = resolve.call::<_, ()>((result.into_js(&ctx)?,));
        Ok(promise)
    }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        indexmap! {
            "brands" => self.brands.clone().into_js(&ctx)?,
            "mobile" => self.mobile.into_js(&ctx)?,
            "platform" => self.platform.clone().into_js(&ctx)?,
        }
        .into_js(&ctx)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "NavigatorUAData"
    }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct Navigator<'js> {
    #[qjs(skip_trace)]
    host: HostInfo,
    user_agent_data: Class<'js, NavigatorUAData<'js>>,
}

impl<'js> Navigator<'js> {
    fn from_host(ctx: &Ctx<'js>, host: HostInfo) -> Result<Self> {
        let user_agent_data =
            Class::instance(ctx.clone(), NavigatorUAData::from_host(ctx, host.clone())?)?;
        Ok(Self {
            host,
            user_agent_data,
        })
    }

    pub fn instance(ctx: &Ctx<'js>) -> Result<Class<'js, Self>> {
        Class::instance(ctx.clone(), Self::from_host(ctx, HostInfo::capture())?)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> Navigator<'js> {
    #[qjs(get)]
    pub fn user_agent(&self) -> String {
        format!("den/{}", self.host.version)
    }

    #[qjs(get)]
    pub fn hardware_concurrency(&self) -> u32 {
        self.host.hardware_concurrency
    }

    #[qjs(get)]
    pub fn platform(&self) -> String {
        self.host.navigator_platform()
    }

    #[qjs(get)]
    pub fn user_agent_data(&self) -> Class<'js, NavigatorUAData<'js>> {
        self.user_agent_data.clone()
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Navigator"
    }
}

/// Install `NavigatorUAData` is the module's job; this places the `navigator`
/// instance on `target` as a non-writable, non-configurable data property.
pub fn install_navigator<'js>(ctx: &Ctx<'js>, target: &Object<'js>) -> Result<()> {
    let navigator = Navigator::instance(ctx)?;
    target.prop("navigator", Property::from(navigator).enumerable())
}
