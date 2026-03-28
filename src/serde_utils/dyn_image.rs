use std::io::Cursor;

use anyhow::anyhow;
use image::DynamicImage;
use serde::{Deserializer, Serializer};
use serde_with::{DeserializeAs, SerializeAs};

pub struct DynImageConverter;

impl SerializeAs<DynamicImage> for DynImageConverter {
    fn serialize_as<S>(source: &DynamicImage, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut buf = Cursor::new(Vec::new());
        source
            .write_to(&mut buf, image::ImageFormat::WebP)
            .map_err(|err| {
                serde::ser::Error::custom(anyhow!(err).context("image encoding error"))
            })?;
        serializer.serialize_bytes(buf.get_ref())
    }
}

impl<'de> DeserializeAs<'de, DynamicImage> for DynImageConverter {
    fn deserialize_as<D>(deserializer: D) -> Result<DynamicImage, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: Vec<u8> = serde::de::Deserialize::deserialize(deserializer)?;
        image::load_from_memory(&bytes)
            .map_err(|err| serde::de::Error::custom(anyhow!(err).context("image decoding error")))
    }
}
