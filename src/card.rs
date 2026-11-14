//! One card, its print, and the two-letter code it is stored as.
//!
//! No `web-sys`, no `js-sys`, no `wasm-bindgen`: this module compiles natively
//! and its tests run under plain `cargo test`.
//!
//! **This file is carried, byte for byte, by both solitaires in this family** —
//! Solitaire and Spider Solitaire. They are separate repositories, so nothing
//! enforces that; the rule is that a change here belongs in both, or the two
//! games stop agreeing about what a card is. `src/style.css` carries the other
//! half of the card format: the `.card` block, also identical.

use std::fmt;

/// The four suits, in bridge order so that a full deck is also sorted by suit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Suit {
    Clubs,
    Diamonds,
    Hearts,
    Spades,
}

impl Suit {
    /// Every suit, in the order a fresh deck is built and displayed.
    pub const ALL: [Self; 4] = [Self::Clubs, Self::Diamonds, Self::Hearts, Self::Spades];

    /// The printed symbol.
    #[must_use]
    pub const fn symbol(self) -> char {
        match self {
            Self::Clubs => '♣',
            Self::Diamonds => '♦',
            Self::Hearts => '♥',
            Self::Spades => '♠',
        }
    }

    /// The suit's name in running text.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Clubs => "clubs",
            Self::Diamonds => "diamonds",
            Self::Hearts => "hearts",
            Self::Spades => "spades",
        }
    }

    /// The one-letter code used by the stored game.
    #[must_use]
    pub const fn code(self) -> char {
        match self {
            Self::Clubs => 'C',
            Self::Diamonds => 'D',
            Self::Hearts => 'H',
            Self::Spades => 'S',
        }
    }

    /// A suit from its stored code, case-insensitively.
    #[must_use]
    pub const fn from_code(code: char) -> Option<Self> {
        match code {
            'C' | 'c' => Some(Self::Clubs),
            'D' | 'd' => Some(Self::Diamonds),
            'H' | 'h' => Some(Self::Hearts),
            'S' | 's' => Some(Self::Spades),
            _ => None,
        }
    }

    /// Whether the suit is printed in red.
    #[must_use]
    pub const fn is_red(self) -> bool {
        matches!(self, Self::Diamonds | Self::Hearts)
    }

    /// The class token the stylesheet colours the face by.
    #[must_use]
    pub const fn class(self) -> &'static str {
        if self.is_red() {
            "red"
        } else {
            "black"
        }
    }
}

/// The thirteen ranks, ace low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Rank {
    Ace = 1,
    Two = 2,
    Three = 3,
    Four = 4,
    Five = 5,
    Six = 6,
    Seven = 7,
    Eight = 8,
    Nine = 9,
    Ten = 10,
    Jack = 11,
    Queen = 12,
    King = 13,
}

impl Rank {
    /// Every rank, ace first.
    pub const ALL: [Self; 13] = [
        Self::Ace,
        Self::Two,
        Self::Three,
        Self::Four,
        Self::Five,
        Self::Six,
        Self::Seven,
        Self::Eight,
        Self::Nine,
        Self::Ten,
        Self::Jack,
        Self::Queen,
        Self::King,
    ];

    /// The rank as a number, ace = 1 and king = 13.
    #[must_use]
    pub const fn value(self) -> u8 {
        self as u8
    }

    /// A rank from its number, rejecting anything outside 1..=13.
    #[must_use]
    pub const fn from_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Ace),
            2 => Some(Self::Two),
            3 => Some(Self::Three),
            4 => Some(Self::Four),
            5 => Some(Self::Five),
            6 => Some(Self::Six),
            7 => Some(Self::Seven),
            8 => Some(Self::Eight),
            9 => Some(Self::Nine),
            10 => Some(Self::Ten),
            11 => Some(Self::Jack),
            12 => Some(Self::Queen),
            13 => Some(Self::King),
            _ => None,
        }
    }

    /// The rank's name in running text.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ace => "ace",
            Self::Two => "two",
            Self::Three => "three",
            Self::Four => "four",
            Self::Five => "five",
            Self::Six => "six",
            Self::Seven => "seven",
            Self::Eight => "eight",
            Self::Nine => "nine",
            Self::Ten => "ten",
            Self::Jack => "jack",
            Self::Queen => "queen",
            Self::King => "king",
        }
    }

    /// The rank as it is printed in a corner.
    #[must_use]
    pub const fn short(self) -> &'static str {
        match self {
            Self::Ace => "A",
            Self::Two => "2",
            Self::Three => "3",
            Self::Four => "4",
            Self::Five => "5",
            Self::Six => "6",
            Self::Seven => "7",
            Self::Eight => "8",
            Self::Nine => "9",
            Self::Ten => "10",
            Self::Jack => "J",
            Self::Queen => "Q",
            Self::King => "K",
        }
    }

    /// The next rank up, or `None` for a king.
    #[must_use]
    pub const fn up(self) -> Option<Self> {
        Self::from_value(self.value() + 1)
    }

    /// The next rank down, or `None` for an ace.
    #[must_use]
    pub const fn down(self) -> Option<Self> {
        if self.value() == 1 {
            None
        } else {
            Self::from_value(self.value() - 1)
        }
    }
}

