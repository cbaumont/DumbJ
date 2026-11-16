use std::fmt;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ureq::config::Config;
use ureq::tls::{RootCerts, TlsConfig};

use crate::ReviewError;

pub struct Client {
    endpoint: String,
    token: String,
    agent: ureq::Agent,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct Request<'a> {
    query: &'a str,
    variables: Value,
}

#[derive(Deserialize)]
struct Envelope {
    data: Option<Value>,
    errors: Option<Vec<GraphqlError>>,
}

#[derive(Deserialize)]
struct GraphqlError {
    message: String,
}

impl Client {
    pub fn new(host: &str, token: String) -> Self {
        let endpoint = if host == "github.com" {
            "https://api.github.com/graphql".to_string()
        } else {
            format!("https://{host}/api/graphql")
        };
        Self::with_endpoint(endpoint, token)
    }

    pub fn with_endpoint(endpoint: String, token: String) -> Self {
        let config = Config::builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();
        Self {
            endpoint,
            token,
            agent: config.into(),
        }
    }

    /// Returns the `data` member as `T`; a non-empty `errors` array is an error even with data.
    pub fn graphql<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: Value,
    ) -> Result<T, ReviewError> {
        let body = serde_json::to_string(&Request { query, variables })
            .map_err(|e| ReviewError::Decode(e.to_string()))?;
        let mut response = self
            .agent
            .post(&self.endpoint)
            .header("Authorization", format!("bearer {}", self.token))
            .header("User-Agent", "dumbj")
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| ReviewError::Http(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| ReviewError::Http(e.to_string()))?;
        match status {
            200..=299 => {}
            401 => return Err(ReviewError::Unauthorized),
            _ => {
                return Err(ReviewError::Http(format!(
                    "HTTP {status}: {}",
                    text.chars().take(200).collect::<String>()
                )));
            }
        }
        let envelope: Envelope =
            serde_json::from_str(&text).map_err(|e| ReviewError::Decode(e.to_string()))?;
        if let Some(errors) = envelope.errors.filter(|e| !e.is_empty()) {
            let messages: Vec<_> = errors.into_iter().map(|e| e.message).collect();
            return Err(ReviewError::Api(messages.join("; ")));
        }
        let data = envelope
            .data
            .ok_or_else(|| ReviewError::Decode("response has no data".to_string()))?;
        serde_json::from_value(data).map_err(|e| ReviewError::Decode(e.to_string()))
    }
}
