use std::net::SocketAddr;

use delegate_attr::delegate;
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::class::Trace;

use crate::ip_addr::IpAddrWrapper;

#[derive(Trace, Derivative, From, Into, Deref, DerefMut)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "SocketAddr")]
pub struct SocketAddrWrapper {
    #[qjs(skip_trace)]
    addr: SocketAddr,
}

#[rquickjs::methods]
impl SocketAddrWrapper {
    #[qjs(get, enumerable)]
    #[delegate(self.addr)]
    pub fn port(&self) -> u16 {}

    #[qjs(set, rename = "port")]
    #[delegate(self.addr)]
    pub fn set_port(mut self, new_port: u16) {}

    #[qjs(get, enumerable)]
    #[delegate(self.addr)]
    pub fn is_ipv4(&self) -> bool {}

    #[qjs(get, enumerable)]
    #[delegate(self.addr)]
    pub fn is_ipv6(&self) -> bool {}

    #[qjs(get, rename = "ip", enumerable)]
    #[delegate(self.addr)]
    #[into]
    pub fn ip(&self) -> IpAddrWrapper {}

    #[qjs(set, rename = "ip", enumerable)]
    pub fn set_ip(mut self, ip: IpAddrWrapper) {
        self.addr.set_ip(ip.into())
    }

    #[qjs(rename = "toString")]
    #[delegate(self.addr)]
    pub fn to_string(&self) -> String {}
}
