//! Optional per-agent authentication. Values must never be included in diagnostics.
use crate::Result;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Auth {
    #[serde(default)]
    pub bearer_token: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}
impl Auth {
    pub fn headers(&self) -> Result<HeaderMap> {
        let mut result = HeaderMap::new();
        for (name, value) in &self.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| "Invalid authentication header name")?;
            if matches!(
                name.as_str(),
                "host"
                    | "content-length"
                    | "transfer-encoding"
                    | "connection"
                    | "content-type"
                    | "a2a-version"
                    | "proxy-authorization"
                    | "cookie"
            ) {
                return Err("Reserved header cannot be configured".into());
            }
            if result.contains_key(&name) {
                return Err("Duplicate authentication header".into());
            }
            let mut value =
                HeaderValue::from_str(value).map_err(|_| "Invalid authentication header value")?;
            value.set_sensitive(true);
            result.insert(name, value);
        }
        if !self.bearer_token.is_empty() {
            if result.contains_key(AUTHORIZATION) {
                return Err("Choose Bearer token or Authorization header, not both".into());
            }
            let mut value = HeaderValue::from_str(&format!("Bearer {}", self.bearer_token))
                .map_err(|_| "Invalid Bearer token")?;
            value.set_sensitive(true);
            result.insert(AUTHORIZATION, value);
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validation() {
        assert!(Auth::default().headers().unwrap().is_empty());
        let mut auth = Auth {
            bearer_token: "secret".into(),
            ..Auth::default()
        };
        assert_eq!(auth.headers().unwrap()[AUTHORIZATION], "Bearer secret");
        auth.headers.insert("Authorization".into(), "other".into());
        assert!(auth.headers().is_err());
        auth.headers.clear();
        auth.headers.insert("Host".into(), "x".into());
        assert!(auth.headers().is_err());
        auth.headers.clear();
        auth.bearer_token = "secret\r\nx:y".into();
        assert!(auth.headers().is_err());
    }
}
