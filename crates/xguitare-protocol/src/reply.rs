use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `get` reply: one entry per Parameter asked for.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GetReply {
    pub params: Vec<ParamStatus>,
}

/// A Parameter as `get` lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParamStatus {
    pub module: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub param: String,
    pub value: f64,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub integer: bool,
    pub labels: Vec<String>,
    pub bypass: bool,
    /// `None` on a Module that is not a cabinet (no `ir` key); `Some(None)` on a cabinet without a Cabinet IR (`"ir": null`).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub ir: Option<Option<String>>,
}

/// The `items` reply: every Track of the open Project with its Items.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemsReply {
    pub sample_rate: u32,
    pub tracks: Vec<ItemsTrack>,
}

/// A Track as `items` lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemsTrack {
    pub id: u64,
    pub name: String,
    pub items: Vec<ItemStatus>,
}

/// An Item as `items` lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemStatus {
    pub id: u64,
    pub position: u64,
    pub bars_beats: String,
    pub offset: u64,
    pub length: u64,
    pub take: usize,
    pub takes: Vec<String>,
    /// The active Take's frames; `null` without one.
    pub take_frames: Option<u64>,
}

/// A typed reply as the payload of `Response::Ok`.
///
/// # Panics
///
/// When `reply` does not serialise to JSON; the reply structs of this crate always do.
pub fn to_payload<T: Serialize>(reply: &T) -> Value {
    serde_json::to_value(reply).expect("a reply struct serialises to JSON")
}

/// The payload of `Response::Ok` read as a typed reply; a missing key reads as its default and an unknown key is ignored.
///
/// # Errors
///
/// The `serde_json` error when a key holds a value of the wrong type.
pub fn from_payload<T: DeserializeOwned>(payload: &Value) -> Result<T, serde_json::Error> {
    T::deserialize(payload)
}

/// Reads a key that is there, also as `null`, as `Some`; a missing key stays `None` through `#[serde(default)]`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
