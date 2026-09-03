use std::net::SocketAddr;

use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{JsLifetime, class::Trace};

use crate::ip_addr::IpAddrWrapper;

#[derive(Trace, JsLifetime, Clone, Debug, From, Into, Deref, DerefMut)]
#[rquickjs::class(rename = "SocketAddr")]
pub struct SocketAddrWrapper {
    #[qjs(skip_trace)]
    addr: SocketAddr,
}

#[rquickjs::methods]
impl SocketAddrWrapper {
    #[qjs(get, enumerable)]
    pub const fn port(&self) -> u16 { self.addr.port() }

    #[qjs(get, enumerable)]
    pub const fn is_ipv4(&self) -> bool { self.addr.is_ipv4() }

    #[qjs(get, enumerable)]
    pub const fn is_ipv6(&self) -> bool { self.addr.is_ipv6() }

    #[qjs(get, rename = "ip", enumerable)]
    pub fn ip(&self) -> IpAddrWrapper { self.addr.ip().into() }

    #[qjs(rename = "toString")]
    pub fn as_string(&self) -> String { self.addr.to_string() }
}
