use std::net::IpAddr;

use delegate_attr::delegate;
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{class::Trace, JsLifetime};

#[derive(Trace, JsLifetime, Derivative, From, Into, Deref, DerefMut)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "IpAddr")]
pub struct IpAddrWrapper {
    #[qjs(skip_trace)]
    addr: IpAddr,
}

#[rquickjs::methods]
impl IpAddrWrapper {
    #[qjs(get, enumerable)]
    #[delegate(self.addr)]
    pub fn is_unspecified(&self) -> bool {}

    #[qjs(get, enumerable)]
    #[delegate(self.addr)]
    pub fn is_loopback(&self) -> bool {}

    #[qjs(get, enumerable)]
    #[delegate(self.addr)]
    pub fn is_multicast(&self) -> bool {}

    #[qjs(get, enumerable)]
    #[delegate(self.addr)]
    pub fn is_ipv4(&self) -> bool {}

    #[qjs(get, enumerable)]
    #[delegate(self.addr)]
    pub fn is_ipv6(&self) -> bool {}

    #[qjs(rename = "toString")]
    #[delegate(self.addr)]
    pub fn to_string(&self) -> String {}
}
