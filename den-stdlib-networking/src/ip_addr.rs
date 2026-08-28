use std::net::IpAddr;

use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{JsLifetime, class::Trace};

#[derive(Trace, JsLifetime, Clone, Debug, From, Into, Deref, DerefMut)]
#[rquickjs::class(rename = "IpAddr")]
pub struct IpAddrWrapper {
    #[qjs(skip_trace)]
    addr: IpAddr,
}

#[rquickjs::methods]
impl IpAddrWrapper {
    #[qjs(get, enumerable)]
    pub const fn is_unspecified(&self) -> bool { self.addr.is_unspecified() }

    #[qjs(get, enumerable)]
    pub const fn is_loopback(&self) -> bool { self.addr.is_loopback() }

    #[qjs(get, enumerable)]
    pub const fn is_multicast(&self) -> bool { self.addr.is_multicast() }

    #[qjs(get, enumerable)]
    pub const fn is_ipv4(&self) -> bool { self.addr.is_ipv4() }

    #[qjs(get, enumerable)]
    pub const fn is_ipv6(&self) -> bool { self.addr.is_ipv6() }

    #[qjs(rename = "toString")]
    pub fn as_string(&self) -> String { self.addr.to_string() }
}
