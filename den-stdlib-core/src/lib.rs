use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

pub static WORLD_END: OnceCell<CancellationToken> = OnceCell::const_new();
