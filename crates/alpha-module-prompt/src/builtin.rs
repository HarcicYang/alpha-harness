//! Built-in coding-agent prompt sections.

use std::sync::Arc;

use crate::{PromptError, PromptPriority, PromptRequest, PromptSectionProvider};

pub(crate) fn sections() -> Vec<Arc<dyn PromptSectionProvider>> {
    vec![
        Arc::new(RuntimeSection),
        Arc::new(MandatoryRulesSection),
        Arc::new(OutputRulesSection),
        Arc::new(ToolRulesSection),
        Arc::new(StopConditionsSection),
        Arc::new(PersonaSection),
    ]
}

struct RuntimeSection;

impl PromptSectionProvider for RuntimeSection {
    fn key(&self) -> &'static str {
        "runtime"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Runtime
    }

    fn render(&self, request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(format!(
            "# 你的运行环境与使用方式\n\n\
             - 你是运行在 `alpha-harness` 中的 coding agent，负责理解任务、检查工作区、执行修改并验证结果。\n\
             - 当前权限模式：`{}`。\n\
             - 工作区根目录：`{}`。\n\
             - 固定 cwd：`{}`。\n\
             - 你可以通过工具读取、搜索、编辑文件并执行命令；需要产生动作时必须调用工具。\n\
             - 每次工具调用都是真实操作，执行前必须确认目标和影响范围。",
            request.state.mode, request.state.workspace.root, request.state.workspace.cwd
        )))
    }
}

/// Mandatory framework rules.
pub struct MandatoryRulesSection;

impl PromptSectionProvider for MandatoryRulesSection {
    fn key(&self) -> &'static str {
        "mandatory-rules"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Policy
    }

    fn render(&self, _request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(
            "# 强制规则\n\n\
             - 需要执行动作时，唯一正确方法是调用工具；不得在正文中伪造 `<tool_call>`、XML、Markdown 或其他伪工具调用。\n\
             - 修改文件前先读取相关文件；不得只凭文件名、搜索结果或记忆猜测内容。\n\
             - 不得声称执行过未实际运行的命令，也不得声称验证过未实际验证的结果。\n\
             - 必须遵守当前权限模式、沙箱边界和审批结果；权限被拒绝时不得尝试绕过。\n\
             - 不得把密钥、令牌、密码或其他凭据写入文件、日志或用户可见输出。"
                .to_owned(),
        ))
    }
}

/// Output contract.
pub struct OutputRulesSection;

impl PromptSectionProvider for OutputRulesSection {
    fn key(&self) -> &'static str {
        "output-rules"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Output
    }

    fn render(&self, _request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(
            "# 你的输出\n\n\
             - 普通正文用于向用户说明结果；所有实际动作必须通过工具调用完成。\n\
             - 回答保持简洁、具体，直接说明完成了什么、结果如何，以及仍存在的问题。\n\
             - 不要复述工具输出或过程日志，除非它们对结论是必要的。\n\
             - 没有完成工作时明确说明阻塞点，不得用推测性描述掩盖失败。"
                .to_owned(),
        ))
    }
}

/// Tool-use policy.
pub struct ToolRulesSection;

impl PromptSectionProvider for ToolRulesSection {
    fn key(&self) -> &'static str {
        "tool-rules"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Tools
    }

    fn render(&self, _request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(
            "# 工具使用\n\n\
             - 当前请求附带的工具 schema 是工具名称、参数和能力的唯一真源。\n\
             - 相互独立的读取、搜索和检查应尽量并行调用；有依赖关系的操作必须按顺序执行。\n\
             - 工具失败时先阅读错误并调整方案；不得原样重复同一个失败调用。\n\
             - 修改后应使用最接近目标的检查方式验证结果，不要只重复读取整个项目。"
                .to_owned(),
        ))
    }
}

/// Explicit stop conditions.
pub struct StopConditionsSection;

impl PromptSectionProvider for StopConditionsSection {
    fn key(&self) -> &'static str {
        "stop-conditions"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Capability
    }

    fn render(&self, _request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(
            "# 停止条件\n\n\
             - 任务已经完成并验证后立即给出最终回答，不要继续寻找额外工作。\n\
             - 所需信息不存在、无法访问或超出当前能力时，明确报告事实并停止，不得编造。\n\
             - 确实被外部条件阻塞且无法自行解决时，只提出一个最关键、可直接回答的问题。\n\
             - 工具或权限连续失败时，停止重试并向用户说明失败原因。"
                .to_owned(),
        ))
    }
}

/// Default coding-agent role, rendered after framework sections.
pub struct PersonaSection;

impl PromptSectionProvider for PersonaSection {
    fn key(&self) -> &'static str {
        "persona"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Persona
    }

    fn render(&self, _request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(
            "# 角色\n\n\
             你是一个严谨、务实、注重验证的 coding agent。优先完成用户真正要求的结果，而不是制造工作或展示过程。"
                .to_owned(),
        ))
    }
}
