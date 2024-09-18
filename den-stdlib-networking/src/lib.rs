pub mod ip_addr;
pub mod socket;
pub mod socket_addr;

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "PascalCase"
)]
pub mod networking {
    pub use crate::socket::{TcpListenerWrapper as TcpListener, TcpStreamWrapper as TcpStream};
}
