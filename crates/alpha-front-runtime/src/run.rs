//! Headless one-shot front: run one prompt and exit.

use alpha_host_api::{FrontError, FrontRequest};
use alpha_schema::{AssistantContentBlock, LiveAgentEvent, ToolStatus};

use crate::{
    ChatState, FrontEvent, FrontRuntime, FrontRuntimeConfig, SessionOptions, TranscriptItem,
    read_terminal_approval,
};

/// Run one prompt against a fresh or resumed session, streaming to the terminal.
pub fn run_once(request: FrontRequest) -> Result<(), FrontError> {
    let prompt = request.prompt_text();
    if prompt.trim().is_empty() && request.resume.is_none() {
        return Err(FrontError::message(
            "provide a prompt or use --resume <SESSION_ID>",
        ));
    }
    let mut options = SessionOptions::new(request.workspace);
    options.mode = request.mode;
    options.provider = request.provider;
    options.model = request.model;
    let mut runtime = FrontRuntime::start(FrontRuntimeConfig {
        session: options,
        ..FrontRuntimeConfig::default()
    })
    .map_err(|error| FrontError::message(error.to_string()))?;

    let session_id = match request.resume {
        Some(session_id) => {
            let resumed = runtime
                .resume_session(session_id)
                .map_err(|error| FrontError::message(error.to_string()))?;
            eprintln!(
                "alpha run: resumed {} ({} messages, repairs={})",
                session_id, resumed.messages, resumed.repairs
            );
            session_id
        }
        None => runtime
            .create_session()
            .map_err(|error| {
                FrontError::message(format!(
                    "{error}. Configure a provider with `alpha tui` first."
                ))
            })?
            .session_id,
    };

    if prompt.trim().is_empty() {
        return Ok(());
    }
    runtime
        .submit_prompt(&prompt)
        .map_err(|error| FrontError::message(error.to_string()))?;

    let mut streamed = false;
    loop {
        let events = runtime
            .receive_events(std::time::Duration::from_millis(100))
            .map_err(|error| FrontError::message(error.to_string()))?;
        for event in events {
            match event {
                FrontEvent::StateChanged => {}
                FrontEvent::Live { event } => match event {
                    LiveAgentEvent::TextDelta { text } => {
                        print!("{text}");
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                        streamed = true;
                    }
                    LiveAgentEvent::StepStarted { step } => {
                        eprintln!("alpha run: step {step}: calling model...");
                    }
                    LiveAgentEvent::ToolStarted { name, .. } => {
                        eprintln!("alpha run: -> {name}");
                    }
                    LiveAgentEvent::ToolFinished { name, status, .. } => {
                        eprintln!("alpha run: <- {name} {}", tool_status_label(status));
                    }
                    LiveAgentEvent::Error { message } => {
                        eprintln!("alpha run: {message}");
                    }
                    LiveAgentEvent::ThinkingDelta { .. } => {}
                },
                FrontEvent::InteractionRequested { .. } => {
                    let prompt = runtime
                        .chat()
                        .interaction
                        .as_ref()
                        .map(|prompt| prompt.prompt.clone())
                        .unwrap_or_default();
                    let decision = read_terminal_approval(&prompt);
                    runtime
                        .respond_interaction(decision)
                        .map_err(|error| FrontError::message(error.to_string()))?;
                }
                FrontEvent::TurnEnded { .. } => {
                    if streamed {
                        println!();
                    } else {
                        let final_text = final_assistant_text(runtime.chat());
                        if !final_text.is_empty() {
                            println!("{final_text}");
                        }
                    }
                    eprintln!("session: {session_id}");
                    return Ok(());
                }
                FrontEvent::CoreRestarted { resumed } => {
                    return Err(FrontError::message(format!(
                        "core restarted; session {}",
                        if resumed { "resumed" } else { "not resumed" }
                    )));
                }
                FrontEvent::CoreDisconnected { error } => {
                    return Err(FrontError::message(format!("core disconnected: {error}")));
                }
            }
        }
    }
}

fn final_assistant_text(chat: &ChatState) -> String {
    chat.transcript
        .iter()
        .rev()
        .find_map(|item| match item {
            TranscriptItem::AssistantMessage { blocks } => Some(
                blocks
                    .iter()
                    .filter_map(AssistantContentBlock::text)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn tool_status_label(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Completed => "completed",
        ToolStatus::Failed => "failed",
        ToolStatus::Interrupted => "interrupted",
    }
}

/// Runtime configuration for one front request.
pub fn runtime_config(request: &FrontRequest) -> FrontRuntimeConfig {
    let mut session = SessionOptions::new(&request.workspace);
    session.mode = request.mode;
    session.provider = request.provider.clone();
    session.model = request.model.clone();
    FrontRuntimeConfig {
        session,
        ..FrontRuntimeConfig::default()
    }
}

/// Front entry used by the generated composition table.
pub fn __front_run(request: FrontRequest) -> Result<(), FrontError> {
    run_once(request)
}
