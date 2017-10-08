extern crate ctrlc;
extern crate rand;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate slack;

use slack::{Event, Message, RtmClient};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::fs::File;
use std::io::{BufReader, BufWriter};

mod hanabi;
use hanabi::{Clue, Color, Game, Number};

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
fn main() {
    let api_key = if let Ok(key) = std::env::var("API_KEY") {
        key
    } else {
        eprintln!("No API_KEY provided.");
        std::process::exit(1);
    };

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    let handler: Hanabi = File::open("state.json")
        .map_err(|_| ())
        .and_then(|f| {
            serde_json::from_reader(BufReader::new(f)).map_err(|e| {
                eprintln!("Found past state, but failed to parse it: {}", e);
                ()
            })
        })
        .unwrap_or_default();

    let mut r = Runner {
        state: handler,
        running: running.clone(),
    };

    while let Err(e) = RtmClient::login_and_run(&api_key, &mut r) {
        if !running.load(Ordering::SeqCst) {
            // most likely, we'll get the error:
            // slack::Error::WebSocket(tungstenite::error::Error::Io(e))
            // where e.kind() == ErrorKind::Interrupted
            // but that's annoying to match against, so we always print it:
            eprintln!("Error when exiting: {}", e);
            break;
        }

        eprintln!("Error while running: {}", e);
    }

    // we're exiting; serialize state so we can later resume
    match File::create("state.json") {
        Ok(f) => if let Err(e) = serde_json::to_writer(BufWriter::new(f), &r.state) {
            eprintln!("Failed to save game state: {}", e);
        },
        Err(e) => {
            eprintln!("Failed to save game state: {}", e);
        }
    }
}

struct Runner {
    state: Hanabi,
    running: Arc<AtomicBool>,
}

impl slack::EventHandler for Runner {
    fn on_connect(&mut self, cli: &RtmClient) {
        self.state.on_connect(cli)
    }

    fn on_event(&mut self, cli: &RtmClient, event: Event) {
        self.state.on_event(cli, event);

        if !self.running.load(Ordering::SeqCst) {
            // we're unfortunately rarely get here because the Ctrl-C will cause the outer run to
            // return an error...
            let _ = cli.sender().send_message(
                &self.state.channel,
                "Hanabi bot is going to be unavailable for a little bit :slightly_frowning_face:",
            );
            let _ = cli.sender().shutdown();
        }
    }

    fn on_close(&mut self, cli: &RtmClient) {
        self.state.on_close(cli);
    }
}

impl slack::EventHandler for Hanabi {
    fn on_connect(&mut self, cli: &RtmClient) {
        // join the #hanabi channel
        let channel = cli.start_response()
            .channels
            .as_ref()
            .and_then(|channels| {
                channels.iter().find(|chan| match chan.name {
                    None => false,
                    Some(ref name) => name == "hanabi",
                })
            })
            .and_then(|chan| chan.id.as_ref());
        let group = cli.start_response()
            .groups
            .as_ref()
            .and_then(|groups| {
                groups.iter().find(|group| match group.name {
                    None => false,
                    Some(ref name) => name == "hanabi",
                })
            })
            .and_then(|group| group.id.as_ref());

        // we want to know our own id, so we know when we're mentioned by name
        self.me = cli.start_response()
            .slf
            .as_ref()
            .and_then(|u| u.id.clone())
            .unwrap();

        match channel.or(group) {
            None => panic!("#hanabi not found"),
            Some(channel) => {
                println!("joined channel {}", channel);
                if self.ngames == 0 && self.waiting.is_empty() {
                    // we must be starting for the first time
                    let _ = cli.sender().send_message(
                        &channel,
                        "Hanabi bot is now available! :tada:\n\
                         Send me the message 'join' to join a game.",
                    );
                } else {
                    let _ = cli.sender()
                        .send_message(&channel, "Hanabi bot was restarted -- everything is fine.");
                }
                self.channel = channel.clone();
            }
        }
    }

