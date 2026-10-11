use alpha_api_anthropic_messages::AnthropicMessagesApiModule;
use alpha_api_openai_chat::OpenAiChatApiModule;
use alpha_api_openai_responses::OpenAiResponsesApiModule;
use alpha_kernel::{Ctx, Module};
use alpha_module_provider_anthropic_messages::AnthropicMessagesProviderModule;
use alpha_module_provider_deepseek::DeepSeekProviderModule;
use alpha_module_provider_openai_chat_completions::OpenAiChatProviderModule;
use alpha_module_provider_openai_responses::OpenAiResponsesProviderModule;
use alpha_module_provider_opencode_go::OpenCodeGoProviderModule;
use alpha_provider_api::{ProviderApiModule, ProviderKindRegistry};

#[test]
fn default_composition_registers_all_first_provider_kinds() {
    let ctx = Ctx::root();
    ProviderApiModule.activate(&ctx, &()).expect("provider api");
    OpenAiChatApiModule
        .activate(&ctx, &())
        .expect("openai chat adapter");
    OpenAiResponsesApiModule
        .activate(&ctx, &())
        .expect("openai responses adapter");
    AnthropicMessagesApiModule
        .activate(&ctx, &())
        .expect("anthropic adapter");
    OpenAiChatProviderModule
        .activate(&ctx, &())
        .expect("openai chat provider");
    OpenAiResponsesProviderModule
        .activate(&ctx, &())
        .expect("openai responses provider");
    AnthropicMessagesProviderModule
        .activate(&ctx, &())
        .expect("anthropic provider");
    DeepSeekProviderModule
        .activate(&ctx, &())
        .expect("deepseek provider");
    OpenCodeGoProviderModule
        .activate(&ctx, &())
        .expect("opencode provider");

    let registry = ctx
        .require::<ProviderKindRegistry>()
        .expect("provider registry");
    let kinds: Vec<String> = registry
        .descriptors()
        .into_iter()
        .map(|descriptor| descriptor.kind)
        .collect();
    assert!(kinds.contains(&"openai-chat-completions".to_owned()));
    assert!(kinds.contains(&"openai-responses".to_owned()));
    assert!(kinds.contains(&"anthropic-messages".to_owned()));
    assert!(kinds.contains(&"deepseek".to_owned()));
    assert!(kinds.contains(&"opencode-go".to_owned()));

    let deepseek = registry.get("deepseek").expect("deepseek kind");
    assert!(
        deepseek
            .descriptor()
            .adapters
            .iter()
            .any(|adapter| adapter.key == "anthropic-messages" && adapter.required)
    );
    let opencode = registry.get("opencode-go").expect("opencode kind");
    assert_eq!(opencode.descriptor().adapters.len(), 3);
}
