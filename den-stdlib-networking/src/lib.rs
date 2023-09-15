pub mod ip_addr;
pub mod socket;
pub mod socket_addr;

#[rquickjs::module]
pub mod socket {
    pub use crate::{
        ip_addr::IpAddrWrapper,
        socket::{TcpListenerWrapper, TcpStreamWrapper},
        socket_addr::SocketAddrWrapper,
    };
}
