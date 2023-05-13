use axum::headers::{Error, Header, HeaderName, HeaderValue};
use base64::{engine::general_purpose::STANDARD as base64, Engine};

pub struct ContentMd5([u8; 16]);

static HEADER_NAME: HeaderName = HeaderName::from_static("content-md5");

impl Header for ContentMd5 {
    fn name() -> &'static HeaderName {
        &HEADER_NAME
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i HeaderValue>,
    {
        let value = values.next().ok_or_else(Error::invalid)?;

        // Ensure base64 encoded length fits the expected MD5 digest length.
        if value.len() < 22 || value.len() > 24 {
            return Err(Error::invalid());
        }

        let value = value.to_str().map_err(|_| Error::invalid())?;
        let mut slice = [0; 16];
        base64
            .decode_slice(value, &mut slice)
            .map_err(|_| Error::invalid())?;
        Ok(Self(slice))
    }

    fn encode<E>(&self, values: &mut E)
    where
        E: Extend<HeaderValue>,
    {
        let encoded = base64.encode(self.0);
        if let Ok(value) = HeaderValue::from_str(&encoded) {
            values.extend(std::iter::once(value));
        }
    }
}
