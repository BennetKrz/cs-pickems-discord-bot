//! A [`Post`] as a Discord message, and the [`Outbox`] that sends posts
//! through Discord's REST API.

use std::sync::Arc;

use async_trait::async_trait;
use poise::serenity_prelude as serenity;

use crate::announce::{Outbox, OutboxError};
use crate::render::{LAUNCH_ID, Post};

/// The post's embed.
pub fn embed(post: &Post) -> serenity::CreateEmbed {
    let mut e = serenity::CreateEmbed::new()
        .title(&post.title)
        .color(crate::render::COLOR);
    if !post.description.is_empty() {
        e = e.description(&post.description);
    }
    for f in &post.fields {
        e = e.field(&f.name, &f.value, false);
    }
    e
}

/// The post's buttons as one action row (none if it has no buttons).
pub fn components(post: &Post) -> Vec<serenity::CreateActionRow> {
    if post.buttons.is_empty() {
        return Vec::new();
    }
    let buttons = post
        .buttons
        .iter()
        .map(|b| {
            let style = if b.custom_id == LAUNCH_ID {
                serenity::ButtonStyle::Primary
            } else {
                serenity::ButtonStyle::Secondary
            };
            serenity::CreateButton::new(&b.custom_id)
                .label(&b.label)
                .style(style)
        })
        .collect();
    vec![serenity::CreateActionRow::Buttons(buttons)]
}

/// The post as an (ephemeral or not) interaction reply.
pub fn reply(post: &Post, ephemeral: bool) -> poise::CreateReply {
    poise::CreateReply::default()
        .embed(embed(post))
        .components(components(post))
        .ephemeral(ephemeral)
}

/// Sends posts with the bot token over REST; needs no gateway connection.
pub struct DiscordOutbox {
    http: Arc<serenity::Http>,
}

impl DiscordOutbox {
    pub fn new(http: Arc<serenity::Http>) -> Self {
        Self { http }
    }
}

fn parse_id(s: &str) -> Result<u64, OutboxError> {
    s.parse()
        .ok()
        .filter(|&n| n != 0)
        .ok_or_else(|| OutboxError::Gone(format!("`{s}` is not a Discord ID")))
}

/// 403 and 404 mean the channel or message is gone or off-limits.
fn outbox_error(e: serenity::Error) -> OutboxError {
    if let serenity::Error::Http(http) = &e
        && let Some(status) = http.status_code()
        && matches!(status.as_u16(), 403 | 404)
    {
        return OutboxError::Gone(e.to_string());
    }
    OutboxError::Failed(e.to_string())
}

#[async_trait]
impl Outbox for DiscordOutbox {
    async fn send(
        &self,
        channel_id: &str,
        post: &Post,
        nonce: &str,
    ) -> Result<String, OutboxError> {
        let channel = serenity::ChannelId::new(parse_id(channel_id)?);
        let message = serenity::CreateMessage::new()
            .embed(embed(post))
            .components(components(post))
            .nonce(serenity::Nonce::String(nonce.to_owned()))
            .enforce_nonce(true);
        channel
            .send_message(&self.http, message)
            .await
            .map(|m| m.id.get().to_string())
            .map_err(outbox_error)
    }

    async fn edit(
        &self,
        channel_id: &str,
        message_id: &str,
        post: &Post,
    ) -> Result<(), OutboxError> {
        let channel = serenity::ChannelId::new(parse_id(channel_id)?);
        let message = serenity::MessageId::new(parse_id(message_id)?);
        let edit = serenity::EditMessage::new()
            .embed(embed(post))
            .components(components(post));
        channel
            .edit_message(&self.http, message, edit)
            .await
            .map(|_| ())
            .map_err(outbox_error)
    }
}
