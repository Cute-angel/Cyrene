pub mod app;
pub mod auth;
pub mod config;
pub mod domain;
pub mod embedding;
pub mod error;
pub mod protocol;
pub mod search;
pub mod singleton;
pub mod storage;
pub mod transport;

pub use app::Application;
pub use config::CoreConfig;
pub use embedding::{EmbeddingProvider, OpenAiCompatibleEmbeddingProvider};
pub use protocol::{PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope};
pub use storage::Storage;
