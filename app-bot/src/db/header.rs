use std::str::FromStr;

use reqwest::header::{HeaderName, HeaderValue};

use crate::db::error::ParseHeaderError;

pub const SAFARI_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.4 Safari/605.1.15";

pub fn parse_headers(
    str: impl AsRef<str>,
) -> Result<Vec<(HeaderName, HeaderValue)>, ParseHeaderError> {
    str.as_ref()
        .split(';')
        .map(|pair| {
            pair.trim()
                .split_once('=')
                .ok_or(ParseHeaderError::FormatError)
                .and_then(|(k, v)| -> Result<_, ParseHeaderError> {
                    Ok((
                        HeaderName::from_str(k)
                            .map_err(|_| ParseHeaderError::InvalidPair(k.to_string()))?,
                        HeaderValue::from_str(v)
                            .map_err(|_| ParseHeaderError::InvalidPair(k.to_string()))?,
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()
}
