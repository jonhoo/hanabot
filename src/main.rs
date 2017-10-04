extern crate rand;
extern crate slack;

use slack::{Event, Message, RtmClient};
use std::collections::{HashMap, VecDeque};

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

    pub fn flush(self, user_to_channel: &HashMap<String, String>) {
        for (user, msgs) in self.msgs {
            let _ = self.cli
                .sender()
                .send_message(&user_to_channel[&user], &msgs.join("\n"));
        }
    }
}

mod hanabi;
use hanabi::{Clue, Color, Game, Number};

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
    fn progress_game(&mut self, game_id: usize, msgs: &mut MessageProxy) {
        if self.games.get_mut(&game_id).unwrap().progress_game(msgs) {
            // game has ended
            let game = self.games.remove(&game_id).unwrap();

            println!("game #{} ended with score {}/25", game_id, game.score());

            for player in game.players() {
                self.in_game.remove(player);
                self.waiting.push_back(player.clone());
            }
            self.on_join(msgs);
        }
    }

    fn handle_move(&mut self, user: &str, text: &str, msgs: &mut MessageProxy) {
        if text == "start" {
            if self.in_game.contains_key(user) {
                // game has already started, so ignore this
                return;
            }
            // the user wants to start the game even though there aren't enough players
            self.start_game(Some(user), msgs);
            return;
        }

        if text == "quit" {
            msgs.send(user, "Yeeeeeahhhhh, we don't support quitting games yet...");
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

        let mut command = text.split_whitespace();
        let cmd = command.next();

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
            Some("discards") => {
                self.games[&game_id].show_discards(user, msgs);
            }
            Some("clues") => {
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

                self.games[&game_id].show_clues(user, player, msgs);
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

                match self.games
                    .get_mut(&game_id)
                    .unwrap()
                    .clue(user, player, clue)
                {
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
                    .play(user, card.unwrap() - 1)
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
                    .discard(user, card.unwrap() - 1)
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

    fn on_join(&mut self, msgs: &mut MessageProxy) {
        match self.waiting.len() {
            0 => {
                // technically reachable since we call on_join after starting a game
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
                self.start_game(None, msgs);
                self.on_join(msgs);
            }
        }
    }
}

impl slack::EventHandler for Hanabi {
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
                    if t == "join" {
                        // some poor user tried to join -- help them out
                        let _ = cli.sender()
                            .send_message(c, &format!("<@{}> please dm me instead", u));
                    }
                    return;
                }

                let t = t.trim_left_matches(&prefix);
                if t == "join" {
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
                        self.on_join(&mut messages);
                        messages.flush(&self.playing_users);
                    }
                } else if t == "players" {
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
                    out.push_str(&format!(
                        "\nWaiting: {}",
                        self.waiting
                            .iter()
                            .map(|p| format!("<@{}>", p))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
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

        self.me = cli.start_response()
            .slf
            .as_ref()
            .and_then(|u| u.id.clone())
            .unwrap();

        match channel.or(group) {
            None => panic!("#hanabi not found"),
            Some(channel) => {
                println!("joined channel {}", channel);
                let _ = cli.sender().send_message(
                    &channel,
                    "Hanabi bot is now available! :tada:\nSend me the message 'join' to join a game.",
                );
                self.channel = channel.clone();
            }
        }
    }
}

fn main() {
    let api_key = if let Ok(key) = std::env::var("API_KEY") {
        key
    } else {
        eprintln!("No API_KEY provided.");
        std::process::exit(1);
    };

    let mut handler = Hanabi::default();
    while let Err(e) = RtmClient::login_and_run(&api_key, &mut handler) {
        eprintln!("Error while running: {}", e);
    }
}
