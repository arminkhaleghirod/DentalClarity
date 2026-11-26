//! A scripted text model for tests. It never touches the network.

use crate::{Result, TextModel, TextRequest, TextResponse, Usage};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Returns queued responses in order and records every request it receives.
#[derive(Debug, Default)]
pub struct FakeTextModel {
    responses: Mutex<VecDeque<String>>,
    requests: Mutex<Vec<TextRequest>>,
}

impl FakeTextModel {
    pub fn with_responses<I, S>(responses: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            responses: Mutex::new(responses.into_iter().map(Into::into).collect()),
            requests: Mutex::default(),
        }
    }

    pub fn requests(&self) -> Vec<TextRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl TextModel for FakeTextModel {
    fn provider(&self) -> &'static str {
        "fake"
    }

    async fn generate(&self, request: TextRequest) -> Result<TextResponse> {
        let model = request.model.clone();
        self.requests.lock().unwrap().push(request);
        let content = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("FakeTextModel ran out of scripted responses");
        Ok(TextResponse {
            model,
            content,
            usage: Usage::default(),
            latency_ms: 0,
        })
    }
}