    fn on_event(&mut self, cli: &RtmClient, event: Event) {
        //println!("on_event(event: {:?})", event);
        match event {
            Event::Message(m) => if let Message::Standard(m) = *m {
                if m.user.is_none() || m.text.is_none() || m.channel.is_none() {
                    return;
                }

                let u = m.user.as_ref().unwrap();
                let t = m.text.as_ref().unwrap();
                let c = m.channel.as_ref().unwrap();

                let prefix = format!("<@{}> ", self.me);

                // first and foremost, if this message isn't for us, ignore it
                if c == &self.channel && !t.starts_with(&prefix) {
                    if t.to_lowercase() == "join" {
                        // some poor user tried to join -- help them out
                        let _ = cli.sender()
                            .send_message(c, &format!("<@{}> please dm me instead", u));
                    }
                    return;
                }

                let t = t.trim_left_matches(&prefix);
                if t.to_lowercase() == "join" {
                    if c == &self.channel {
                        // we need to know the DM channel ID, so force the user to DM us
                        let _ = cli.sender()
                            .send_message(c, &format!("<@{}> please dm me instead", u));
                    } else if self.playing_users.insert(u.clone(), c.clone()).is_none() {
                        let _ = cli.sender().send_message(
                            c,
                            "\
                             Welcome! \
                             I'll get you started with a game \
                             as soon as there are some other \
                             players available.",
                        );
                        println!("user {} joined game with channel {}", u, c);

                        self.waiting.push_back(u.clone());

                        let mut messages = MessageProxy::new(cli);
                        self.on_player_change(&mut messages);
                        messages.flush(&self.playing_users);
                    }
                } else if t.to_lowercase() == "leave" {
                    if self.playing_users.contains_key(u) {
                        // the user wants to leave
                        let mut messages = MessageProxy::new(cli);

                        // first make them quit.
                        if self.in_game.contains_key(u) {
                            self.handle_move(u, "quit", &mut messages);
                        }

                        // then make them not wait anymore.
                        if let Some(i) = self.waiting.iter().position(|p| p == u) {
                            println!("user {} left", u);
                            self.waiting.remove(i);
                        } else {
                            println!("user {} wanted to leave, but not waiting?", u);
                        }

                        // let them know we removed them
                        messages.send(u, "I have stricken you from all my lists.");
                        messages.flush(&self.playing_users);

                        // then actually remove
                        self.playing_users.remove(u);
                    }
                } else if t.to_lowercase() == "players" {
                    let mut out = format!(
                        "There are currently {} games and {} players:",
                        self.games.len(),
                        self.playing_users.len()
                    );
                    for (game_id, game) in &self.games {
                        out.push_str(&format!(
                            "\n#{}: <@{}>",
                            game_id,
                            game.players()
                                .map(|p| &**p)
                                .collect::<Vec<_>>()
                                .join(">, <@")
                        ));
                    }
                    if self.waiting.is_empty() {
                        out.push_str("\nNo players waiting.");
                    } else {
                        out.push_str(&format!(
                            "\nWaiting: {}",
                            self.waiting
                                .iter()
                                .map(|p| format!("<@{}>", p))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    let _ = cli.sender().send_message(c, &out);
                } else if t == "help" {
                    let out = "\
                               Oh, so you're confused? I'm so sorry to hear that.\n\
                               \n\
                               On your turn, you can `play`, `discard`, or `clue`. \
                               If you `play` or `discard`, you must also specify which card using \
                               the card's position from the left-hand side, starting at one. \
                               To `clue`, you give the player you are cluing (`@player`), \
                               and the clue you want to give (e.g., `red`, `one`).\n\
                               \n\
                               To look around, you can use `hands` or `discards`, or you can use \
                               `hand @player` to see what a particular player knows. \
                               If everything goes south, you can always use `quit` to give up.\n\
                               \n\
                               If you want more information, try \
                               https://github.com/jonhoo/hanabot.";
                    let _ = cli.sender().send_message(c, &out);
                } else {
                    match self.playing_users.get(u) {
                        Some(uc) if c == uc => {
                            // known user made a move in DM
                        }
                        Some(_) if c == &self.channel => {
                            // known user made move in public -- fine...
                        }
                        Some(uc) => {
                            // known user made a move in unknown channel?
                            println!("user {} made move in {}, but messages are in {}", u, c, uc);
                            return;
                        }
                        None => {
                            // unknown user made move that wasn't `join`
                            return;
                        }
                    }

                    let mut messages = MessageProxy::new(cli);
                    self.handle_move(u, t, &mut messages);
                    messages.flush(&self.playing_users);
                }
            },
            _ => {}
        }
    }

    fn on_close(&mut self, _: &RtmClient) {}
}

/// `MessageProxy` buffers messages that are to be sent to a user in a given turn, and flushes them
/// in a single private message to each user when the turn has completed. This avoids sending lots
/// of notifications to each user, and hides Slack API details such as the distinction between user
/// ids and channel ids from `hanabi::Game`.
pub(crate) struct MessageProxy<'a> {
    cli: &'a RtmClient,
    msgs: HashMap<String, Vec<String>>,
}

impl<'a> MessageProxy<'a> {
    pub fn new(cli: &'a RtmClient) -> Self {
        MessageProxy {
            cli: cli,
            msgs: Default::default(),
        }
    }

    pub fn send(&mut self, user: &str, text: &str) {
        self.msgs
            .entry(user.to_owned())
            .or_insert_with(Vec::new)
            .push(text.to_owned());
    }

    pub fn flush(&mut self, user_to_channel: &HashMap<String, String>) {
        for (user, msgs) in self.msgs.drain() {
            let _ = self.cli
                .sender()
                .send_message(&user_to_channel[&user], &msgs.join("\n"));
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Hanabi {
    /// id of the bot's user
    me: String,

    /// main game channel id
    channel: String,

    /// users who have joined, and their corresponding dm channel id
    playing_users: HashMap<String, String>,

    /// users waiting for a game
    waiting: VecDeque<String>,

    /// total number of games
    ngames: usize,

    /// currently running games, indexed by game number
    games: HashMap<usize, hanabi::Game>,

    /// map from each user to the game they are in
    in_game: HashMap<String, usize>,
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
            2 | 3 | 4 => {
                // *could* start a game if the users don't want to wait
                let message = format!(
                    "I have {} other available players, so we *could* start a game.\n\
                     If you'd like to do so instead of waiting for five players, \
                     just send me the message `start`.",
                    self.waiting.len() - 1
                );
                for p in &self.waiting {
                    msgs.send(p, &message);
                }
                return;
            }
            5 | _ => {
                // start a new game with 5 players
                self.start_game(None, msgs);
                // and also check if we can start a second game
                self.on_player_change(msgs);
            }
        }
    }

    /// Start a new game.
    ///
    /// If `user` is not `None`, then `user` tried to force a game to start despite there not being
    /// a full five waiting players. If this is the case, `user` should certainly be included in
    /// the new game (assuming there are at least two free players).
    fn start_game(&mut self, user: Option<&str>, msgs: &mut MessageProxy) {
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
                return;
            }
        }

        while players.len() < 5 {
            if let Some(u) = self.waiting.pop_front() {
                players.push(u);
            } else {
                break;
            }
        }
        assert!(players.len() <= 5);

        if players.len() < 2 {
            // no game -- not enough players
            if let Some(u) = user {
                msgs.send(
                    u,
                    "Unfortunately, there aren't enough players to start a game yet.",
                );
            }
            self.waiting.extend(players);
            return;
        }

        let game = Game::new(&players[..]);
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
            msgs.send(p, &message);
        }
        for p in players {
            let already_in = self.in_game.insert(p, game_id);
            assert_eq!(already_in, None);
        }

        self.progress_game(game_id, msgs);
    }

