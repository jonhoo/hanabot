extern crate rand;
extern crate slack;

use slack::{Event, Message, RtmClient};
use std::collections::{HashMap, LinkedList, VecDeque};

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
enum Color {
    Red,
    Green,
    White,
    Blue,
    Yellow,
}

use std::fmt;
impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Color::Red => write!(f, ":red_circle:"),
            Color::Green => write!(f, ":green_apple:"),
            Color::White => write!(f, ":white_medium_square:"),
            Color::Blue => write!(f, ":large_blue_diamond:"),
            Color::Yellow => write!(f, ":yellow_heart:"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Number {
    One,
    Two,
    Three,
    Four,
    Five,
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Number::One => write!(f, ":one:"),
            Number::Two => write!(f, ":two:"),
            Number::Three => write!(f, ":three:"),
            Number::Four => write!(f, ":four:"),
            Number::Five => write!(f, ":five:"),
        }
    }
}

impl Number {
    pub fn as_usize(&self) -> usize {
        match *self {
            Number::One => 1,
            Number::Two => 2,
            Number::Three => 3,
            Number::Four => 4,
            Number::Five => 5,
        }
    }
}

use std::ops::Add;
impl Add<usize> for Number {
    type Output = Number;
    fn add(self, rhs: usize) -> Self::Output {
        if rhs == 0 {
            return self;
        }
        let next = match self {
            Number::One => Number::Two,
            Number::Two => Number::Three,
            Number::Three => Number::Four,
            Number::Four => Number::Five,
            Number::Five => Number::Five,
        };
        next + (rhs - 1)
    }
}

#[derive(Clone, Copy)]
enum Clue {
    Color(Color),
    Number(Number),
}

impl fmt::Display for Clue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Clue::Color(ref c) => write!(f, "{}", c),
            Clue::Number(ref n) => write!(f, "{}", n),
        }
    }
}

enum ClueError {
    NoSuchPlayer,
    NoMatchingCards,
    NotEnoughClues,
    GameOver,
}

enum PlayError {
    NoSuchCard,
    GameOver,
}

enum DiscardError {
    NoSuchCard,
    MaxClues,
    GameOver,
}

struct Card {
    color: Color,
    number: Number,
    clues: Vec<(String, Clue)>,
}

impl fmt::Display for Card {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {}", self.color, self.number)
    }
}


struct Deck(Vec<Card>);

impl Deck {
    pub fn draw(&mut self) -> Option<Card> {
        self.0.pop()
    }
}

struct Hand {
    player: String,
    cards: LinkedList<Card>,
}

impl Hand {
    pub fn new(player: &str) -> Self {
        Hand {
            player: String::from(player),
            cards: LinkedList::default(),
        }
    }

    pub fn draw(&mut self, deck: &mut Deck) -> bool {
        deck.draw().map(|card| self.cards.push_back(card)).is_some()
    }

    pub fn clue(&mut self, player: &str, clue: Clue) -> Result<usize, ClueError> {
        let matches = self.cards
            .iter()
            .filter(|card| match clue {
                Clue::Color(ref c) => c == &card.color,
                Clue::Number(ref n) => n == &card.number,
            })
            .count();

        if matches == 0 {
            return Err(ClueError::NoMatchingCards);
        }

        for card in &mut self.cards {
            card.clues.push((player.to_owned(), clue));
        }

        Ok(matches)
    }

    pub fn remove(&mut self, card: usize) -> Option<Card> {
        if card > self.cards.len() {
            return None;
        }

        let mut after = self.cards.split_off(card);
        let card = after.pop_front();
        self.cards.append(&mut after);
        card
    }
}

impl Default for Deck {
    fn default() -> Self {
        use rand::{thread_rng, Rng};

        let colors = vec![
            Color::Red,
            Color::Green,
            Color::White,
            Color::Blue,
            Color::Yellow,
        ];
        let numbers = vec![
            Number::One,
            Number::One,
            Number::One,
            Number::Two,
            Number::Two,
            Number::Three,
            Number::Three,
            Number::Four,
            Number::Four,
            Number::Five,
        ];
        let mut cards: Vec<_> = colors
            .into_iter()
            .flat_map(|color| {
                numbers.iter().map(move |&number| {
                    Card {
                        color,
                        number,
                        clues: Vec::new(),
                    }
                })
            })
            .collect();

        thread_rng().shuffle(&mut cards[..]);
        Deck(cards)
    }
}

