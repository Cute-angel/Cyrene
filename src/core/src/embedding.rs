use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, CoreResult};

pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn model(&self) -> &str;
    fn dimensions(&self) -> usize;
    async fn embed(&self, inputs: &[String]) -> CoreResult<Vec<Vec<f32>>>;
}

pub type SharedEmbeddingProvider = Arc<dyn EmbeddingProvider>;

#[derive(Clone)]
pub struct OpenAiCompatibleEmbeddingProvider {
    client: Client,
    endpoint_url: Url,
    api_key: Option<String>,
    model: String,
    dimensions: usize,
}

impl OpenAiCompatibleEmbeddingProvider {
    pub fn new(
        base_url: String,
        api_key: Option<String>,
        model: String,
        dimensions: usize,
        timeout: Duration,
    ) -> CoreResult<Self> {
        if model.trim().is_empty() {
            return Err(CoreError::Config(
                "embedding model must not be empty".into(),
            ));
        }
        if dimensions == 0 {
            return Err(CoreError::Config(
                "embedding dimensions must be positive".into(),
            ));
        }
        let endpoint_url = embeddings_endpoint(&base_url)?;
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| CoreError::Config(format!("could not build HTTP client: {error}")))?;
        Ok(Self {
            client,
            endpoint_url,
            api_key: api_key.and_then(|key| {
                let key = key.trim().to_owned();
                (!key.is_empty()).then_some(key)
            }),
            model,
            dimensions,
        })
    }

    pub fn from_environment(
        base_url: String,
        api_key_env: &str,
        model: String,
        dimensions: usize,
        timeout: Duration,
    ) -> CoreResult<Option<Self>> {
        let api_key_env = api_key_env.trim();
        if api_key_env.is_empty() {
            return Self::new(base_url, None, model, dimensions, timeout).map(Some);
        }
        match std::env::var(api_key_env) {
            Ok(key) if !key.trim().is_empty() => {
                Self::new(base_url, Some(key), model, dimensions, timeout).map(Some)
            }
            Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
            Err(error) => Err(CoreError::Config(format!(
                "embedding API key environment variable {api_key_env} is not valid Unicode: {error}"
            ))),
        }
    }
}

fn embeddings_endpoint(base_url: &str) -> CoreResult<Url> {
    let base_url = base_url.trim().trim_end_matches('/');
    let endpoint = format!("{base_url}/embeddings");
    let url = Url::parse(&endpoint)
        .map_err(|error| CoreError::Config(format!("embedding base URL is invalid: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(CoreError::Config(
            "embedding base URL must be an absolute HTTP or HTTPS URL".into(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(CoreError::Config(
            "embedding base URL must not contain a query or fragment".into(),
        ));
    }
    Ok(url)
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    input: &'a [String],
    model: &'a str,
    dimensions: usize,
    encoding_format: &'static str,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

#[derive(Deserialize)]
struct OpenAiErrorEnvelope {
    error: Option<OpenAiError>,
}

#[derive(Deserialize)]
struct OpenAiError {
    message: String,
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatibleEmbeddingProvider {
    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed(&self, inputs: &[String]) -> CoreResult<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        if inputs.iter().any(|input| input.trim().is_empty()) {
            return Err(CoreError::Validation(
                "embedding input cannot be empty".into(),
            ));
        }
        let mut request = self.client.post(self.endpoint_url.clone());
        if let Some(api_key) = &self.api_key {
            request = request.bearer_auth(api_key);
        }
        let response = request
            .json(&EmbeddingRequest {
                input: inputs,
                model: &self.model,
                dimensions: self.dimensions,
                encoding_format: "float",
            })
            .send()
            .await
            .map_err(|error| CoreError::Embedding(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .json::<OpenAiErrorEnvelope>()
                .await
                .ok()
                .and_then(|body| body.error)
                .map_or_else(|| status.to_string(), |error| error.message);
            return Err(CoreError::Embedding(format!(
                "OpenAI-compatible embedding service returned {status}: {body}"
            )));
        }
        let mut data = response
            .json::<EmbeddingResponse>()
            .await
            .map_err(|error| {
                CoreError::Embedding(format!(
                    "invalid OpenAI-compatible embedding response: {error}"
                ))
            })?
            .data;
        data.sort_by_key(|item| item.index);
        if data.len() != inputs.len() {
            return Err(CoreError::Embedding(format!(
                "embedding service returned {} vectors for {} inputs",
                data.len(),
                inputs.len()
            )));
        }
        let vectors = data
            .into_iter()
            .map(|item| item.embedding)
            .collect::<Vec<_>>();
        if vectors.iter().any(|vector| vector.len() != self.dimensions) {
            return Err(CoreError::Embedding(format!(
                "embedding service returned a vector with dimensions other than {}",
                self.dimensions
            )));
        }
        Ok(vectors)
    }
}

#[must_use]
pub fn content_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[cfg(test)]
pub(crate) mod tests {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use serde_json::{Value, json};

    use super::*;

    #[derive(Clone)]
    pub struct FakeEmbeddingProvider {
        pub fail: bool,
        pub dimensions: usize,
    }

    #[async_trait]
    impl EmbeddingProvider for FakeEmbeddingProvider {
        fn model(&self) -> &str {
            "fake"
        }

        fn dimensions(&self) -> usize {
            self.dimensions
        }

        async fn embed(&self, inputs: &[String]) -> CoreResult<Vec<Vec<f32>>> {
            if self.fail {
                return Err(CoreError::Embedding("simulated outage".into()));
            }
            Ok(inputs
                .iter()
                .map(|input| {
                    let mut vector = vec![0.0; self.dimensions];
                    for (index, byte) in input.bytes().enumerate() {
                        vector[index % self.dimensions] += f32::from(byte) / 255.0;
                    }
                    vector
                })
                .collect())
        }
    }

    #[test]
    fn builds_openai_compatible_endpoint_without_api_key() {
        let provider = OpenAiCompatibleEmbeddingProvider::new(
            "http://localhost:11434/v1/".into(),
            None,
            "nomic-embed-text".into(),
            768,
            Duration::from_secs(30),
        )
        .unwrap();

        assert_eq!(
            provider.endpoint_url.as_str(),
            "http://localhost:11434/v1/embeddings"
        );
        assert!(provider.api_key.is_none());
    }

    #[tokio::test]
    async fn sends_openai_compatible_request_and_orders_batch_response() {
        async fn embeddings(headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
            assert_eq!(headers["authorization"], "Bearer local-key");
            assert_eq!(body["model"], "local-model");
            assert_eq!(body["input"], json!(["first", "second"]));
            assert_eq!(body["dimensions"], 3);
            assert_eq!(body["encoding_format"], "float");
            Json(json!({
                "data": [
                    { "index": 1, "embedding": [4.0, 5.0, 6.0] },
                    { "index": 0, "embedding": [1.0, 2.0, 3.0] }
                ]
            }))
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/v1/embeddings", post(embeddings)),
            )
            .await
            .unwrap();
        });
        let provider = OpenAiCompatibleEmbeddingProvider::new(
            format!("http://{address}/v1"),
            Some("local-key".into()),
            "local-model".into(),
            3,
            Duration::from_secs(5),
        )
        .unwrap();

        let vectors = provider
            .embed(&["first".into(), "second".into()])
            .await
            .unwrap();
        server.abort();

        assert_eq!(vectors, vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]);
    }
}
