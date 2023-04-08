use rquickjs::bind;
pub use socket_addr::*;

#[bind(object, public)]
#[quickjs(bare)]
mod socket_addr {
    use std::{cell::Cell, net::SocketAddr};

    use delegate_attr::delegate;
    use derivative::Derivative;
    use derive_more::{Deref, DerefMut, From, Into};

    use crate::IpAddrWrapper;

    #[quickjs(cloneable, rename = "SocketAddr")]
    #[derive(Derivative, From, Into, Deref, DerefMut)]
    #[derivative(Clone, Debug)]
    pub struct SocketAddrWrapper(Cell<SocketAddr>);

    #[quickjs(rename = "SocketAddr")]
    impl SocketAddrWrapper {
        pub fn new(ip: IpAddrWrapper, port: u16) -> Self {
            let addr = SocketAddr::new(ip.into(), port);
            Self(Cell::new(addr))
        }

        #[quickjs(get, enumerable)]
        #[delegate(self.get())]
        pub fn port(&self) -> u16;

        #[quickjs(set, rename = "port")]
        #[delegate(self.get_mut())]
        pub fn set_port(mut self, new_port: u16);

        #[quickjs(get, enumerable)]
        #[delegate(self.get())]
        pub fn is_ipv4(&self) -> bool;

        #[quickjs(get, enumerable)]
        #[delegate(self.get())]
        pub fn is_ipv6(&self) -> bool;

        #[quickjs(get, enumerable, rename = "ip")]
        #[delegate(self.get())]
        #[into]
        pub fn ip(&self) -> IpAddrWrapper;

        #[quickjs(rename = "ip", set, enumerable)]
        pub fn set_ip(mut self, ip: IpAddrWrapper) {
            self.get_mut().set_ip(ip.into())
        }

        #[quickjs(rename = "toString")]
        #[delegate(self.get())]
        pub fn to_string(&self) -> String;
    }
}
