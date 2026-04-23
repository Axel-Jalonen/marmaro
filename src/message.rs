use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Roles in a conversation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "assistant" => Role::Assistant,
            _ => Role::User,
        }
    }
}

/// A single chat message
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub id: String,
    pub conversation_id: String,
    pub role: Role,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub version: u64,
}

impl ChatMessage {
    pub fn new(conversation_id: &str, role: Role, content: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            role,
            content: content.to_string(),
            created_at: Utc::now(),
            version: 0,
        }
    }

    pub fn append_token(&mut self, token: &str) {
        self.content.push_str(token);
        self.version = self.version.wrapping_add(1);
    }
}

/// Metadata for a conversation shown in the sidebar
#[derive(Debug, Clone)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub system_prompt: String,
    pub model_id: String,
    pub region: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Conversation {
    pub fn new(title: &str, model_id: &str, region: &str) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.to_string(),
            system_prompt: String::new(),
            model_id: model_id.to_string(),
            region: region.to_string(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// A model entry: (display_name, model_id, provider_group)
pub struct ModelEntry {
    pub name: &'static str,
    pub id: &'static str,
    pub provider: &'static str,
}

/// All streaming-capable text models on Bedrock (cross-region inference profile IDs where available)
pub const MODELS: &[ModelEntry] = &[
    // ── Anthropic ──
    ModelEntry {
        name: "Claude Opus 4.6",
        id: "us.anthropic.claude-opus-4-6-v1",
        provider: "Anthropic",
    },
    ModelEntry {
        name: "Claude Opus 4.5",
        id: "us.anthropic.claude-opus-4-5-20251101-v1:0",
        provider: "Anthropic",
    },
    ModelEntry {
        name: "Claude Opus 4.1",
        id: "us.anthropic.claude-opus-4-1-20250805-v1:0",
        provider: "Anthropic",
    },
    ModelEntry {
        name: "Claude Sonnet 4.6",
        id: "us.anthropic.claude-sonnet-4-6",
        provider: "Anthropic",
    },
    ModelEntry {
        name: "Claude Sonnet 4.5",
        id: "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
        provider: "Anthropic",
    },
    ModelEntry {
        name: "Claude Sonnet 4",
        id: "us.anthropic.claude-sonnet-4-20250514-v1:0",
        provider: "Anthropic",
    },
    ModelEntry {
        name: "Claude Haiku 4.5",
        id: "us.anthropic.claude-haiku-4-5-20251001-v1:0",
        provider: "Anthropic",
    },
    ModelEntry {
        name: "Claude 3.5 Haiku",
        id: "us.anthropic.claude-3-5-haiku-20241022-v1:0",
        provider: "Anthropic",
    },
    ModelEntry {
        name: "Claude 3 Haiku",
        id: "anthropic.claude-3-haiku-20240307-v1:0",
        provider: "Anthropic",
    },
    // ── Amazon Nova ──
    ModelEntry {
        name: "Nova Premier",
        id: "us.amazon.nova-premier-v1:0",
        provider: "Amazon",
    },
    ModelEntry {
        name: "Nova Pro",
        id: "us.amazon.nova-pro-v1:0",
        provider: "Amazon",
    },
    ModelEntry {
        name: "Nova Lite",
        id: "us.amazon.nova-lite-v1:0",
        provider: "Amazon",
    },
    ModelEntry {
        name: "Nova Micro",
        id: "us.amazon.nova-micro-v1:0",
        provider: "Amazon",
    },
    ModelEntry {
        name: "Nova 2 Lite",
        id: "us.amazon.nova-2-lite-v1:0",
        provider: "Amazon",
    },
    // ── Meta Llama ──
    ModelEntry {
        name: "Llama 4 Maverick 17B",
        id: "us.meta.llama4-maverick-17b-instruct-v1:0",
        provider: "Meta",
    },
    ModelEntry {
        name: "Llama 4 Scout 17B",
        id: "us.meta.llama4-scout-17b-instruct-v1:0",
        provider: "Meta",
    },
    ModelEntry {
        name: "Llama 3.3 70B",
        id: "us.meta.llama3-3-70b-instruct-v1:0",
        provider: "Meta",
    },
    ModelEntry {
        name: "Llama 3.2 90B",
        id: "us.meta.llama3-2-90b-instruct-v1:0",
        provider: "Meta",
    },
    ModelEntry {
        name: "Llama 3.2 11B",
        id: "us.meta.llama3-2-11b-instruct-v1:0",
        provider: "Meta",
    },
    ModelEntry {
        name: "Llama 3.2 3B",
        id: "us.meta.llama3-2-3b-instruct-v1:0",
        provider: "Meta",
    },
    ModelEntry {
        name: "Llama 3.2 1B",
        id: "us.meta.llama3-2-1b-instruct-v1:0",
        provider: "Meta",
    },
    ModelEntry {
        name: "Llama 3.1 405B",
        id: "us.meta.llama3-1-405b-instruct-v1:0",
        provider: "Meta",
    },
    ModelEntry {
        name: "Llama 3.1 70B",
        id: "us.meta.llama3-1-70b-instruct-v1:0",
        provider: "Meta",
    },
    ModelEntry {
        name: "Llama 3.1 8B",
        id: "us.meta.llama3-1-8b-instruct-v1:0",
        provider: "Meta",
    },
    // ── DeepSeek ──
    ModelEntry {
        name: "DeepSeek R1",
        id: "us.deepseek.r1-v1:0",
        provider: "DeepSeek",
    },
    ModelEntry {
        name: "DeepSeek V3.2",
        id: "deepseek.v3.2",
        provider: "DeepSeek",
    },
    ModelEntry {
        name: "DeepSeek V3.1",
        id: "deepseek.v3-v1:0",
        provider: "DeepSeek",
    },
    // ── Mistral AI ──
    ModelEntry {
        name: "Mistral Large 3 675B",
        id: "mistral.mistral-large-3-675b-instruct",
        provider: "Mistral",
    },
    ModelEntry {
        name: "Devstral 2 123B",
        id: "mistral.devstral-2-123b",
        provider: "Mistral",
    },
    ModelEntry {
        name: "Magistral Small",
        id: "mistral.magistral-small-2509",
        provider: "Mistral",
    },
    ModelEntry {
        name: "Pixtral Large",
        id: "us.mistral.pixtral-large-2502-v1:0",
        provider: "Mistral",
    },
    ModelEntry {
        name: "Ministral 14B",
        id: "mistral.ministral-3-14b-instruct",
        provider: "Mistral",
    },
    ModelEntry {
        name: "Ministral 8B",
        id: "mistral.ministral-3-8b-instruct",
        provider: "Mistral",
    },
    ModelEntry {
        name: "Ministral 3B",
        id: "mistral.ministral-3-3b-instruct",
        provider: "Mistral",
    },
    ModelEntry {
        name: "Mixtral 8x7B",
        id: "mistral.mixtral-8x7b-instruct-v0:1",
        provider: "Mistral",
    },
    ModelEntry {
        name: "Mistral 7B",
        id: "mistral.mistral-7b-instruct-v0:2",
        provider: "Mistral",
    },
    // ── Qwen ──
    ModelEntry {
        name: "Qwen3 Coder Next",
        id: "qwen.qwen3-coder-next",
        provider: "Qwen",
    },
    ModelEntry {
        name: "Qwen3 Coder 480B",
        id: "qwen.qwen3-coder-480b-a35b-v1:0",
        provider: "Qwen",
    },
    ModelEntry {
        name: "Qwen3 Coder 30B",
        id: "qwen.qwen3-coder-30b-a3b-v1:0",
        provider: "Qwen",
    },
    ModelEntry {
        name: "Qwen3 VL 235B",
        id: "qwen.qwen3-vl-235b-a22b",
        provider: "Qwen",
    },
    ModelEntry {
        name: "Qwen3 235B",
        id: "qwen.qwen3-235b-a22b-2507-v1:0",
        provider: "Qwen",
    },
    ModelEntry {
        name: "Qwen3 32B",
        id: "qwen.qwen3-32b-v1:0",
        provider: "Qwen",
    },
    ModelEntry {
        name: "Qwen3 Next 80B",
        id: "qwen.qwen3-next-80b-a3b",
        provider: "Qwen",
    },
    // ── Moonshot / Kimi ──
    ModelEntry {
        name: "Kimi K2.5",
        id: "moonshotai.kimi-k2.5",
        provider: "Moonshot",
    },
    ModelEntry {
        name: "Kimi K2 Thinking",
        id: "moonshot.kimi-k2-thinking",
        provider: "Moonshot",
    },
    // ── Google ──
    ModelEntry {
        name: "Gemma 3 27B",
        id: "google.gemma-3-27b-it",
        provider: "Google",
    },
    ModelEntry {
        name: "Gemma 3 12B",
        id: "google.gemma-3-12b-it",
        provider: "Google",
    },
    ModelEntry {
        name: "Gemma 3 4B",
        id: "google.gemma-3-4b-it",
        provider: "Google",
    },
    // ── NVIDIA ──
    ModelEntry {
        name: "Nemotron Nano 3 30B",
        id: "nvidia.nemotron-nano-3-30b",
        provider: "NVIDIA",
    },
    ModelEntry {
        name: "Nemotron Nano 12B VL",
        id: "nvidia.nemotron-nano-12b-v2",
        provider: "NVIDIA",
    },
    ModelEntry {
        name: "Nemotron Nano 9B",
        id: "nvidia.nemotron-nano-9b-v2",
        provider: "NVIDIA",
    },
    // ── MiniMax ──
    ModelEntry {
        name: "MiniMax M2.1",
        id: "minimax.minimax-m2.1",
        provider: "MiniMax",
    },
    ModelEntry {
        name: "MiniMax M2",
        id: "minimax.minimax-m2",
        provider: "MiniMax",
    },
    // ── Writer ──
    ModelEntry {
        name: "Palmyra X5",
        id: "us.writer.palmyra-x5-v1:0",
        provider: "Writer",
    },
    ModelEntry {
        name: "Palmyra X4",
        id: "us.writer.palmyra-x4-v1:0",
        provider: "Writer",
    },
    // ── Cohere ──
    ModelEntry {
        name: "Command R+",
        id: "cohere.command-r-plus-v1:0",
        provider: "Cohere",
    },
    ModelEntry {
        name: "Command R",
        id: "cohere.command-r-v1:0",
        provider: "Cohere",
    },
    // ── AI21 ──
    ModelEntry {
        name: "Jamba 1.5 Large",
        id: "ai21.jamba-1-5-large-v1:0",
        provider: "AI21",
    },
    ModelEntry {
        name: "Jamba 1.5 Mini",
        id: "ai21.jamba-1-5-mini-v1:0",
        provider: "AI21",
    },
    // ── Z.AI ──
    ModelEntry {
        name: "GLM 4.7",
        id: "zai.glm-4.7",
        provider: "Z.AI",
    },
    ModelEntry {
        name: "GLM 4.7 Flash",
        id: "zai.glm-4.7-flash",
        provider: "Z.AI",
    },
    // ── OpenAI (on Bedrock) ──
    ModelEntry {
        name: "GPT OSS 120B",
        id: "openai.gpt-oss-120b-1:0",
        provider: "OpenAI",
    },
    ModelEntry {
        name: "GPT OSS 20B",
        id: "openai.gpt-oss-20b-1:0",
        provider: "OpenAI",
    },
];

/// Available AWS regions for Bedrock
pub const REGIONS: &[&str] = &[
    "us-east-1",
    "us-east-2",
    "us-west-2",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eu-north-1",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ca-central-1",
    "sa-east-1",
];

/// Token usage from a single API call
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenUsage {
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
}
