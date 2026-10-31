//! HTTP seam contract.
//!
//! API adapters and the fixed shell talk to these types only, so the HTTP
//! driver is a replaceable dependency rather than a fixed one.

#![forbid(unsafe_code)]

pub mod cassette;
pub mod seam;

pub use cassette::{Cassette, CassetteEntry, RecordingHttpClient, ReplayHttpClient};
pub use seam::{HttpClient, HttpError, HttpRequest, HttpResponse, HttpResponseStream};