/// One printed card: a rank and a suit. Identity is the pair, which is why a
/// deck is built by pairing every rank with every suit rather than by numbering
/// cards: the rules never need to tell two copies of an ace apart, only the
/// screen does, and the screen indexes the deck instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Card {
    /// The rank.
    pub rank: Rank,
    /// The suit.
    pub suit: Suit,
}

impl Card {
    /// A card of `rank` and `suit`.
    #[must_use]
    pub const fn new(rank: Rank, suit: Suit) -> Self {
        Self { rank, suit }
    }

    /// Whether the card is printed red.
    #[must_use]
    pub const fn is_red(self) -> bool {
        self.suit.is_red()
    }

    /// The two-character code the saved game uses, e.g. `"AS"` or `"10H"`.
    ///
    /// Ten is `T` — two characters for every card keeps a pile a fixed-width
    /// string that can be split without a separator.
    #[must_use]
    pub fn code(self) -> String {
        let rank = match self.rank {
            Rank::Ten => 'T',
            other => other.short().chars().next().unwrap_or('?'),
        };
        let mut code = String::with_capacity(2);
        code.push(rank);
        code.push(self.suit.code());
        code
    }

    /// A card from its two-character code.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        let mut chars = code.trim().chars();
        let rank = chars.next()?;
        let suit = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        let rank = match rank {
            'A' | 'a' => Rank::Ace,
            '2' => Rank::Two,
            '3' => Rank::Three,
            '4' => Rank::Four,
            '5' => Rank::Five,
            '6' => Rank::Six,
            '7' => Rank::Seven,
            '8' => Rank::Eight,
            '9' => Rank::Nine,
            'T' | 't' | '0' => Rank::Ten,
            'J' | 'j' => Rank::Jack,
            'Q' | 'q' => Rank::Queen,
            'K' | 'k' => Rank::King,
            _ => return None,
        };
        Some(Self::new(rank, Suit::from_code(suit)?))
    }

    /// What a screen reader calls this card: `"ace of spades"`.
    #[must_use]
    pub fn name(self) -> String {
        format!("{} of {}", self.rank.name(), self.suit.name())
    }

    /// The printed short form, e.g. `"A♠"`, for status lines that quote a card.
    #[must_use]
    pub fn short(self) -> String {
        format!("{}{}", self.rank.short(), self.suit.symbol())
    }
}

impl fmt::Display for Card {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} of {}", self.rank.name(), self.suit.name())
    }
}

/// The whole deck in rank-major order: `A♣ 2♣ … K♣ A♦ … K♠`.
///
/// Used where the *order* matters rather than the deal — building the deck the
/// game shuffles, and giving a card a stable index the screen can key on.
#[must_use]
pub fn deck() -> Vec<Card> {
    let mut cards = Vec::with_capacity(52);
    for suit in Suit::ALL {
        for rank in Rank::ALL {
            cards.push(Card::new(rank, suit));
        }
    }
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deck_holds_every_card_once() {
        let cards = deck();
        assert_eq!(cards.len(), 52);
        let mut seen = cards.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 52, "a card was printed twice");
    }

    #[test]
    fn every_card_round_trips_through_its_code() {
        for card in deck() {
            let code = card.code();
            assert_eq!(code.len(), 2, "{code}");
            assert_eq!(Card::from_code(&code), Some(card), "{code}");
        }
        assert_eq!(Card::new(Rank::Ten, Suit::Hearts).code(), "TH");
    }

    #[test]
    fn unreadable_codes_are_rejected_rather_than_guessed() {
        for bad in ["", "A", "ASX", "1S", "AX", "SS", " A "] {
            assert_eq!(Card::from_code(bad), None, "input: {bad:?}");
        }
        // A leading or trailing space is harmless; a hand-written save can have
        // one, and it is not a different card.
        assert_eq!(
            Card::from_code(" AS "),
            Some(Card::new(Rank::Ace, Suit::Spades))
        );
    }

    #[test]
    fn ranks_step_up_and_down_and_stop_at_the_ends() {
        assert_eq!(Rank::Ace.up(), Some(Rank::Two));
        assert_eq!(Rank::King.up(), None);
        assert_eq!(Rank::Ace.down(), None);
        assert_eq!(Rank::Two.down(), Some(Rank::Ace));
        assert!(Rank::from_value(0).is_none());
        assert!(Rank::from_value(14).is_none());
        for rank in Rank::ALL {
            assert_eq!(Rank::from_value(rank.value()), Some(rank));
        }
    }

    #[test]
    fn suits_print_in_two_colours_and_round_trip() {
        assert!(Suit::Hearts.is_red());
        assert!(Suit::Diamonds.is_red());
        assert!(!Suit::Clubs.is_red());
        assert!(!Suit::Spades.is_red());
        for suit in Suit::ALL {
            assert_eq!(Suit::from_code(suit.code()), Some(suit));
            assert_eq!(
                Suit::from_code(suit.code().to_ascii_lowercase()),
                Some(suit)
            );
        }
    }

    #[test]
    fn a_card_is_named_for_the_screen_reader() {
        let card = Card::new(Rank::Ace, Suit::Spades);
        assert_eq!(card.name(), "ace of spades");
        assert_eq!(card.short(), "A♠");
        assert_eq!(card.to_string(), "ace of spades");
    }
}
