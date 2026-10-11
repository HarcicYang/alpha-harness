//! Network seam and local HTTP driver, with record and replay for tests.

#![forbid(unsafe_code)]

pub mod cassette;
pub mod local;
pub mod seam;

use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use serde::Deserialize;

pub use cassette::{Cassette, CassetteEntry, RecordingHttpClient, ReplayHttpClient};
pub use local::LocalHttpClient;
pub use seam::{HttpClient, HttpError, HttpRequest, HttpResponse, HttpResponseStream};

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "http.local";

/// Configuration for the local HTTP driver.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct HttpConfig {}

/// Provides the HTTP seam backed by the local machine.
pub struct HttpLocalModule;

impl Module for HttpLocalModule {
    type Requirements = ();
    type Config = HttpConfig;

    const KEY: &'static str = "http.local";
    const CAPABILITIES: &'static [CapabilitySpec] =
        &[CapabilitySpec::singleton("http", CapabilityScope::Location)];

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        let client: Arc<dyn HttpClient> = Arc::new(LocalHttpClient::new());
        ctx.provide::<dyn HttpClient>(client)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
