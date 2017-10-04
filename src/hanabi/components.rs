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

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
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
            Color::Red => write!(f, ":red_circle:"),
            Color::Green => write!(f, ":green_apple:"),
            Color::White => write!(f, ":white_medium_square:"),
            Color::Blue => write!(f, ":large_blue_diamond:"),
            Color::Yellow => write!(f, ":yellow_heart:"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
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

#[derive(Clone, Copy)]
pub(crate) enum Clue {
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

pub(super) struct Card {
    pub(super) color: Color,
    pub(super) number: Number,

    /// All clues given to a player while this card was in their hand.
    /// The `String` is the username of the player who gave each clue.
    pub(super) clues: Vec<(String, Clue)>,
}

impl fmt::Display for Card {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {}", self.color, self.number)
    }
}


pub(super) struct Deck(Vec<Card>);

impl Deck {
    pub(super) fn draw(&mut self) -> Option<Card> {
        self.0.pop()
    }
}

impl Default for Deck {
    fn default() -> Self {
        use rand::{thread_rng, Rng};

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
        let mut cards: Vec<_> = super::COLOR_ORDER
            .iter()
            .flat_map(|&color| {
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

    pub(super) fn clue(&mut self, player: &str, clue: Clue) -> Result<usize, ClueError> {
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
