use cfx_types::{address_util::hex_to_address, Address};
use serde::{de, Deserialize};

/// Deserializes a [string][String] as a [u64].
pub fn deserialize_str_as_u64<'de, D>(
    deserializer: D,
) -> Result<u64, D::Error>
where D: de::Deserializer<'de> {
    let string = String::deserialize(deserializer)?;

    if let Some(stripped) = string.strip_prefix("0x") {
        u64::from_str_radix(stripped, 16)
    } else {
        string.parse()
    }
    .map_err(serde::de::Error::custom)
}

/// Deserializes a [string][String] as an optional [Address].
pub fn deserialize_maybe_empty<'de, D>(
    deserializer: D,
) -> Result<Option<Address>, D::Error>
where D: de::Deserializer<'de> {
    let string = String::deserialize(deserializer)?;
    if string.is_empty() {
        Ok(None)
    } else {
        hex_to_address(&string).map_err(de::Error::custom).map(Some)
    }
}
