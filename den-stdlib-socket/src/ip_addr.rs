pub use ip_addr::*;
use rquickjs::bind;

#[bind(object, public)]
#[quickjs(bare)]
mod ip_addr {
    use std::{net::IpAddr, ops::Deref};

    use delegate_attr::delegate;
    use derivative::Derivative;
    use derive_more::{Deref, DerefMut, From, Into};

    #[quickjs(cloneable, rename = "IpAddr")]
    #[derive(Derivative, From, Into, Deref, DerefMut)]
    #[derivative(Clone, Debug)]
    pub struct IpAddrWrapper(IpAddr);

    #[quickjs(rename = "IpAddr")]
    impl IpAddrWrapper {
        #[quickjs(get, enumerable)]
        #[delegate(self.deref())]
        pub fn is_unspecified(&self) -> bool;

        #[quickjs(get, enumerable)]
        #[delegate(self.deref())]
        pub fn is_loopback(&self) -> bool;

        #[quickjs(get, enumerable)]
        #[delegate(self.deref())]
        pub fn is_multicast(&self) -> bool;

        #[quickjs(get, enumerable)]
        #[delegate(self.deref())]
        pub fn is_ipv4(&self) -> bool;

        #[quickjs(get, enumerable)]
        #[delegate(self.deref())]
        pub fn is_ipv6(&self) -> bool;

        #[quickjs(rename = "toString")]
        #[delegate(self.deref())]
        pub fn to_string(&self) -> String;
    }
}
