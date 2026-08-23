//! Host identity backing `src/prelude/navigator.js`: OS, arch, crate version,
//! `hardwareConcurrency` and kernel release (`uname`).

use std::num::NonZero;

use rquickjs::{Ctx, Object, Result};

/// Snapshot of the process, taken when natives install.
pub struct HostInfo;

impl HostInfo {
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

    /// `natives.host` for the navigator prelude.
    pub fn install<'js>(ctx: &Ctx<'js>, natives: &Object<'js>) -> Result<()> {
        let (machine, release) = Self::uname();
        let host = Object::new(ctx.clone())?;
        host.set("os", Self::os())?;
        host.set(
            "arch",
            if machine.is_empty() {
                Self::arch().to_owned()
            } else {
                machine
            },
        )?;
        host.set("version", Self::version())?;
        host.set("hardwareConcurrency", Self::hardware_concurrency())?;
        host.set("bitness", Self::bitness())?;
        host.set("kernelRelease", release)?;
        natives.set("host", host)?;
        Ok(())
    }
}
