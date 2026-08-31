//! Fetch `data:` URL processing delegated to Servo's WHATWG implementation.

pub struct DataUrl {
    pub mime: String,
    pub body: Vec<u8>,
}

pub fn parse(input: &str) -> Option<DataUrl> {
    let mut url = reqwest::Url::parse(input).ok()?;
    if url.scheme() != "data" {
        return None;
    }
    url.set_fragment(None);
    let parsed = data_url::DataUrl::process(url.as_str()).ok()?;
    let mime = parsed.mime_type().to_string();
    let (body, _fragment) = parsed.decode_to_vec().ok()?;
    Some(DataUrl { mime, body })
}

#[cfg(test)]
#[path = "../../tests/unit/data_url.rs"]
mod tests;
