use rand::seq::SliceRandom;
use std::collections::LinkedList;

/// An error that occurred while giving a clue.
pub(crate) enum ClueError {
    NoSuchPlayer,
    NoMatchingCards,
    NotEnoughClues,
    GameOver,
}

/// An error that occurred while giving playing a card.
pub(crate) enum PlayError {
    NoSuchCard,
    GameOver,
}

/// An error that occurred while giving discarding a card.
pub(crate) enum DiscardError {
    NoSuchCard,
    MaxClues,
    GameOver,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Color {
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
            Color::Red => write!(f, ":heart:"),
            Color::Green => write!(f, ":deciduous_tree:"),
            Color::White => write!(f, ":cloud:"),
            Color::Blue => write!(f, ":droplet:"),
            Color::Yellow => write!(f, ":sunny:"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Number {
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
    pub(super) fn as_usize(&self) -> usize {
        match *self {
            Number::One => 1,
            Number::Two => 2,
            Number::Three => 3,
            Number::Four => 4,
            Number::Five => 5,
        }
    }
}

use serde::{Deserialize, Serialize};
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
            // this should probably never happen
            Number::Five => Number::Five,
        };
        next + (rhs - 1)
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum Clue {
    Color(Color),
    Number(Number),
}

impl fmt::Display for Clue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Clue::Color(ref c) => write!(f, "{c}"),
            Clue::Number(ref n) => write!(f, "{n}"),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Card {
    pub(super) color: Color,
    pub(super) number: Number,

    /// All clues given to a player while this card was in their hand.
    /// The `usize` is the hand index of the player who gave each clue.
    pub(super) clues: Vec<(usize, Clue)>,
}

impl fmt::Display for Card {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {}", self.color, self.number)
    }
}

impl Card {
    pub fn known(&self) -> String {
        let know_color = self.clues.iter().any(|&(_, clue)| match clue {
            Clue::Color(ref c) => c == &self.color,
            _ => false,
        });
        let know_number = self.clues.iter().any(|&(_, clue)| match clue {
            Clue::Number(ref n) => n == &self.number,
            _ => false,
        });

        match (know_color, know_number) {
            (false, false) => ":rainbow: :keycap_star:".to_string(),
            (false, true) => format!(":rainbow: {}", self.number),
            (true, false) => format!("{} :keycap_star:", self.color),
            (true, true) => format!("{} {}", self.color, self.number),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Deck(usize, Vec<Card>);

impl Deck {
    pub(super) fn is_empty(&self) -> bool {
        self.1.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.1.len()
    }

    pub(super) fn of(&self) -> usize {
        self.0
    }

    pub(super) fn draw(&mut self) -> Option<Card> {
        self.1.pop()
    }
}

impl Default for Deck {
    fn default() -> Self {
        let numbers = [
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
        let mut cards: Vec<_> = super::COLOR_ORDER
            .iter()
            .flat_map(|&color| {
                numbers.iter().map(move |&number| Card {
                    color,
                    number,
                    clues: Vec::new(),
                })
            })
            .collect();

        cards.shuffle(&mut rand::rng());
        Deck(cards.len(), cards)
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Hand {
    pub(super) player: String,
    pub(super) cards: LinkedList<Card>,
}

impl Hand {
    pub(super) fn new(player: &str) -> Self {
        Hand {
            player: String::from(player),
            cards: LinkedList::default(),
        }
    }

    pub(super) fn draw(&mut self, deck: &mut Deck) -> bool {
        deck.draw().map(|card| self.cards.push_back(card)).is_some()
    }

    pub(super) fn clue(&mut self, player: usize, clue: Clue) -> Result<usize, ClueError> {
        let matches = self
            .cards
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
            card.clues.push((player, clue));
        }

        Ok(matches)
    }

    pub(super) fn remove(&mut self, card: usize) -> Option<Card> {
        if card > self.cards.len() {
            return None;
        }

        let mut after = self.cards.split_off(card);
        let card = after.pop_front();
        self.cards.append(&mut after);
        card
    }
}