    /// Handle a turn command by the given `user`.
    fn handle_move(&mut self, user: &str, text: &str, msgs: &mut MessageProxy) {
        if text.to_lowercase() == "start" {
            if self.in_game.contains_key(user) {
                // game has already started, so ignore this
                return;
            }
            // the user wants to start the game even though there aren't enough players
            self.start_game(Some(user), msgs);
            return;
        }

        let game_id = if let Some(game_id) = self.in_game.get(user) {
            *game_id
        } else {
            msgs.send(
                user,
                "You're not currently in any games, and thus can't make a move.",
            );
            return;
        };

        let mut command = text.split_whitespace().peekable();

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
                if current != user {
                    msgs.send(
                        user,
                        &format!("It's not your turn yet, it's <@{}>'s.", current),
                    );
                    return;
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
                            user,
                            score
                        ),
                    );
                }
                self.end_game(game_id, msgs);
            }
            Some("ping") => {
                let current = self.games[&game_id].current_player();
                if current == user {
                    msgs.send(
                        user,
                        "It's your turn... No need to bother the other players.",
                    );
                } else {
                    msgs.send(
                        current,
                        &format!("<@{}> pinged you -- it's your turn.", user),
                    );
                }
            }
            Some("discards") => {
                self.games[&game_id].show_discards(user, msgs);
            }
            Some("hands") => {
                self.games[&game_id].show_hands(user, msgs);
            }
            Some("hand") => {
                let player = command.next();
                if player.is_none() || command.next().is_some() {
                    msgs.send(
                        user,
                        "I believe you are mistaken. \
                         To view what a person knows about their hand, you just name a player \
                         (using @playername), and nothing else.",
                    );
                    return;
                }
                let player = player.unwrap();
                let player = player.trim_left_matches("<@");
                let player = player.trim_right_matches('>');

                self.games[&game_id].show_hand(user, player, msgs);
            }
            Some("clue") => {
                let player = command.next();
                let specifier = command.next();
                if player.is_none() || specifier.is_none() || command.next().is_some() {
                    msgs.send(
                        user,
                        "I don't have a clue what you mean. \
                         To clue, you give a player (using @playername), \
                         a card specifier (e.g., \"red\" or \"one\"), \
                         and nothing else.",
                    );
                    return;
                }
                let player = player.unwrap();
                let specifier = specifier.unwrap();

                let clue = match specifier {
                    "red" => Clue::Color(Color::Red),
                    "green" => Clue::Color(Color::Green),
                    "white" => Clue::Color(Color::White),
                    "blue" => Clue::Color(Color::Blue),
                    "yellow" => Clue::Color(Color::Yellow),
                    "one" => Clue::Number(Number::One),
                    "two" => Clue::Number(Number::Two),
                    "three" => Clue::Number(Number::Three),
                    "four" => Clue::Number(Number::Four),
                    "five" => Clue::Number(Number::Five),
                    s => {
                        msgs.send(
                            user,
                            &format!("You're making no sense. A card can't be {}...", s),
                        );
                        return;
                    }
                };

                let player = player.trim_left_matches("<@");
                let player = player.trim_right_matches('>');

                match self.games.get_mut(&game_id).unwrap().clue(player, clue) {
                    Ok(_) => {
                        self.progress_game(game_id, msgs);
                    }
                    Err(hanabi::ClueError::NoSuchPlayer) => {
                        msgs.send(
                            user,
                            "The player you specified does not exist. \
                             Remember to use Slack's @username tagging.",
                        );
                    }
                    Err(hanabi::ClueError::NoMatchingCards) => {
                        msgs.send(
                            user,
                            "The card you specified is not in your hand. \
                             Remember that card indexing starts at 1.",
                        );
                    }
                    Err(hanabi::ClueError::NotEnoughClues) => {
                        msgs.send(user, "There are no clue tokens left, so you cannot clue.");
                    }
                    Err(hanabi::ClueError::GameOver) => {}
                }
            }
            Some("play") => {
                let card = command.next().and_then(|card| card.parse::<usize>().ok());
                if card.is_none() || card == Some(0) || command.next().is_some() {
                    msgs.send(
                        user,
                        "I think you played incorrectly there. \
                         To play, you just specify which card you'd like to play by specifying \
                         its index from the left side of your hand (starting at 1).",
                    );
                    return;
                }

                match self.games
                    .get_mut(&game_id)
                    .unwrap()
                    .play(card.unwrap() - 1)
                {
                    Ok(()) => {}
                    Err(hanabi::PlayError::NoSuchCard) => {
                        msgs.send(
                            user,
                            "The card you specified is not in your hand. \
                             Remember that card indexing starts at 1.",
                        );
                        return;
                    }
                    Err(hanabi::PlayError::GameOver) => {}
                }
                self.progress_game(game_id, msgs);
            }
            Some("discard") => {
                let card = command.next().and_then(|card| card.parse::<usize>().ok());
                if card.is_none() || card == Some(0) || command.next().is_some() {
                    msgs.send(
                        user,
                        "I'm going to discard that move. \
                         To discard, you must specify which card you'd like to play by specifying \
                         its index from the left side of your hand (starting at 1).",
                    );
                    return;
                }

                match self.games
                    .get_mut(&game_id)
                    .unwrap()
                    .discard(card.unwrap() - 1)
                {
                    Ok(()) => {
                        self.progress_game(game_id, msgs);
                    }
                    Err(hanabi::DiscardError::NoSuchCard) => {
                        msgs.send(
                            user,
                            "The card you specified is not in your hand. \
                             Remember that card indexing starts at 1.",
                        );
                    }
                    Err(hanabi::DiscardError::MaxClues) => {
                        msgs.send(
                            user,
                            "All 8 clue tokens are available, so discard is disallowed.",
                        );
                    }
                    Err(hanabi::DiscardError::GameOver) => {}
                }
            }
            Some(cmd) => {
                msgs.send(
                    user,
                    &format!(
                        "What do you mean \"{}\"?! You must either clue, play, or discard.",
                        cmd
                    ),
                );
            }
            None => {
                msgs.send(user, "You must either clue, play, or discard.");
            }
        }
    }

    /// Called to progress the state of a game after a turn has been taken.
    ///
    /// This also detects if the game has ended, and if it has, returns the players of that game to
    /// the pool of waiting players.
    fn progress_game(&mut self, game_id: usize, msgs: &mut MessageProxy) {
        if self.games.get_mut(&game_id).unwrap().progress_game(msgs) {
            msgs.flush(&self.playing_users);
            self.end_game(game_id, msgs);
        }
    }

    /// Called to end a game.
    fn end_game(&mut self, game_id: usize, msgs: &mut MessageProxy) {
        // game has ended
        let game = self.games.remove(&game_id).unwrap();

        println!("game #{} ended with score {}/25", game_id, game.score());

        for player in game.players() {
            self.in_game.remove(player);
            self.waiting.push_back(player.clone());
        }
        self.on_player_change(msgs);
    }
}