struct Game {
    deck: Deck,
    hands: Vec<Hand>,
    played: HashMap<Color, Number>,
    discard: Vec<Card>,
    last_move: String,
    clues: usize,
    lives: usize,
    turn: usize,

    last_turns: Option<usize>,
}

impl Game {
    pub fn new(players: &[String]) -> Self {
        let mut deck = Deck::default();
        let mut hands: Vec<_> = players
            .into_iter()
            .map(|player| Hand::new(player))
            .collect();
        let cards = match hands.len() {
            0 | 1 => unreachable!(),
            2 | 3 => 5,
            4 | 5 => 4,
            _ => unreachable!(),
        };

        for hand in &mut hands {
            for _ in 0..cards {
                let drew = hand.draw(&mut deck);
                assert!(drew);
            }
        }

        Game {
            hands,
            deck,
            played: Default::default(),
            discard: Vec::new(),
            last_move: String::new(),
            clues: 8,
            lives: 3,
            turn: 0,

            last_turns: None,
        }
    }

    pub fn current_player(&self) -> &str {
        &*self.hands[self.turn].player
    }

    pub fn clue(&mut self, player: &str, to: &str, clue: Clue) -> Result<usize, ClueError> {
        if self.clues == 0 {
            return Err(ClueError::NotEnoughClues);
        }

        let to = to.trim_left_matches("<@");
        let to = to.trim_right_matches('>');

        if player == to {
            return Err(ClueError::NoSuchPlayer);
        }

        let hands = self.hands.len();
        let hand = if let Some(h) = self.hands.iter_mut().find(|hand| &hand.player == to) {
            h
        } else {
            return Err(ClueError::NoSuchPlayer);
        };

        match hand.clue(player, clue) {
            Ok(num) => {
                self.last_move = format!(
                    "<@{}> clued <@{}> that {} {} {}",
                    player,
                    to,
                    num,
                    if num == 1 { "card is" } else { "cards are" },
                    clue
                );
                self.clues -= 1;
                self.turn = (self.turn + 1) % hands;
                if let Some(ref mut last_turns) = self.last_turns {
                    *last_turns += 1;
                    if *last_turns == hands {
                        return Err(ClueError::GameOver);
                    }
                }
                Ok(num)
            }
            e => e,
        }
    }

    pub fn play(&mut self, player: &str, card: usize) -> Result<(), PlayError> {
        let hands = self.hands.len();
        let hand = self.hands
            .iter_mut()
            .find(|hand| &hand.player == player)
            .unwrap();
        if let Some(card) = hand.remove(card) {
            if !hand.draw(&mut self.deck) && self.last_turns.is_none() {
                self.last_turns = Some(0);
            }

            use std::collections::hash_map::Entry;
            let success = match self.played.entry(card.color) {
                Entry::Vacant(e) => if card.number == Number::One {
                    e.insert(Number::One);
                    true
                } else {
                    false
                },
                Entry::Occupied(mut e) => if card.number == *e.get() + 1 {
                    e.insert(card.number);
                    true
                } else {
                    false
                },
            };

            if !success {
                self.lives -= 1;
                self.last_move = format!("<@{}> played a {} incorrectly", player, card);
                self.discard.push(card);
                if self.lives == 0 {
                    return Err(PlayError::GameOver);
                }
            } else {
                self.last_move = format!("<@{}> played a {}", player, card);
            }

            self.turn = (self.turn + 1) % hands;
            if let Some(ref mut last_turns) = self.last_turns {
                *last_turns += 1;
                if *last_turns == hands {
                    return Err(PlayError::GameOver);
                }
            }
            Ok(())
        } else {
            Err(PlayError::NoSuchCard)
        }
    }

