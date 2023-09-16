pub mod ip_addr;
pub mod socket;
pub mod socket_addr;

#[rquickjs::module]
pub mod networking {
    use std::sync::Arc;

    use den_stdlib_core::WORLD_END;
    use den_utils::FutureExt;
    use tokio::net::TcpListener;

    #[rquickjs::function]
    pub async fn listen(addr: String) -> rquickjs::Result<crate::socket::TcpListenerWrapper> {
        let listener = TcpListener::bind(addr)
            .with_cancellation(&WORLD_END.child_token())
            .await??;
        Ok(Arc::new(listener).into())
    }
}
