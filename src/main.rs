use eyre::Context;
use hanabot::{Hanabi, MessageProxy};
use slack_morphism::prelude::*;
use slack_morphism::{SlackApiToken, SlackApiTokenValue};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let app_token_value: SlackApiTokenValue = std::env::var("SLACK_APP_TOKEN")
        .expect("SLACK_APP_TOKEN was not set")
        .into();
    let app_token: SlackApiToken = SlackApiToken::new(app_token_value);

    let api_token_value: SlackApiTokenValue = std::env::var("SLACK_API_TOKEN")
        .expect("SLACK_API_TOKEN was not set")
        .into();
    let api_token: SlackApiToken = SlackApiToken::new(api_token_value);

    let hanabi = Hanabi::resume()
        .await
        .context("resume from saved game states")?
        .unwrap_or_default();

    let state = Arc::new(State {
        api_token,
        hanabi: Mutex::new(hanabi),
    });

    let socket_mode_callbacks =
        SlackSocketModeListenerCallbacks::new().with_push_events(on_push_event);

    let client = Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));
    let listener_environment = Arc::new(
        SlackClientEventsListenerEnvironment::new(client.clone())
            .with_error_handler(on_error)
            .with_user_state(Arc::clone(&state)),
    );

    let socket_mode_listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        listener_environment.clone(),
        socket_mode_callbacks,
    );

    // Register an app token to listen for events,
    socket_mode_listener
        .listen_for(&app_token)
        .await
        .context("listen in socket mode")?;

    // Start WS connections calling Slack API to get WS url for the token,
    // and wait for Ctrl-C to shutdown
    socket_mode_listener.serve().await;

    // we're exiting; serialize state so we can later resume
    {
        let hanabi = state.hanabi.lock().await;
        hanabi.save().await
    }
}

struct State {
    api_token: SlackApiToken,
    hanabi: Mutex<Hanabi>,
}

fn on_error(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackHyperClient>,
    _states: SlackClientEventsUserState,
) -> http::StatusCode {
    eprintln!("{err:?}");

    // This return value should be OK if we want to return successful ack
    // to the Slack server using Web-sockets
    // https://api.slack.com/apis/connections/socket-implement#acknowledge
    // so that Slack knows whether to retry
    http::StatusCode::OK
}

async fn on_push_event(
    event: SlackPushEventCallback,
    client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let SlackEventCallbackBody::Message(m) = event.event else {
        return Ok(());
    };

    if !m
        .subtype
        .is_none_or(|ct| ct == SlackMessageEventType::MeMessage)
    {
        return Ok(());
    }

    let Some(user) = m.sender.user else {
        return Ok(());
    };
    let Some(text) = m.content.as_ref().and_then(|v| v.text.as_ref()) else {
        return Ok(());
    };
    let Some(_channel) = m.origin.channel else {
        return Ok(());
    };
    if m.origin.channel_type.is_none_or(|ct| ct.0 != "im") {
        return Ok(());
    }

    let states = states.read().await;
    let state = states
        .get_user_state::<Arc<State>>()
        .expect("we always use hanabi as user state");

    let mut hanabi = state.hanabi.lock().await;
    let cli = client.open_session(&state.api_token);
    let mut messages = ApiMessageProxy::new(cli);

    hanabi
        .on_dm_recv(text, user, &mut messages)
        .await
        .context("handle dm message")?;

    messages.flush().await.context("flush user messages")?;

    Ok(())
}

/// `MessageProxy` buffers messages that are to be sent to a user in a given turn, and flushes them
/// in a single private message to each user when the turn has completed. This avoids sending lots
/// of notifications to each user, and hides Slack API details such as the distinction between user
/// ids and channel ids from `hanabi::Game`.
pub struct ApiMessageProxy<'a> {
    cli: SlackClientSession<'a, SlackClientHyperHttpsConnector>,
    msgs: HashMap<String, Vec<String>>,
}

impl<'a> ApiMessageProxy<'a> {
    pub fn new(cli: SlackClientSession<'a, SlackClientHyperHttpsConnector>) -> Self {
        Self {
            cli,
            msgs: Default::default(),
        }
    }

    async fn flush(&mut self) -> eyre::Result<()> {
        for (user, msgs) in self.msgs.drain() {
            let _ = self
                .cli
                .chat_post_message(
                    &SlackApiChatPostMessageRequest::new(
                        SlackChannelId(user.clone()),
                        SlackMessageContent::new().with_text(msgs.join("\n")),
                    )
                    .without_unfurl_links(),
                )
                .await
                .with_context(|| format!("send to {user}"))?;
        }

        Ok(())
    }
}

impl<'a> MessageProxy for ApiMessageProxy<'a> {
    fn send(&mut self, user: &str, text: &str) {
        self.msgs
            .entry(user.to_string())
            .or_default()
            .push(text.to_owned());
    }
}
