pub mod ip_addr;
pub mod socket;
pub mod socket_addr;

#[rquickjs::module]
pub mod networking {
    pub use crate::socket::{TcpListenerWrapper as TcpListener, TcpStreamWrapper as TcpStream};
}
