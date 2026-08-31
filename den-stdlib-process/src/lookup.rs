//! DNS lookup via `tokio::net::lookup_host`.

use rquickjs::{Ctx, Exception, Object, Result, Value};
use tokio::net::lookup_host;

/// A hostname lookup, returning `{ family, ip }` or an array of those.
pub async fn host<'js>(
    ctx: Ctx<'js>, host: String, options: Option<Object<'js>>,
) -> Result<Value<'js>> {
    if host.is_empty() {
        return Err(Exception::throw_type(
            &ctx,
            "host must be a non-empty string",
        ));
    }

    let all = options
        .as_ref()
        .and_then(|options| options.get::<_, Option<bool>>("all").ok())
        .flatten()
        .unwrap_or(false);
    let family = options
        .as_ref()
        .and_then(|options| options.get::<_, Option<i32>>("family").ok())
        .flatten()
        .unwrap_or(0);

    let addrs = lookup_host((host.as_str(), 0))
        .await
        .map_err(|error| Exception::throw_internal(&ctx, &error.to_string()))?
        .filter(|addr| {
            match family {
                4 => addr.is_ipv4(),
                6 => addr.is_ipv6(),
                _ => true,
            }
        })
        .collect::<Vec<_>>();

    let Some(first) = addrs.first().copied() else {
        return Err(Exception::throw_internal(
            &ctx,
            &format!("ENOTFOUND {host}"),
        ));
    };

    if all {
        let list = rquickjs::Array::new(ctx.clone())?;
        for (index, addr) in addrs.into_iter().enumerate() {
            list.set(index, addr_value(&ctx, addr)?)?;
        }
        Ok(list.into_value())
    } else {
        addr_value(&ctx, first)
    }
}

fn addr_value<'js>(ctx: &Ctx<'js>, addr: std::net::SocketAddr) -> Result<Value<'js>> {
    let value = Object::new(ctx.clone())?;
    value.set("family", if addr.is_ipv4() { 4 } else { 6 })?;
    value.set("ip", addr.ip().to_string())?;
    Ok(value.into_value())
}
