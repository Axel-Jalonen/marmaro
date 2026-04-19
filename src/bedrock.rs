use anyhow::{Context, Result};
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ContentBlockDelta, ConversationRole, InferenceConfiguration, Message,
    SystemContentBlock,
};
use aws_sdk_bedrockruntime::types::ConverseStreamOutput as StreamEvent;
use eframe::egui;
use tokio::sync::mpsc;
use tracing::{error, info};

/// Events sent from the background streaming task to the UI
#[derive(Debug)]
pub enum StreamToken {
    Delta(String),
    Done,
    Error(String),
}

/// Build a Bedrock client for the given region.
///
/// Auth is handled automatically by the SDK:
/// - If `AWS_BEARER_TOKEN_BEDROCK` is set (Bedrock API key), it uses bearer auth.
/// - Otherwise falls back to the standard credential chain (~/.aws, env vars, SSO, IMDS).
async fn make_client(region: &str) -> Result<aws_sdk_bedrockruntime::Client> {
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .load()
        .await;
    Ok(aws_sdk_bedrockruntime::Client::new(&config))
}

/// Spawn a tokio task that streams a Bedrock ConverseStream response.
pub fn spawn_stream(
    rt: &tokio::runtime::Handle,
    ctx: egui::Context,
    model_id: String,
    region: String,
    system_prompt: String,
    history: Vec<(String, String)>,
) -> mpsc::UnboundedReceiver<StreamToken> {
    let (tx, rx) = mpsc::unbounded_channel();

    rt.spawn(async move {
        if let Err(e) =
            run_stream(&tx, &ctx, &model_id, &region, &system_prompt, &history).await
        {
            error!("Bedrock stream error: {e:#}");
            let _ = tx.send(StreamToken::Error(format!("{e:#}")));
            ctx.request_repaint();
        }
    });

    rx
}

async fn run_stream(
    tx: &mpsc::UnboundedSender<StreamToken>,
    ctx: &egui::Context,
    model_id: &str,
    region: &str,
    system_prompt: &str,
    history: &[(String, String)],
) -> Result<()> {
    let client = make_client(region).await?;

    let mut messages = Vec::new();
    for (role, content) in history {
        let conv_role = match role.as_str() {
            "assistant" => ConversationRole::Assistant,
            _ => ConversationRole::User,
        };
        let msg = Message::builder()
            .role(conv_role)
            .content(ContentBlock::Text(content.clone()))
            .build()
            .context("building Message")?;
        messages.push(msg);
    }

    info!(model = model_id, region, messages = messages.len(), "starting converse_stream");

    let mut req = client
        .converse_stream()
        .model_id(model_id)
        .inference_config(
            InferenceConfiguration::builder()
                .max_tokens(4096)
                .build(),
        );

    if !system_prompt.is_empty() {
        req = req.system(SystemContentBlock::Text(system_prompt.to_string()));
    }

    for msg in messages {
        req = req.messages(msg);
    }

    let resp = req.send().await.context("converse_stream send")?;
    let mut stream = resp.stream;

    loop {
        match stream.recv().await {
            Ok(Some(event)) => match event {
                StreamEvent::ContentBlockDelta(delta_event) => {
                    if let Some(ContentBlockDelta::Text(text)) = delta_event.delta {
                        let _ = tx.send(StreamToken::Delta(text));
                        ctx.request_repaint();
                    }
                }
                StreamEvent::MessageStop(_) => {
                    let _ = tx.send(StreamToken::Done);
                    ctx.request_repaint();
                    break;
                }
                _ => {}
            },
            Ok(None) => {
                let _ = tx.send(StreamToken::Done);
                ctx.request_repaint();
                break;
            }
            Err(e) => {
                let msg = format!("{e:#}");
                error!("stream recv error: {msg}");
                let _ = tx.send(StreamToken::Error(msg));
                ctx.request_repaint();
                break;
            }
        }
    }

    Ok(())
}
