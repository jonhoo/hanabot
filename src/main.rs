use eyre::Context;
use hanabi::{Clue, Color, Game, Number};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use slack_morphism::prelude::*;
use slack_morphism::{SlackApiToken, SlackApiTokenValue};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::Mutex;

mod hanabi;

// Welcome to the Hanabi bot code.
//
// It's not particularly pretty, but I hope it's still possible to digest.
// There are two main parts to the code: the stuff in this file, which interacts with users and
// keeps track of all running games, and the stuff in `hanabi.rs`, which contains all the logic
// related to Hanabi gameplay.
//
// The basic flow for most games is as follows:
//
//  - A user sends a message to the bot.
//  - `Hanabi::on_event` is called with an `Event::Message`
//  - `on_event` handles meta-moves like `join` or `players`, and otherwise calls
//    `Hanabi::handle_move`.
//  - `handle_move` parses the user's command, checks that they are allowed to do what they want to
//     do, and then invokes the appropriate method on the underlying `hanabi::Game`.
//
// All state tracking happens on the `Hanabi` struct. The most important part of it is
// `Hanabi.games`, which keeps the game state for active games. The struct also keeps the mapping
// from Slack user IDs to the dm channel ID for that user, so that we can send them private
// messages. This mapping is never exposed to `hanabi::Game`; instead, we use `MessageProxy`, which
// buffers up messages we want to send to each player, and then flushes them all to the appropriate
// channels when the turn has finished.
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

    let hanabi = if tokio::fs::try_exists("state.json")
        .await
        .context("check for state.json")?
    {
        let state_json = tokio::fs::read("state.json")
            .await
            .context("read state.json")?;
        serde_json::from_reader(&*state_json).context("parse state.json")?
    } else {
        Hanabi::default()
    };
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

// TODO
// "Hanabi bot is going to be unavailable for a little bit :slightly_frowning_face:",

