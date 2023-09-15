use std::{cell::Cell, net::SocketAddr};

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
    pub fn new(ip: IpAddrWrapper, port: u16) -> Self {
        let addr = SocketAddr::new(ip.into(), port);
        Self { addr }
    }

    #[qjs(get, enumerable)]
    #[delegate(self.get())]
    pub fn port(&self) -> u16;

    #[qjs(set, rename = "port")]
    #[delegate(self.get_mut())]
    pub fn set_port(mut self, new_port: u16);

    #[qjs(get, enumerable)]
    #[delegate(self.get())]
    pub fn is_ipv4(&self) -> bool;

    #[qjs(get, enumerable)]
    #[delegate(self.get())]
    pub fn is_ipv6(&self) -> bool;

    #[qjs(get, enumerable, rename = "ip")]
    #[delegate(self.get())]
    #[into]
    pub fn ip(&self) -> IpAddrWrapper;

    #[qjs(rename = "toString")]
    #[delegate(self.get())]
    pub fn to_string(&self) -> String;
}