    pub fn discard(&mut self, player: &str, card: usize) -> Result<(), DiscardError> {
        if self.clues == 8 {
            return Err(DiscardError::MaxClues);
        }

        let hands = self.hands.len();
        let hand = self.hands
            .iter_mut()
            .find(|hand| &hand.player == player)
            .unwrap();

        if let Some(card) = hand.remove(card) {
            if !hand.draw(&mut self.deck) && self.last_turns.is_none() {
                self.last_turns = Some(0);
            }
            self.last_move = format!("<@{}> discarded a {}", player, card);
            self.discard.push(card);
            self.turn = (self.turn + 1) % hands;
            if let Some(ref mut last_turns) = self.last_turns {
                *last_turns += 1;
                if *last_turns == hands {
                    return Err(DiscardError::GameOver);
                }
            }
            Ok(())
        } else {
            Err(DiscardError::NoSuchCard)
        }
    }

    fn show_known(&self, hand: usize, channel: &str, cli: &RtmClient, index: bool) {
        let hand: Vec<_> = self.hands[hand]
            .cards
            .iter()
            .enumerate()
            .map(|(i, card)| {
                let know_color = card.clues.iter().any(|&(_, clue)| match clue {
                    Clue::Color(ref c) => c == &card.color,
                    _ => false,
                });
                let know_number = card.clues.iter().any(|&(_, clue)| match clue {
                    Clue::Number(ref n) => n == &card.number,
                    _ => false,
                });

                let mut desc = match (know_color, know_number) {
                    (false, false) => format!(":rainbow: :keycap_star:"),
                    (false, true) => format!(":rainbow: {}", card.number),
                    (true, false) => format!("{} :keycap_star:", card.color),
                    (true, true) => format!("{} {}", card.color, card.number),
                };
                if index {
                    desc.push_str(&format!(" ({})", i + 1));
                }
                desc
            })
            .collect();
        let _ = cli.sender().send_message(channel, &hand.join(" | "));
    }

    fn print_game_state(&mut self, hand: usize, channel: &str, cli: &RtmClient) {
        let msg = move |m: &str| {
            let _ = cli.sender().send_message(channel, m);
        };

        let last = if self.last_turns.is_some() {
            " *last*"
        } else {
            ""
        };

        let setup = if self.turn == hand {
            format!("It's *your*{} turn", last)
        } else {
            format!("It's <@{}>'s{} turn", self.hands[self.turn].player, last)
        };

        // show some states about the general game state
        msg(&format!(
            "{}, and there are *{}* :information_source: and {} :bomb: remaining.",
            setup,
            self.clues,
            self.lives
        ));

        let stacks: Vec<_> = [
            Color::Red,
            Color::Yellow,
            Color::White,
            Color::Green,
            Color::Blue,
        ].into_iter()
            .map(|color| if let Some(top) = self.played.get(&color) {
                format!("{} {}", color, top)
            } else {
                format!("{} :zero:", color)
            })
            .collect();
        msg(&format!("Played: {}", stacks.join(" ")));

        // we want to use attachments to show other players' hands
        // but we can't yet: https://api.slack.com/bot-users#post_messages_and_react_to_users
        msg("The other players' hands are:");
        for (i, h) in self.hands.iter().enumerate() {
            if i == hand {
                continue;
            }

            let cards: Vec<_> = h.cards.iter().map(|c| format!("{}", c)).collect();
            msg(&format!("*<@{}>*: {}", h.player, cards.join(" | ")));
        }

        msg("Your hand, as far as you know, is:");
        self.show_known(hand, channel, cli, true);

        msg("When you have the time, let me know here what move you want to make next!");
    }

    pub fn show_clues(&self, channel: &str, player: &str, cli: &RtmClient) {
        let p = player.trim_left_matches("<@");
        let p = p.trim_right_matches('>');
        let p = self.hands.iter().position(|hand| &hand.player == p);

        if p.is_none() {
            let _ = cli.sender().send_message(
                channel,
                &format!("there is no player in this game named {}", player),
            );
            return;
        }

        let p = p.unwrap();
        let _ = cli.sender().send_message(
            channel,
            &format!(
                "<@{}> knows the following about their hand:",
                self.hands[p].player
            ),
        );
        self.show_known(p, channel, cli, false)
    }