// TODO
// "Hanabi bot is now available! :tada:\n\
//  Send me the message 'join' to join a game.",

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

    let Some(u) = m.sender.user else {
        return Ok(());
    };
    let Some(t) = m.content.as_ref().and_then(|v| v.text.as_ref()) else {
        return Ok(());
    };
    let Some(c) = m.origin.channel else {
        return Ok(());
    };
    if !m.origin.channel_type.is_some_and(|ct| ct.0 == "im") {
        return Ok(());
    }

    let states = states.read().await;
    let state = states
        .get_user_state::<Arc<State>>()
        .expect("we always use hanabi as user state");

    let mut hanabi = state.hanabi.lock().await;
    let cli = client.open_session(&state.api_token);

    let Some(command) = t.split_whitespace().next() else {
        // empty message
        return Ok(());
    };

    if command.starts_with("<@") {
        let _ = cli
            .chat_post_message(&SlackApiChatPostMessageRequest::new(
                c,
                SlackMessageContent::new().with_text(format!(
                    "You don't need to prefix your commands with {command} when DMing me :)",
                )),
            ))
            .await;
        return Ok(());
    }

    match &*command.to_lowercase() {
        "join" => {
            if hanabi.playing_users.insert(u.clone()) {
                cli.chat_post_message(&SlackApiChatPostMessageRequest::new(
                    c,
                    SlackMessageContent::new().with_text(String::from(
                        "\
                                 Welcome! \
                                 I'll get you started with a game \
                                 as soon as there are some other \
                                 players available.",
                    )),
                ))
                .await
                .context("notify user of join")?;
                println!("user {u} joined game");

                hanabi.waiting.push_back(u.clone());

                let mut messages = MessageProxy::new(cli);
                hanabi.on_player_change(&mut messages);
                messages.flush().await.context("handle player join")?;
                hanabi.save().await.context("save on user join")?;
            } else if hanabi.waiting.contains(&u) {
                let _ = cli
                    .chat_post_message(&SlackApiChatPostMessageRequest::new(
                        c,
                        SlackMessageContent::new().with_text(String::from(
                            "You can start a game with `start` once there are enough players available.",
                        )),
                    ))
                    .await;
            } else {
                let _ = cli
                    .chat_post_message(&SlackApiChatPostMessageRequest::new(
                        c,
                        SlackMessageContent::new().with_text(String::from(
                            "You're already playing, but I appreciate your enthusiasm.",
                        )),
                    ))
                    .await;
            }
        }
        "leave" => {
            if hanabi.playing_users.contains(&u) {
                // the user wants to leave
                let mut messages = MessageProxy::new(cli);

                // first make them quit.
                if hanabi.in_game.contains_key(&u) {
                    hanabi
                        .handle_move(&u, "quit", &mut messages)
                        .await
                        .context("handle mid-game departure")?;
                }

                // then make them not wait anymore.
                if let Some(i) = hanabi.waiting.iter().position(|p| p == &u) {
                    println!("user {u} left");
                    hanabi.waiting.remove(i);
                } else {
                    println!("user {u} wanted to leave, but not waiting?");
                }

                // let them know we removed them
                messages.send(&u.0, "I have stricken you from all my lists.");
                messages.flush().await.context("handle player departure")?;

                // then actually remove
                hanabi.playing_users.remove(&u);
                hanabi.save().await.context("save on user leave")?;
            }
        }
        "players" => {
            let mut out = format!(
                "There are currently {} games and {} players:",
                hanabi.games.len(),
                hanabi.playing_users.len()
            );
            for (game_id, game) in &hanabi.games {
                out.push_str(&format!(
                    "\n#{}: <@{}>",
                    game_id,
                    game.players().collect::<Vec<_>>().join(">, <@")
                ));
            }
            if hanabi.waiting.is_empty() {
                out.push_str("\nNo players waiting.");
            } else {
                out.push_str(&format!(
                    "\nWaiting: {}",
                    hanabi
                        .waiting
                        .iter()
                        .map(|p| format!("<@{p}>"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            cli.chat_post_message(&SlackApiChatPostMessageRequest::new(
                c,
                SlackMessageContent::new().with_text(out),
            ))
            .await
            .context("list out players and games")?;
        }
        "help" => {
            let out = if hanabi.playing_users.contains(&u) {
                "Oh, so you're confused? I'm so sorry to hear that.\n\
                 \n\
                 On your turn, you can `play`, `discard`, or `clue`. \
                 If you `play` or `discard`, you must also specify which card using \
                 the card's position from the left-hand side, starting at one. \
                 To `clue`, you give the player you are cluing (`@player`), \
                 and the clue you want to give (e.g., `red`, `one`).\n\
                 \n\
                 To look around, you can use `hands`, `deck`, or `discards`. \
                 `hands` will tell you what each player has and knows, `deck` will \
                 show you the number of cards left, and `discards` will show \
                 you the discard pile. If everything goes south, you can always use \
                 `quit` to give up.\n\
                 \n\
                 Should you no longer wish to play, write `leave`.\n\
                 \n\
                 If you want more information, try \
                 <https://github.com/jonhoo/hanabot>."
            } else {
                "Welcome to the game Hanabi!
                 \n\
                 All gameplay happens through your interactions with this bot. \n\
                 To indicate your interest in joining a game, type `join`. \n\
                 Once you've done so, you can type `help` again to get game-specific help. \n\
                 \n\
                 If you want more information, try \
                 <https://en.wikipedia.org/wiki/Hanabi_(card_game)> or \
                 <https://github.com/jonhoo/hanabot>."
            };
            cli.chat_post_message(
                &SlackApiChatPostMessageRequest::new(
                    SlackChannelId(u.to_string()),
                    SlackMessageContent::new().with_text(String::from(out)),
                )
                .without_unfurl_links(),
            )
            .await
            .context("send help message")?;
        }
        _ => {
            if hanabi.in_game.contains_key(&u) {
                // known user made a move in a game
            } else if hanabi.playing_users.contains(&u) {
                // known user made a move, but isn't in a game
                let _ = cli
                    .chat_post_message(&SlackApiChatPostMessageRequest::new(
                        c,
                        SlackMessageContent::new().with_text(String::from(
                            "You're not in a game at the moment, so can't make a move.",
                        )),
                    ))
                    .await;
            } else {
                // unknown user made move that wasn't `join`
                // let's tell them
                let out = "I have no idea what you mean. Try `help` :)";
                cli.chat_post_message(&SlackApiChatPostMessageRequest::new(
                    SlackChannelId(u.to_string()),
                    SlackMessageContent::new().with_text(String::from(out)),
                ))
                .await
                .context("send message to confused user")?;
                return Ok(());
            }

            let mut messages = MessageProxy::new(cli);
            hanabi
                .handle_move(&u, t, &mut messages)
                .await
                .with_context(|| format!("handle move '{t}'"))?;
            messages.flush().await.context("handle game play message")?;
        }
    }

    Ok(())
}

/// `MessageProxy` buffers messages that are to be sent to a user in a given turn, and flushes them
/// in a single private message to each user when the turn has completed. This avoids sending lots
/// of notifications to each user, and hides Slack API details such as the distinction between user
/// ids and channel ids from `hanabi::Game`.
pub(crate) struct MessageProxy<'a> {
    cli: SlackClientSession<'a, SlackClientHyperHttpsConnector>,
    msgs: HashMap<String, Vec<String>>,
}

impl<'a> MessageProxy<'a> {
    pub fn new(cli: SlackClientSession<'a, SlackClientHyperHttpsConnector>) -> Self {
        MessageProxy {
            cli,
            msgs: Default::default(),
        }
    }

    pub fn send(&mut self, user: &str, text: &str) {
        self.msgs
            .entry(user.to_string())
            .or_default()
            .push(text.to_owned());
    }

    pub async fn flush(&mut self) -> eyre::Result<()> {
        for (user, msgs) in self.msgs.drain() {
            let _ = self
                .cli
                .chat_post_message(&SlackApiChatPostMessageRequest::new(
                    SlackChannelId(user.clone()),
                    SlackMessageContent::new().with_text(msgs.join("\n")),
                ))
                .await
                .with_context(|| format!("send to {user}"))?;
        }

        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Hanabi {
    /// id of the bot's user
    me: String,

    /// main game channel id
    channel: String,

    /// users who have joined
    playing_users: HashSet<SlackUserId>,

    /// users waiting for a game
    waiting: VecDeque<SlackUserId>,

    /// total number of games
    ngames: usize,

    /// currently running games, indexed by game number
    games: HashMap<usize, hanabi::Game>,

    /// map from each user to the game they are in
    in_game: HashMap<SlackUserId, usize>,
}

impl Default for Hanabi {
    fn default() -> Self {
        Hanabi {
            me: String::new(),
            channel: String::new(),

            playing_users: Default::default(),
            waiting: Default::default(),

            ngames: 0,
            games: Default::default(),
            in_game: Default::default(),
        }
    }
}

impl Hanabi {
    pub async fn save(&self) -> eyre::Result<()> {
        let state = serde_json::to_vec(self).context("serialize Hanabi state")?;
        tokio::fs::write("state.json", &state)
            .await
            .context("write out Hanabi state to state.json")?;
        Ok(())
    }

    /// Determine whether we can start a new game, and notify players if they can force a new game
    /// to start. Should be called when the number of waiting players has changed.
    fn on_player_change(&mut self, msgs: &mut MessageProxy) {
        match self.waiting.len() {
            0 => {
                // technically reachable since we call on_player_change after starting a game
            }
            1 => {
                // can't start a game yet
                return;
            }
            _ => {
                // *could* start a game if the users are ready
                let message = format!(
                    "I have {} other available players, so we can start a game.\n\
                     Use `start` to do so. \
                     You can optionally pass the number of players to include.",
                    self.waiting.len() - 1
                );
                for p in &self.waiting {
                    msgs.send(&p.0, &message);
                }
                return;
            }
        }
    }

    /// Start a new game.
    ///
    /// If `user` is not `None`, then `user` tried to force a game to start despite there not being
    /// a full five waiting players. If this is the case, `user` should certainly be included in
    /// the new game (assuming there are at least two free players).
    async fn start_game(
        &mut self,
        user: Option<&SlackUserId>,
        users: Option<usize>,
        msgs: &mut MessageProxy<'_>,
    ) -> eyre::Result<()> {
        let mut players = Vec::new();

        if let Some(u) = user {
            // a specific user requested the game to start immediately
            // make sure that they are included
            if let Some(u) = self.waiting.iter().position(|user| user == u) {
                let mut following = self.waiting.split_off(u);
                players.push(following.pop_front().unwrap());
                self.waiting.append(&mut following);
            } else {
                // that user isn't waiting, so do nothing
                return Ok(());
            }
        }

        let users = users.unwrap_or(5);
        while players.len() < users && players.len() <= 5 {
            if let Some(u) = self.waiting.pop_front() {
                players.push(u);
            } else {
                break;
            }
        }

        if players.len() < 2 {
            // no game -- not enough players
            if let Some(u) = user {
                msgs.send(
                    &u.0,
                    "Unfortunately, there aren't enough players to start a game yet.",
                );
            }
            self.waiting.extend(players);
            return Ok(());
        }

        let game = Game::new(players.iter().map(|slack_user| &*slack_user.0));
        let game_id = self.ngames;
        self.ngames += 1;
        self.games.insert(game_id, game);

        println!(
            "starting game #{} with {} users: {:?}",
            game_id,
            players.len(),
            players
        );

        for p in &players {
            let others: Vec<_> = players
                .iter()
                .filter(|&player| player != p)
                .map(|player| format!("<@{}>", player))
                .collect();
            let message = format!(
                "You are now in a game with {} other players: {}",
                players.len() - 1,
                others.join(", ")
            );
            msgs.send(&p.0, &message);
        }
        for p in players {
            let already_in = self.in_game.insert(p, game_id);
            assert_eq!(already_in, None);
        }

        self.progress_game(game_id, msgs)
            .await
            .context("progress game")?;
        Ok(())
    }

    /// Handle a turn command by the given `user`.
    async fn handle_move(
        &mut self,
        user: &SlackUserId,
        text: &str,
        msgs: &mut MessageProxy<'_>,
    ) -> eyre::Result<()> {
        let mut command = text.split_whitespace().peekable();

        if command
            .peek()
            .map(|s| s.to_lowercase() == "start")
            .unwrap_or(false)
        {
            if self.in_game.contains_key(user) {
                // game has already started, so ignore this
                return Ok(());
            }

            let _ = command.next().is_some();
            let nplayers = command.peek().and_then(|n| n.parse().ok());

            if command.next().is_some() && nplayers.is_none() {
                msgs.send(
                    &user.0,
                    "You can only give an integral number of players to start a game with",
                );
                return Ok(());
            }

            // the user wants to start the game even though there aren't enough players
            self.start_game(Some(user), nplayers, msgs)
                .await
                .context("start game")?;
            return Ok(());
        }

        let game_id = if let Some(game_id) = self.in_game.get(user) {
            *game_id
        } else {
            msgs.send(
                &user.0,
                "You're not currently in any games, and thus can't make a move.",
            );
            return Ok(());
        };

        let cmd = match command.peek() {
            Some(cmd) if cmd.starts_with("<@") && cmd.ends_with(">") => Some("clue"),
            _ => None,
        };
        let cmd = cmd.or_else(|| command.next());
        let cmd = cmd.map(|cmd| cmd.to_lowercase());
        let cmd = cmd.as_ref().map(|cmd| cmd.as_str());

        if let Some(cmd) = cmd {
            if cmd == "play" || cmd == "clue" || cmd == "discard" {
                let current = self.games[&game_id].current_player();
                if current != user.0 {
                    msgs.send(
                        &user.0,
                        &format!("It's not your turn yet, it's <@{}>'s.", current),
                    );
                    return Ok(());
                }
            }
        }

        match cmd {
            Some("quit") => {
                let score = self.games[&game_id].score();
                for player in self.games[&game_id].players() {
                    msgs.send(
                        player,
                        &format!(
                            "The game was ended prematurely by <@{}> with a score of {}/25",
                            user, score
                        ),
                    );
                }
                self.end_game(game_id, msgs);
            }
            Some("ping") => {
                let current = self.games[&game_id].current_player();
                if current == user.0 {
                    msgs.send(
                        &user.0,
                        "It's your turn... No need to bother the other players.",
                    );
                } else {
                    msgs.send(
                        current,
                        &format!("<@{}> pinged you -- it's your turn.", user),
                    );
                    msgs.send(&user.0, &format!("I've pinged <@{}>.", current));
                }
            }
            Some("discards") => {
                self.games[&game_id].show_discards(&user.0, msgs);
            }
            Some("hands") => {
                self.games[&game_id].show_hands(&user.0, false, msgs);
            }
            Some("deck") => {
                self.games[&game_id].show_deck(&user.0, msgs);
            }
            Some("clue") => {
                let player = command.next();
                let specifier = command.next();
                if player.is_none() || specifier.is_none() || command.next().is_some() {
                    msgs.send(
                        &user.0,
                        "I don't have a clue what you mean. \
                         To clue, you give a player (using @playername), \
                         a card specifier (e.g., \"red\" or \"one\"), \
                         and nothing else.",
                    );
                    return Ok(());
                }
                let player = player.unwrap();
                let specifier = specifier.unwrap();

                let clue = match specifier {
                    "red" => Clue::Color(Color::Red),
                    "green" => Clue::Color(Color::Green),
                    "white" => Clue::Color(Color::White),
                    "blue" => Clue::Color(Color::Blue),
                    "yellow" => Clue::Color(Color::Yellow),
                    "one" | "1" => Clue::Number(Number::One),
                    "two" | "2" => Clue::Number(Number::Two),
                    "three" | "3" => Clue::Number(Number::Three),
                    "four" | "4" => Clue::Number(Number::Four),
                    "five" | "5" => Clue::Number(Number::Five),
                    s => {
                        msgs.send(
                            &user.0,
                            &format!("You're making no sense. A card can't be {}...", s),
                        );
                        return Ok(());
                    }
                };

                let player = player.trim_start_matches("<@");
                let player = player.trim_end_matches('>');

                match self.games.get_mut(&game_id).unwrap().clue(player, clue) {
                    Ok(_) => {}
                    Err(hanabi::ClueError::NoSuchPlayer) => {
                        msgs.send(
                            &user.0,
                            "The player you specified does not exist. \
                             Remember to use Slack's @username tagging.",
                        );
                        return Ok(());
                    }
                    Err(hanabi::ClueError::NoMatchingCards) => {
                        msgs.send(
                            &user.0,
                            "The card you specified is not in your hand. \
                             Remember that card indexing starts at 1.",
                        );
                        return Ok(());
                    }
                    Err(hanabi::ClueError::NotEnoughClues) => {
                        msgs.send(
                            &user.0,
                            "There are no clue tokens left, so you cannot clue.",
                        );
                        return Ok(());
                    }
                    Err(hanabi::ClueError::GameOver) => {}
                }
                self.progress_game(game_id, msgs)
                    .await
                    .context("progress game after clue")?;
            }
            Some("play") => {
                let card = command.next().and_then(|card| card.parse::<usize>().ok());
                if card.is_none() || card == Some(0) || command.next().is_some() {
                    msgs.send(
                        &user.0,
                        "I think you played incorrectly there. \
                         To play, you just specify which card you'd like to play by specifying \
                         its index from the left side of your hand (starting at 1).",
                    );
                    return Ok(());
                }

                match self
                    .games
                    .get_mut(&game_id)
                    .unwrap()
                    .play(card.unwrap() - 1)
                {
                    Ok(()) => {}
                    Err(hanabi::PlayError::NoSuchCard) => {
                        msgs.send(
                            &user.0,
                            "The card you specified is not in your hand. \
                             Remember that card indexing starts at 1.",
                        );
                        return Ok(());
                    }
                    Err(hanabi::PlayError::GameOver) => {}
                }
                self.progress_game(game_id, msgs)
                    .await
                    .context("progress game after play")?;
            }
            Some("discard") => {
                let card = command.next().and_then(|card| card.parse::<usize>().ok());
                if card.is_none() || card == Some(0) || command.next().is_some() {
                    msgs.send(
                        &user.0,
                        "I'm going to discard that move. \
                         To discard, you must specify which card you'd like to play by specifying \
                         its index from the left side of your hand (starting at 1).",
                    );
                    return Ok(());
                }

                match self
                    .games
                    .get_mut(&game_id)
                    .unwrap()
                    .discard(card.unwrap() - 1)
                {
                    Ok(()) => {}
                    Err(hanabi::DiscardError::NoSuchCard) => {
                        msgs.send(
                            &user.0,
                            "The card you specified is not in your hand. \
                             Remember that card indexing starts at 1.",
                        );
                        return Ok(());
                    }
                    Err(hanabi::DiscardError::MaxClues) => {
                        msgs.send(
                            &user.0,
                            "All 8 clue tokens are available, so discard is disallowed.",
                        );
                        return Ok(());
                    }
                    Err(hanabi::DiscardError::GameOver) => {}
                }
                self.progress_game(game_id, msgs)
                    .await
                    .context("progress game after discard")?;
            }
            Some(cmd) => {
                msgs.send(
                    &user.0,
                    &format!(
                        "What do you mean \"{}\"?! You must either clue, play, or discard.",
                        cmd
                    ),
                );
            }
            None => {
                msgs.send(&user.0, "You must either clue, play, or discard.");
            }
        }

        Ok(())
    }

    /// Called to progress the state of a game after a turn has been taken.
    ///
    /// This also detects if the game has ended, and if it has, returns the players of that game to
    /// the pool of waiting players.
    async fn progress_game(
        &mut self,
        game_id: usize,
        msgs: &mut MessageProxy<'_>,
    ) -> eyre::Result<()> {
        let game = self.games.get_mut(&game_id).unwrap();
        if game.progress_game(msgs) {
            msgs.flush()
                .await
                .context("send out game progress messages")?;
            self.end_game(game_id, msgs);
        } else if game.became_unwinnable() {
            // last move caused game to be unwinnable -- call someone out
            let game = self.games.get(&game_id).unwrap();
            for p in game.players() {
                msgs.send(
                    p,
                    &format!(
                        "{} became unwinnable after {}",
                        self.desc_game(game_id),
                        game.last_move()
                    ),
                );
            }
        }

        self.save().await
    }

    fn desc_game(&self, game_id: usize) -> String {
        let game = &self.games[&game_id];
        let mut players: Vec<_> = game.players().map(|p| format!("<@{}>", p)).collect();
        players.pop();
        let mut players = players.join(", ");
        players.push_str(
            &game
                .players()
                .last()
                .map(|p| format!(", and <@{}>", p))
                .unwrap(),
        );

        format!("Game with {}", players)
    }

    /// Called to end a game.
    fn end_game(&mut self, game_id: usize, msgs: &mut MessageProxy) {
        // game has ended
        let desc = self.desc_game(game_id);
        let game = self.games.remove(&game_id).unwrap();

        println!("game #{} ended with score {}/25", game_id, game.score());
        for p in game.players() {
            msgs.send(
                p,
                &format!(
                    "{} ended with a score of {}/25 {}",
                    desc,
                    game.score(),
                    game.score_smiley()
                ),
            );
        }

        let mut players: Vec<_> = game.players().map(|s| SlackUserId(s.to_string())).collect();

        // shuffle players so we don't add them back to the queue in the same order they were in
        // when we started the game. if we don't do this, games would always have basically the
        // same player order (though `start` player does go first).
        players.shuffle(&mut rand::rng());
        for player in players {
            self.in_game.remove(&player);
            self.waiting.push_back(player);
        }
        self.on_player_change(msgs);
    }
}
