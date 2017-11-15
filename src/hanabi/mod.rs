use std::collections::HashMap;
use std::time::{Duration, SystemTime, SystemTimeError};

mod components;
use self::components::{Card, Deck, Hand};
pub(crate) use self::components::{ClueError, DiscardError, PlayError};
pub(crate) use self::components::{Clue, Color, Number};

/// We want to ensure that we always print colors in the same order.
const COLOR_ORDER: [Color; 5] = [
    Color::Red,
    Color::Green,
    Color::White,
    Color::Blue,
    Color::Yellow,
];

/// Pretty-print a duration.
fn dur(t: Result<Duration, SystemTimeError>) -> String {
    if t.is_err() {
        return "a while".to_owned();
    }
    let t = t.unwrap().as_secs();

    if t > 24 * 60 * 60 {
        format!("{} days", t / (24 * 60 * 60))
    } else if t > 60 * 60 {
        format!("{} hours", t / (60 * 60))
    } else if t > 60 {
        format!("{} minutes", t / 60)
    } else {
        format!("{} seconds", t)
    }
}

/// Pretty-print and restart last move time.
fn dur_mod(start: &mut SystemTime) -> String {
    let t = start.elapsed();
    *start = SystemTime::now();
    dur(t)
}


#[derive(Serialize, Deserialize)]
struct Move {
    player: usize,
    for_public: String,
    for_others: String,
}

impl Move {
    pub fn new(player: usize, for_public: String, for_others: String) -> Move {
        Move {
            player,
            for_public,
            for_others,
        }
    }