    pub fn show_discards(&self, channel: &str, cli: &RtmClient) {
        if self.discard.is_empty() {
            let _ = cli.sender()
                .send_message(channel, "The discard pile is empty.");
            return;
        }

        let _ = cli.sender()
            .send_message(channel, "The discard pile contains the following cards:");
        let mut waiting = Vec::new();
        for card in &self.discard {
            waiting.push(format!("{}", card));
            if waiting.len() == 5 {
                let _ = cli.sender().send_message(channel, &waiting.join(" | "));
                waiting.clear();
            }
        }
        if !waiting.is_empty() {
            let _ = cli.sender().send_message(channel, &waiting.join(" | "));
        }
    }

    pub fn progress_game(&mut self, users: &HashMap<String, String>, cli: &RtmClient) -> bool {
        // empty line
        for hand in &self.hands {
            let m = "\n--------------------------------------------------------------------------";
            let _ = cli.sender().send_message(&users[&hand.player], m);
        }

        if !self.last_move.is_empty() {
            for hand in &self.hands {
                let mut m = self.last_move
                    .replace(&format!("<@{}>", hand.player), "you");
                if m.starts_with("you") {
                    m = m.replacen("you", "You", 1);
                }
                let _ = cli.sender().send_message(&users[&hand.player], &m);
            }
        }

        let points: usize = self.played.iter().map(|(_, num)| num.as_usize()).sum();
        let mut game_over = self.lives == 0;
        if let Some(last_turns) = self.last_turns {
            game_over = game_over || last_turns == self.hands.len();
        }
        if game_over {
            // the game has ended in a loss :'(
            for hand in &self.hands {
                let _ = cli.sender().send_message(
                    &users[&hand.player],
                    &format!(
                        "Game over :slightly_frowning_face:\n\
                         You got {}/25 points.",
                        points
                    ),
                );
            }
            return true;
        }

        if points == 25 {
            // the game has ended in a win \o/
            for hand in &self.hands {
                let _ = cli.sender().send_message(
                    &users[&hand.player],
                    "You won the game with 25/25 points :tada:",
                );
            }
            return true;
        }

        // game is not yet over -- let's print the game state
        for i in 0..self.hands.len() {
            let channel = {
                let user = &self.hands[i].player;
                &users[user]
            };
            self.print_game_state(i, channel, cli);
        }

        false
    }
}

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
    games: HashMap<usize, Game>,

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
    fn progress_game(&mut self, game_id: usize, cli: &RtmClient) {
        if self.games
            .get_mut(&game_id)
            .unwrap()
            .progress_game(&self.playing_users, cli)
        {
            // game has ended
            let game = self.games.remove(&game_id).unwrap();
            for hand in game.hands {
                self.in_game.remove(&hand.player);
                self.waiting.push_back(hand.player);
            }
            self.on_join(cli);
        }
    }

    fn handle_move(&mut self, user: &str, text: &str, cli: &RtmClient) {
        if text == "start" {
            if self.in_game.contains_key(user) {
                // game has already started, so ignore this
                return;
            }
            // the user wants to start the game even though there aren't enough players
            self.start_game(Some(user), cli);
            return;
        }

        if text == "quit" {
            let _ = cli.sender().send_message(
                &self.playing_users[user],
                "Yeeeeeahhhhh, we don't support quitting games yet...",
            );
            return;
        }

        let game_id = if let Some(game_id) = self.in_game.get(user) {
            *game_id
        } else {
            let _ = cli.sender().send_message(
                &self.playing_users[user],
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
                    let _ = cli.sender().send_message(
                        &self.playing_users[user],
                        &format!("It's not your turn yet, it's <@{}>'s.", current),
                    );
                    return;
                }
            }
        }

        match cmd {
            Some("discards") => {
                self.games[&game_id].show_discards(&self.playing_users[user], cli);
            }
            Some("clues") => {
                let player = command.next();
                if player.is_none() || command.next().is_some() {
                    let _ = cli.sender().send_message(
                        &self.playing_users[user],
                        "I believe you are mistaken. \
                         To view what a person knows about their hand, you just name a player \
                         (using @playername), and nothing else.",
                    );
                    return;
                }
                let player = player.unwrap();

                self.games[&game_id].show_clues(&self.playing_users[user], player, cli);
            }
            Some("clue") => {
                let player = command.next();
                let specifier = command.next();
                if player.is_none() || specifier.is_none() || command.next().is_some() {
                    let _ = cli.sender().send_message(
                        &self.playing_users[user],
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
                        let _ = cli.sender().send_message(
                            &self.playing_users[user],
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
                        self.progress_game(game_id, cli);
                    }
                    Err(ClueError::NoSuchPlayer) => {
                        let _ = cli.sender().send_message(
                            &self.playing_users[user],
                            "The player you specified does not exist. \
                             Remember to use Slack's @username tagging.",
                        );
                    }
                    Err(ClueError::NoMatchingCards) => {
                        let _ = cli.sender().send_message(
                            &self.playing_users[user],
                            "The card you specified is not in your hand. \
                             Remember that card indexing starts at 1.",
                        );
                    }
                    Err(ClueError::NotEnoughClues) => {
                        let _ = cli.sender().send_message(
                            &self.playing_users[user],
                            "There are no clue tokens left, so you cannot clue.",
                        );
                    }
                    Err(ClueError::GameOver) => {}
                }
            }
            Some("play") => {
                let card = command.next().and_then(|card| card.parse::<usize>().ok());
                if card.is_none() || card == Some(0) || command.next().is_some() {
                    let _ = cli.sender().send_message(
                        &self.playing_users[user],
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
                    Err(PlayError::NoSuchCard) => {
                        let _ = cli.sender().send_message(
                            &self.playing_users[user],
                            "The card you specified is not in your hand. \
                             Remember that card indexing starts at 1.",
                        );
                        return;
                    }
                    Err(PlayError::GameOver) => {}
                }
                self.progress_game(game_id, cli);
            }
            Some("discard") => {
                let card = command.next().and_then(|card| card.parse::<usize>().ok());
                if card.is_none() || card == Some(0) || command.next().is_some() {
                    let _ = cli.sender().send_message(
                        &self.playing_users[user],
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
                        self.progress_game(game_id, cli);
                    }
                    Err(DiscardError::NoSuchCard) => {
                        let _ = cli.sender().send_message(
                            &self.playing_users[user],
                            "The card you specified is not in your hand. \
                             Remember that card indexing starts at 1.",
                        );
                    }
                    Err(DiscardError::MaxClues) => {
                        let _ = cli.sender().send_message(
                            &self.playing_users[user],
                            "All 8 clue tokens are available, so discard is disallowed.",
                        );
                    }
                    Err(DiscardError::GameOver) => {}
                }
            }
            Some(cmd) => {
                let _ = cli.sender().send_message(
                    &self.playing_users[user],
                    &format!(
                        "What do you mean \"{}\"?! You must either clue, play, or discard.",
                        cmd
                    ),
                );
            }
            None => {
                let _ = cli.sender().send_message(
                    &self.playing_users[user],
                    "You must either clue, play, or discard.",
                );
            }
        }
    }

    fn start_game(&mut self, user: Option<&str>, cli: &RtmClient) {
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
                let _ = cli.sender().send_message(
                    &self.playing_users[u],
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
            let _ = cli.sender().send_message(&self.playing_users[p], &message);
        }
        for p in players {
            let already_in = self.in_game.insert(p, game_id);
            assert_eq!(already_in, None);
        }

        self.progress_game(game_id, cli);
    }

    fn on_join(&mut self, cli: &RtmClient) {
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
                     If you'd like to do so instead of waiting for five players,\
                     just send me the message `start`.",
                    self.waiting.len() - 1
                );
                for p in &self.waiting {
                    let _ = cli.sender().send_message(&self.playing_users[p], &message);
                }
                return;
            }
            5 | _ => {
                self.start_game(None, cli);
                self.on_join(cli);
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
                            .send_message(c, &format!("<@{}> please send me a DM instead", u));
                    }
                    return;
                }

                let t = t.trim_left_matches(&prefix);
                if t == "join" {
                    if c == &self.channel {
                        // we need to know the DM channel ID, so force the user to DM us
                        let _ = cli.sender()
                            .send_message(c, &format!("<@{}> please send me a DM instead", u));
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
                        self.on_join(cli);
                    }
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
                            let _ = cli.sender()
                                .send_message(c, "<@{}> you need to join before making moves!");
                            return;
                        }
                    }
                    self.handle_move(u, t, cli);
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
                    Some(ref name) => {
                        println!("{}", name);
                        name == "hanabi"
                    }
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
