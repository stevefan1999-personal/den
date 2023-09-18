pub mod ip_addr;
pub mod socket;
pub mod socket_addr;

#[rquickjs::module]
pub mod networking {
    use std::sync::Arc;

    use den_stdlib_core::WorldsEndExt;
    use den_utils::FutureExt;
    use rquickjs::Ctx;
    use tokio::net::TcpListener;

    #[rquickjs::function]
    pub async fn listen(
        addr: String,
        ctx: Ctx<'_>,
    ) -> rquickjs::Result<crate::socket::TcpListenerWrapper> {
        let listener = TcpListener::bind(addr)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        Ok(Arc::new(listener).into())
    }
}
