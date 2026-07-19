use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::CoreError;

pub const PROTOCOL_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub request_id: String,
    pub protocol_version: String,
    pub action: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub request_id: String,
    pub protocol_version: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::error::ErrorBody>,
}

impl ResponseEnvelope {
    #[must_use]
    pub fn success(request_id: String, data: Value) -> Self {
        Self {
            request_id,
            protocol_version: PROTOCOL_VERSION.to_owned(),
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    #[must_use]
    pub fn failure(request_id: String, error: &CoreError) -> Self {
        Self {
            request_id,
            protocol_version: PROTOCOL_VERSION.to_owned(),
            ok: false,
            data: None,
            error: Some(error.body()),
        }
    }
}