    pub fn show_to(&self, player: usize) -> &str {
        if player == self.player {
            &*self.for_public
        } else {
            &*self.for_others
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Game {
    deck: Deck,
    hands: Vec<Hand>,
    played: HashMap<Color, Number>,
    discard: HashMap<Color, Vec<Card>>,
    last_move: Move,
    last_move_at: SystemTime,
    clues: usize,
    lives: usize,
    turn: usize,

    last_turns: Option<usize>,
    started: SystemTime,

    is_unwinnable: bool,
}

impl Game {
    /// Start a new game for the given players with a freshly shuffled deck.
    pub(crate) fn new(players: &[String]) -> Self {
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
            discard: Default::default(),
            last_move: Move::new(0, "".to_owned(), "".to_owned()),
            last_move_at: SystemTime::now(),
            clues: 8,
            lives: 3,
            turn: 0,

            last_turns: None,
            started: SystemTime::now(),

            is_unwinnable: false,
        }
    }

    /// Current total score of this game.
    pub(crate) fn score(&self) -> usize {
        self.played.iter().map(|(_, num)| num.as_usize()).sum()
    }

    /// Enumerate the usernames of the players in this game.
    pub(crate) fn players<'a>(&'a self) -> Box<Iterator<Item = &'a String> + 'a> {
        Box::new(self.hands.iter().map(|h| &h.player)) as Box<_>
    }

    /// Get the username of the player whose turn it is.
    pub(crate) fn current_player(&self) -> &str {
        &*self.hands[self.turn].player
    }

    /// Have the current player give `clue` to `to`.
    pub(crate) fn clue(&mut self, to: &str, clue: Clue) -> Result<usize, ClueError> {
        if self.clues == 0 {
            return Err(ClueError::NotEnoughClues);
        }

        // the clone here is unfortunate, but otherwise it's a double-borrow out of self.hands
        let player = self.hands[self.turn].player.clone();
        if self.hands[self.turn].player == to {
            return Err(ClueError::NoSuchPlayer);
        }

        let hands = self.hands.len();
        let hand = if let Some(h) = self.hands.iter_mut().find(|hand| &hand.player == to) {
            h
        } else {
            return Err(ClueError::NoSuchPlayer);
        };

        match hand.clue(self.turn, clue) {
            Ok(num) => {
                let did = format!(
                    "<@{}> clued <@{}> that {} {} {} after {}",
                    player,
                    to,
                    num,
                    if num == 1 { "card is" } else { "cards are" },
                    clue,
                    dur_mod(&mut self.last_move_at),
                );
                self.last_move = Move::new(self.turn, did.clone(), did);
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

    /// Have the current player play the `card`'th card from the left (0-indexed).
    pub(crate) fn play(&mut self, card: usize) -> Result<(), PlayError> {
        let hands = self.hands.len();
        let hand = self.turn;
        if let Some(card) = self.hands.get_mut(hand).unwrap().remove(card) {
            self.hands.get_mut(hand).unwrap().draw(&mut self.deck);

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
                    if card.number == Number::Five {
                        // completed a stack!
                        // get a clue.
                        if self.clues < 8 {
                            self.clues += 1;
                        }
                    }
                    true
                } else {
                    false
                },
            };

            let drew = if self.last_turns.is_none() {
                format!(
                    ", and then drew a {}",
                    self.hands[hand].cards.back().unwrap()
                )
            } else {
                "".to_owned()
            };

            if !success {
                self.lives -= 1;
                let did = format!(
                    "<@{}> incorrectly played a {} after {}",
                    self.hands[self.turn].player,
                    card,
                    dur_mod(&mut self.last_move_at),
                );
                self.last_move = Move::new(self.turn, did.clone(), format!("{}{}", did, drew));

                self.discarded(card);

                if self.lives == 0 {
                    return Err(PlayError::GameOver);
                }
            } else {
                let did = format!(
                    "<@{}> played a {} after {}",
                    self.hands[self.turn].player,
                    card,
                    dur_mod(&mut self.last_move_at),
                );
                self.last_move = Move::new(self.turn, did.clone(), format!("{}{}", did, drew));
            }

            self.turn = (self.turn + 1) % hands;
            if let Some(ref mut last_turns) = self.last_turns {
                *last_turns += 1;
                if *last_turns == hands {
                    return Err(PlayError::GameOver);
                }
            } else if self.deck.is_empty() {
                self.last_turns = Some(0);
            }
            Ok(())
        } else {
            Err(PlayError::NoSuchCard)
        }
    }

    /// Have the current player discard the `card`'th card from the left (0-indexed).
    pub(crate) fn discard(&mut self, card: usize) -> Result<(), DiscardError> {
        if self.clues == 8 {
            return Err(DiscardError::MaxClues);
        }

        let hands = self.hands.len();
        let hand = self.turn;
        if let Some(card) = self.hands.get_mut(hand).unwrap().remove(card) {
            self.hands.get_mut(hand).unwrap().draw(&mut self.deck);

            let drew = if self.last_turns.is_none() {
                format!(
                    ", and then drew a {}",
                    self.hands[hand].cards.back().unwrap()
                )
            } else {
                "".to_owned()
            };

            let did = format!(
                "<@{}> discarded a {} after {}",
                self.hands[self.turn].player,
                card,
                dur_mod(&mut self.last_move_at),
            );
            self.last_move = Move::new(self.turn, did.clone(), format!("{}{}", did, drew));

            self.discarded(card);
            self.clues += 1;
            self.turn = (self.turn + 1) % hands;
            if let Some(ref mut last_turns) = self.last_turns {
                *last_turns += 1;
                if *last_turns == hands {
                    return Err(DiscardError::GameOver);
                }
            } else if self.deck.is_empty() {
                self.last_turns = Some(0);
            }
            Ok(())
        } else {
            Err(DiscardError::NoSuchCard)
        }
    }

    /// Show `user` every other player's hand + what they know.
    pub(crate) fn show_hands(&self, user: &str, skip_self: bool, cli: &mut super::MessageProxy) {
        let me = self.hands
            .iter()
            .position(|hand| &hand.player == user)
            .unwrap();

        cli.send(user, "The other players' hands (in turn order) are:");
        for i in 0..self.hands.len() {
            let hand = (me + i) % self.hands.len();
            if hand == self.turn {
                cli.send(
                    user,
                    &format!("<@{}> &lt;-- current turn", self.hands[hand].player),
                );
            } else {
                cli.send(user, &format!("<@{}>", self.hands[hand].player));
            }
            let (cards, known): (Vec<_>, Vec<_>) = self.hands[hand]
                .cards
                .iter()
                .map(|card| (format!("{}", card), card.known()))
                .unzip();

            if hand == me {
                if !skip_self {
                    cli.send(user, &format!("{} known", &known.join("  |  ")));
                }
            } else {
                cli.send(
                    user,
                    &format!(
                        "{} in hand\n{} known",
                        &cards.join("  |  "),
                        &known.join("  |  ")
                    ),
                );
            }
        }
    }

    /// Show `user` the current state of the discard pile.
    pub(crate) fn show_discards(&self, user: &str, cli: &mut super::MessageProxy) {
        if self.discard.is_empty() {
            cli.send(user, "The discard pile is empty.");
            return;
        }

        cli.send(user, "The discard pile contains the following cards:");
        for color in &COLOR_ORDER {
            if let Some(cards) = self.discard.get(color) {
                let mut out = format!("{} ", color);
                for card in cards {
                    out.push_str(&format!("{}", card.number));
                }
                cli.send(user, &out);
            }
        }
    }

    /// Show `user` the current state of the deck.
    pub(crate) fn show_deck(&self, user: &str, cli: &mut super::MessageProxy) {
        if self.deck.is_empty() {
            cli.send(user, "The deck is depleted.");
            return;
        }

        let width = 10;
        let left: usize =
            (width as f64 * self.deck.len() as f64 / self.deck.of() as f64).round() as usize;
        let progress = format!(
            "`[{}{}]` {} cards left",
            "-".repeat(width - left),
            " ".repeat(left),
            self.deck.len()
        ).replace("- ", "> ");


        cli.send(user, &progress);
    }

    pub fn score_smiley(&self) -> &'static str {
        let points = self.score();
        if points >= 25 {
            ":tada:"
        } else if points >= 24 {
            ":tired_face:"
        } else if points >= 23 {
            ":slightly_smiling_face:"
        } else if points >= 22 {
            ":neutral_face:"
        } else if points >= 20 {
            ":confused:"
        } else if points >= 15 {
            ":slightly_frowning_face:"
        } else if points >= 10 {
            ":disappointed:"
        } else {
            ":face_with_rolling_eyes:"
        }
    }

    pub fn became_unwinnable(&mut self) -> bool {
        if self.is_unwinnable {
            return false;
        }

        // look through the discard pile, and see if all the copies of a given number for any color
        // has been discarded. if so, the game is no longer winnable.
        for (_, cards) in &self.discard {
            let mut number = cards[0].number;
            let mut n = 0;
            for card in cards {
                if card.number == number {
                    n += 1;
                } else {
                    number = card.number;
                    n = 1;
                }

                let total = match number {
                    Number::One => 3,
                    Number::Five => 1,
                    _ => 2,
                };
                if n == total {
                    self.is_unwinnable = true;
                    return true;
                }
            }
        }

        false
    }

    pub fn last_move(&self) -> &str {
        &*self.last_move.for_public
    }

    /// Progress the current game following a turn, and return true if the game has ended.
    ///
    /// This will inform all the users about the current state of the board.
    /// The player whose turn it is will be shown a slightly different message.
    ///
    /// This *could* be called automatially internally, but it'd make the return types of all the
    /// action methods somewhat annoying.
    pub(crate) fn progress_game(&mut self, cli: &mut super::MessageProxy) -> bool {
        if !self.last_move.show_to(0).is_empty() {
            for (i, hand) in self.hands.iter().enumerate() {
                let mut m = self.last_move
                    .show_to(i)
                    .replace(&format!("<@{}>", hand.player), "you");
                if m.starts_with("you") {
                    m = m.replacen("you", "You", 1);
                }
                let m = format!(":point_right: {}", m);
                cli.send(&hand.player, &m);
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
                cli.send(
                    &hand.player,
                    &format!(
                        "Game over after {}.\n\
                         You got {}/25 points {}\n\
                         Your hand at the end was:\n\
                         {}",
                        dur(self.started.elapsed()),
                        points,
                        self.score_smiley(),
                        hand.cards
                            .iter()
                            .map(|c| format!("{}", c))
                            .collect::<Vec<_>>()
                            .join("  |  ")
                    ),
                );
            }
            return true;
        }

        if points == 25 {
            // the game has ended in a win \o/
            for hand in &self.hands {
                cli.send(
                    &hand.player,
                    &format!(
                        "You won the game with 25/25 points after {} {}",
                        dur(self.started.elapsed()),
                        self.score_smiley()
                    ),
                );
            }
            return true;
        }

        // game is not yet over -- let's print the game state
        for i in 0..self.hands.len() {
            self.print_game_state(i, cli);
        }

        false
    }

    /// Called whenever a card is discarded.
    fn discarded(&mut self, card: Card) {
        // insert into sorted discard list for that color
        let d = self.discard.entry(card.color).or_insert_with(Vec::new);
        let pos = d.binary_search_by_key(&card.number.as_usize(), |c| c.number.as_usize())
            .unwrap_or_else(|e| e);
        d.insert(pos, card);
    }

    /// Show the `hand`'th player the current game state.
    ///
    /// Note that the information displayed depends on whether or not it is `hand`'s turn.
    fn print_game_state(&mut self, hand: usize, cli: &mut super::MessageProxy) {
        let user = &self.hands[hand].player;
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
        cli.send(
            user,
            &format!(
                ":hourglass: {}; *{}* :information_source: and {} :bomb: remain.",
                setup,
                self.clues,
                self.lives
            ),
        );

        if !self.deck.is_empty() && self.deck.len() < 2 * self.hands.len() {
            cli.send(
                user,
                &format!(
                    ":warning: *There are only {} cards left in the deck!*",
                    self.deck.len()
                ),
            );
        }

        let stacks: Vec<_> = COLOR_ORDER
            .iter()
            .map(|&color| {
                if let Some(top) = self.played.get(&color) {
                    format!("{} {}", color, top)
                } else {
                    format!("{} :zero:", color)
                }
            })
            .collect();

        if self.turn == hand {
            cli.send(user, &format!("Played:\n{}", stacks.join("  |  ")));

            // it is our turn.
            // show what we know about our hand, and the hands of the following players

            cli.send(user, "Your hand, as far as you know, is:");
            let known: Vec<_> = self.hands[hand]
                .cards
                .iter()
                .enumerate()
                .map(|(i, card)| format!("{}: {}", i + 1, card.known()))
                .collect();
            cli.send(user, &known.join("  |  "));

            cli.send(user, "");
            self.show_hands(user, true, cli);

            cli.send(
                user,
                "\nWhen you have the time, let me know here what move you want to make next!",
            );
        } else {
            // it is *not* our turn.
            // let's not disturb the other players with extraneous information
        }
    }
}
