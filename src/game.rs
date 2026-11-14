//! Spider as a value: the deal, the rules, the score, the undo history and every
//! word the screen says about them.
//!
//! No `web-sys`, no `js-sys`, no `wasm-bindgen`, so this module compiles and
//! runs natively under `cargo test`. The browser layer in `crate::ui` decides
//! nothing: it turns a tap into a [`Pile`] and an index, and paints what this
//! module reports.
//!
//! Three rules carry the design:
//!
//! * **Every mutation is a transaction.** A move, a deal or a recycle is checked
//!   against a snapshot taken first, so undo is correct by construction — and
//!   that matters more here than in Klondike, because a single move can complete
//!   a run and take thirteen cards off the board.
//! * **Every random deal is a seed.** The same seed deals the same game, which is
//!   what makes "Restart deal" honest and a saved game a few hundred bytes.
//! * **Every word is written here.** [`Game::readout`], [`Report`] and
//!   [`Game::result_words`] are the only places that turn state into prose.

use std::fmt::Write as _;

use crate::card::{Card, Rank, Suit};

/// Key the game is saved under. One prefix per app, so two PWAs on the same
/// origin cannot read or overwrite each other's state.
pub const STORAGE_KEY: &str = "spider_solitaire.game.v1";

/// Key the next deal's options are saved under.
pub const OPTIONS_KEY: &str = "spider_solitaire.options.v1";

/// How many moves can be taken back.
pub const HISTORY_LIMIT: usize = 400;

/// How many tableau columns a Spider board has.
pub const COLUMNS: usize = 10;

/// How many completed runs win the game — a spider's eight legs.
pub const RUNS: usize = 8;

/// The longest a game clock will hold: just under 100 hours.
const ELAPSED_LIMIT: u32 = 359_999;

/// How many `|`-separated fields a stored game has.
///
/// The header, the stock, one field per column, and the completed runs. Named
/// once because the writer and the parser must agree, and a count written twice
/// is one edit away from a save that no longer loads.
const STORED_FIELDS: usize = 9 + 1 + COLUMNS + 1;

/// How difficult the deal is, which in Spider is exactly how many suits are in
/// play.
///
/// That is the axis every established implementation uses, and it is not a
/// slider for its own sake: with one suit every descending run is also a suit
/// run, so any group can be moved and any complete sequence can be lifted. With
/// four suits most descending neighbours are a mixed pair, so a run is a real
/// achievement. The labels say which is which, because "Easy" alone does not
/// tell a player what they are choosing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    /// One suit: every card is a spade.
    Easy,
    /// Two suits: spades and hearts.
    Medium,
    /// Four suits: the full two-deck game.
    Hard,
}

impl Difficulty {
    /// Every difficulty, easiest first — the order the menu lists them in.
    pub const ALL: [Self; 3] = [Self::Easy, Self::Medium, Self::Hard];

    /// The suits this difficulty deals with.
    ///
    /// One suit is eight copies of a single deck's worth of spades; two suits
    /// are four copies each of spades and hearts; four suits are the ordinary
    /// two decks. Every difficulty therefore deals exactly 104 cards, which is
    /// what makes the stock's five deals of ten work in all three.
    #[must_use]
    pub fn suits(self) -> &'static [Suit] {
        match self {
            Self::Easy => &[Suit::Spades],
            Self::Medium => &[Suit::Spades, Suit::Hearts],
            Self::Hard => Suit::ALL.as_slice(),
        }
    }

    /// How many copies of each card are in the deck.
    #[must_use]
    pub const fn copies(self) -> usize {
        match self {
            Self::Easy => 8,
            Self::Medium => 4,
            Self::Hard => 2,
        }
    }

    /// The word for this difficulty.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
        }
    }

    /// The difficulty's name as stored and parsed.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Medium => "medium",
            Self::Hard => "hard",
        }
    }

    /// A difficulty from its stored name.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.slug() == slug)
    }

    /// How many suits are in play.
    #[must_use]
    pub fn suit_count(self) -> usize {
        self.suits().len()
    }

    /// The rule in one sentence, for the menu and the status line.
    #[must_use]
    pub const fn sentence(self) -> &'static str {
        match self {
            Self::Easy => {
                "One suit: every card is a spade, so every descending run can move as a group."
            }
            Self::Medium => "Two suits: spades and hearts, four of each card.",
            Self::Hard => "Four suits: the full two-deck game, and runs are worth earning.",
        }
    }

    /// The short phrase a label uses, e.g. `"one suit"`.
    #[must_use]
    pub const fn suits_phrase(self) -> &'static str {
        match self {
            Self::Easy => "one suit",
            Self::Medium => "two suits",
            Self::Hard => "four suits",
        }
    }
}

/// How the game is scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scoring {
    /// The classic table: 500 to start, a point for each move, a point for each
    /// undo, and 100 for every completed run.
    Standard,
    /// No score at all: the board and the clock, and no arithmetic.
    Off,
}

impl Scoring {
    /// Every style, in the order the menu lists them.
    pub const ALL: [Self; 2] = [Self::Standard, Self::Off];

    /// The word for this style.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Off => "No score",
        }
    }

    /// The style's stored name.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Off => "off",
        }
    }

    /// A style from its stored name.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.slug() == slug)
    }

    /// The style in one sentence, for the menu.
    #[must_use]
    pub const fn sentence(self) -> &'static str {
        match self {
            Self::Standard => {
                "Start at 500. A point for each move and each undo, 100 for each completed run."
            }
            Self::Off => "No score, no penalty. Just the board and the clock.",
        }
    }

    /// What the game starts at.
    #[must_use]
    pub const fn opening(self) -> i32 {
        match self {
            Self::Standard => 500,
            Self::Off => 0,
        }
    }
}

/// The next deal's settings: what the menu shows and what New game applies.
///
/// Deliberately separate from the running [`Game`]: changing a select is a
/// request for the *next* deal, and must not throw away the game in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// How many suits the next deal uses.
    pub difficulty: Difficulty,
    /// How the next deal is scored.
    pub scoring: Scoring,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            difficulty: Difficulty::Medium,
            scoring: Scoring::Standard,
        }
    }
}

impl Options {
    /// Options from the stored line, falling back to the defaults.
    #[must_use]
    pub fn from_stored(raw: Option<&str>) -> Self {
        raw.and_then(Self::parse).unwrap_or_default()
    }

    /// The line to store.
    #[must_use]
    pub fn to_stored(self) -> String {
        format!("v1|{}|{}", self.difficulty.slug(), self.scoring.slug())
    }

    fn parse(text: &str) -> Option<Self> {
        let mut fields = text.split('|');
        if fields.next()? != "v1" {
            return None;
        }
        let difficulty = Difficulty::from_slug(fields.next()?)?;
        let scoring = Scoring::from_slug(fields.next()?)?;
        if fields.next().is_some() {
            return None;
        }
        Some(Self {
            difficulty,
            scoring,
        })
    }

    /// The rule summary, e.g. `"Medium · two suits · five deals · standard scoring"`.
    #[must_use]
    pub fn summary(self) -> String {
        format!(
            "{} · {} · five deals · {} scoring",
            self.difficulty.label(),
            self.difficulty.suits_phrase(),
            self.scoring.label().to_lowercase()
        )
    }
}

/// One place on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pile {
    /// The facedown stock, which deals one card to every column.
    Stock,
    /// One of the ten tableau columns.
    Tableau(usize),
}

impl Pile {
    /// Every interactive pile, in the order the page lays them out.
    pub const ALL: [Self; 1 + COLUMNS] = [
        Self::Stock,
        Self::Tableau(0),
        Self::Tableau(1),
        Self::Tableau(2),
        Self::Tableau(3),
        Self::Tableau(4),
        Self::Tableau(5),
        Self::Tableau(6),
        Self::Tableau(7),
        Self::Tableau(8),
        Self::Tableau(9),
    ];

    /// The pile's name in running text.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Self::Stock => "the stock".to_owned(),
            Self::Tableau(index) => format!("column {}", index + 1),
        }
    }

    /// The stable token the DOM carries and parses back.
    #[must_use]
    pub fn slug(self) -> String {
        match self {
            Self::Stock => "stock".to_owned(),
            Self::Tableau(index) => format!("tableau-{index}"),
        }
    }

    /// A pile from its DOM token, rejecting anything out of range.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        if slug == "stock" {
            return Some(Self::Stock);
        }
        let index = slug.strip_prefix("tableau-")?.parse::<usize>().ok()?;
        (index < COLUMNS).then_some(Self::Tableau(index))
    }
}

/// One card picked up on the board: where it starts, and how far up its pile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// The column the run is in.
    pub pile: Pile,
    /// The position of the run's first card within that column.
    pub index: usize,
}

/// One tableau column: its cards, and how many at the bottom are face down.
#[derive(Debug, Clone)]
struct Column {
    cards: Vec<u16>,
    /// The first face-up card's index; `cards.len()` means none are face up.
    up: usize,
}

impl Column {
    fn new() -> Self {
        Self {
            cards: Vec::new(),
            up: 0,
        }
    }

    fn len(&self) -> usize {
        self.cards.len()
    }

    fn is_face_up(&self, index: usize) -> bool {
        index >= self.up && index < self.cards.len()
    }

    fn top(&self) -> Option<u16> {
        self.cards.last().copied()
    }
}

/// A state that undo returns to.
#[derive(Debug, Clone)]
struct Snapshot {
    tableau: [Column; COLUMNS],
    stock: Vec<u16>,
    completed: Vec<Suit>,
    score: i32,
    moves: u32,
    elapsed: u32,
    won: bool,
}

/// What an action just did, in the game's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// True when the player won on this action.
    pub won: bool,
    /// One sentence for the status line. Empty when there is nothing worth
    /// saying.
    pub words: String,
}

impl Report {
    fn quiet(words: impl Into<String>) -> Self {
        Self {
            won: false,
            words: words.into(),
        }
    }
}

/// A suggested move, or a suggestion about the stock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
    /// Move the run beginning at `index` in `from` to `to`.
    Move {
        /// The source column.
        from: Pile,
        /// The run's first card within the source.
        index: usize,
        /// The destination column.
        to: Pile,
        /// How many cards the move carries.
        count: usize,
        /// The sentence the status line shows.
        words: String,
    },
    /// The stock has cards to deal.
    Deal {
        /// The sentence the status line shows.
        words: String,
    },
    /// There is nothing useful left to do.
    None {
        /// The sentence the status line shows.
        words: String,
    },
}

impl Hint {
    /// The sentence the status line shows.
    #[must_use]
    pub fn words(&self) -> &str {
        match self {
            Self::Move { words, .. } | Self::Deal { words } | Self::None { words } => words,
        }
    }
}

/// The facts the header shows, already formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readout {
    /// `m:ss`, or `h:mm:ss` past an hour.
    pub clock: String,
    /// The move count.
    pub moves: String,
    /// The score, or `None` when scoring is off.
    pub score: Option<String>,
    /// `"3 / 8"` — completed runs against the eight the game needs.
    pub runs: String,
    /// How many deals the stock still holds.
    pub deals: String,
}

/// A game in progress.
pub struct Game {
    /// The immutable deck; a pile holds indexes into it.
    ///
    /// Built once and moved by index, so a snapshot never copies a card and the
    /// page has a stable element key per physical card — which matters here,
    /// because one-suit Spider holds eight copies of every card and only the
    /// index can tell them apart.
    cards: Vec<Card>,
    stock: Vec<u16>,
    tableau: [Column; COLUMNS],
    /// The suits of the runs already completed, in the order they were made.
    completed: Vec<Suit>,
    difficulty: Difficulty,
    scoring: Scoring,
    seed: u64,
    score: i32,
    moves: u32,
    elapsed: u32,
    won: bool,
    /// False until the player has done something, so an untouched first visit
    /// writes nothing.
    started: bool,
    history: Vec<Snapshot>,
    selection: Option<Selection>,
}

impl Game {
    /// A fresh deal, ready for the page but not yet played.
    #[must_use]
    pub fn new(difficulty: Difficulty, scoring: Scoring, seed: u64) -> Self {
        let cards = deck(difficulty);
        let mut order: Vec<u16> = (0..cards.len() as u16).collect();
        shuffle(&mut order, seed);

        let mut tableau: [Column; COLUMNS] = std::array::from_fn(|_| Column::new());
        let mut next = order.into_iter();
        // The conventional deal: six cards to each of the first four columns,
        // five to the rest, and the last card of every column face up.
        for (index, column) in tableau.iter_mut().enumerate() {
            let count = if index < 4 { 6 } else { 5 };
            for _ in 0..count {
                if let Some(card) = next.next() {
                    column.cards.push(card);
                }
            }
            column.up = column.cards.len().saturating_sub(1);
        }
        let stock: Vec<u16> = next.collect();

        Self {
            cards,
            stock,
            tableau,
            completed: Vec::new(),
            difficulty,
            scoring,
            seed,
            score: scoring.opening(),
            moves: 0,
            elapsed: 0,
            won: false,
            started: false,
            history: Vec::new(),
            selection: None,
        }
    }

    /// A fresh deal the player asked for, which may be written to storage at
    /// once even if they have not moved yet.
    #[must_use]
    pub fn dealt(difficulty: Difficulty, scoring: Scoring, seed: u64) -> Self {
        let mut game = Self::new(difficulty, scoring, seed);
        game.started = true;
        game
    }

    /// A new deal with the same rules and the same card order.
    #[must_use]
    pub fn restart(&self) -> Self {
        Self::dealt(self.difficulty, self.scoring, self.seed)
    }

    /// The card behind a deck index.
    #[must_use]
    pub fn card(&self, index: u16) -> Card {
        self.cards[index as usize]
    }

    /// The pile's cards, bottom first.
    #[must_use]
    pub fn pile(&self, pile: Pile) -> &[u16] {
        match pile {
            Pile::Stock => &self.stock,
            Pile::Tableau(index) => &self.tableau[index].cards,
        }
    }

    /// How many cards a pile holds.
    #[must_use]
    pub fn pile_len(&self, pile: Pile) -> usize {
        self.pile(pile).len()
    }

    /// How many cards are in the deck.
    #[must_use]
    pub fn deck_len(&self) -> usize {
        self.cards.len()
    }

    /// Whether a card in a pile is printed face down.
    #[must_use]
    pub fn is_face_down(&self, pile: Pile, index: usize) -> bool {
        match pile {
            Pile::Stock => index < self.stock.len(),
            Pile::Tableau(column) => !self.tableau[column].is_face_up(index),
        }
    }

    /// The rules of this deal.
    #[must_use]
    pub const fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    /// The scoring style of this deal.
    #[must_use]
    pub const fn scoring(&self) -> Scoring {
        self.scoring
    }

    /// The seed this deal was shuffled from.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// The turn in progress, if any.
    #[must_use]
    pub const fn selection(&self) -> Option<Selection> {
        self.selection
    }

    /// True once the player has done something.
    #[must_use]
    pub const fn started(&self) -> bool {
        self.started
    }

    /// True when all eight runs are complete.
    #[must_use]
    pub const fn won(&self) -> bool {
        self.won
    }

    /// The current score.
    #[must_use]
    pub const fn score(&self) -> i32 {
        self.score
    }

    /// The number of moves made, including deals and undos.
    #[must_use]
    pub const fn moves(&self) -> u32 {
        self.moves
    }

    /// Seconds on the clock.
    #[must_use]
    pub const fn elapsed(&self) -> u32 {
        self.elapsed
    }

    /// How many runs are complete.
    #[must_use]
    pub fn runs(&self) -> usize {
        self.completed.len()
    }

    /// The suits of the completed runs, in the order they were made.
    #[must_use]
    pub fn completed_suits(&self) -> &[Suit] {
        &self.completed
    }

    /// How many deals the stock still holds, one card per column each time.
    #[must_use]
    pub fn deals_left(&self) -> usize {
        if self.stock.is_empty() {
            0
        } else {
            self.stock.len().div_ceil(COLUMNS)
        }
    }

    /// Whether there is a move to take back.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    /// The rules in one line, for the menu.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} · {} · {} · {} deals left · {} scoring",
            self.difficulty.label(),
            self.difficulty.suits_phrase(),
            if self.runs() == RUNS {
                "all runs complete".to_owned()
            } else {
                format!("{} of {RUNS} runs", self.runs())
            },
            self.deals_left(),
            self.scoring.label().to_lowercase()
        )
    }

    /// The short label the header shows beside the title, e.g. `"2 suits"`.
    #[must_use]
    pub fn variant(&self) -> &'static str {
        self.difficulty.suits_phrase()
    }

    /// The formatted facts for the header.
    #[must_use]
    pub fn readout(&self) -> Readout {
        Readout {
            clock: clock_text(self.elapsed),
            moves: self.moves.to_string(),
            score: match self.scoring {
                Scoring::Standard => Some(grouped(i64::from(self.score))),
                Scoring::Off => None,
            },
            runs: format!("{} / {RUNS}", self.runs()),
            deals: self.deals_left().to_string(),
        }
    }

    /// What the status line says when the game is over.
    #[must_use]
    pub fn result_words(&self) -> String {
        let moves = if self.moves == 1 {
            "1 move".to_owned()
        } else {
            format!("{} moves", self.moves)
        };
        let score = match self.scoring {
            Scoring::Standard => format!(" and {} points.", grouped(i64::from(self.score))),
            Scoring::Off => String::from("."),
        };
        format!(
            "You won in {} with {}{}",
            clock_text(self.elapsed),
            moves,
            score
        )
    }

    /// Advance the clock. A won game stops.
    pub fn tick(&mut self, seconds: u32) {
        if self.won {
            return;
        }
        self.elapsed = self.elapsed.saturating_add(seconds).min(ELAPSED_LIMIT);
    }

    /// Handle a tap on a card (or on an empty column, where `index` is its
    /// length).
    ///
    /// The whole click grammar lives here: pick up, put down, put the picked-up
    /// run down, or switch the pick to another movable run.
    pub fn tap(&mut self, pile: Pile, index: usize) -> Report {
        if self.won {
            return Report::quiet("The game is over. Start a new one.");
        }
        if pile == Pile::Stock {
            return self.deal();
        }
        if self.selection.is_none() && self.pile_len(pile) == index {
            // A tap on an empty column with nothing in hand is not a move and
            // not a mistake worth a sentence.
            return Report::quiet("");
        }
        if let Some(selection) = self.selection {
            if selection.pile == pile {
                if selection.index == index {
                    self.selection = None;
                    return Report::quiet("Selection cleared.");
                }
                return self.select(pile, index);
            }
            if self.can_move(selection, pile) {
                return self.apply(selection, pile);
            }
            if self.can_pick(pile, index) {
                return self.select(pile, index);
            }
            return Report::quiet("That card cannot go there.");
        }
        self.select(pile, index)
    }

    /// Pick up the run beginning at `index`, if it is movable as a group.
    pub fn select(&mut self, pile: Pile, index: usize) -> Report {
        let Some(count) = self.run_len(pile, index) else {
            if index < self.pile_len(pile) && self.is_face_down(pile, index) {
                return Report::quiet("That card is face down.");
            }
            return Report::quiet("That card cannot move.");
        };
        let Some(card) = self.card_at(pile, index) else {
            return Report::quiet("That card cannot move.");
        };
        self.selection = Some(Selection { pile, index });
        if count == 1 {
            Report::quiet(format!("Picked up the {}.", card.name()))
        } else {
            Report::quiet(format!(
                "Picked up the {} and {} more.",
                card.name(),
                count - 1
            ))
        }
    }

    /// Whether the run at `pile`/`index` can be picked up.
    #[must_use]
    pub fn can_pick(&self, pile: Pile, index: usize) -> bool {
        !self.won && self.run_len(pile, index).is_some()
    }

    /// Drop the held run on `to`. The page calls this on a drag; it is the same
    /// transaction as tapping the destination.
    pub fn drop(&mut self, to: Pile) -> Report {
        let Some(selection) = self.selection else {
            return Report::quiet("");
        };
        if selection.pile == to {
            self.selection = None;
            return Report::quiet("Put back.");
        }
        if !self.can_move(selection, to) {
            return Report::quiet("That card cannot go there.");
        }
        self.apply(selection, to)
    }

    /// Send the tapped run to the best column for it.
    ///
    /// A double tap is the shortcut every implementation has: onto the column
    /// that extends a run of the same suit, otherwise onto the column with the
    /// most cards below, otherwise onto an empty column.
    pub fn double_tap(&mut self, pile: Pile, index: usize) -> Report {
        if self.won {
            return Report::quiet("The game is over. Start a new one.");
        }
        let Some(count) = self.run_len(pile, index) else {
            return Report::quiet("That card cannot move.");
        };
        let Some(card) = self.card_at(pile, index) else {
            return Report::quiet("That card cannot move.");
        };
        let mut best: Option<(i32, Pile)> = None;
        for column in 0..COLUMNS {
            let to = Pile::Tableau(column);
            if to == pile || !self.accepts(to, card) {
                continue;
            }
            let mut score = 0;
            match self.tableau[column].top() {
                // Same suit: this card would extend a run that can actually be
                // lifted one day, so it is worth the most.
                Some(top) if self.card(top).suit == card.suit => score += 100,
                // A non-empty column keeps the run visible and does not spend
                // the one thing empties are for.
                Some(_) => score += 40,
                // An empty column is worth something, but a card parked there
                // can only be retrieved by another move.
                None => score += 10,
            }
            if self.tableau[column].len() >= count {
                score += 5;
            }
            if best.is_none_or(|(best_score, _)| score > best_score) {
                best = Some((score, to));
            }
        }
        match best {
            Some((_, to)) => self.apply(Selection { pile, index }, to),
            None => {
                self.selection = None;
                Report::quiet(format!("No move for the {}.", card.name()))
            }
        }
    }

    /// Deal one card to every column.
    ///
    /// The classic rule holds: the stock cannot be dealt while any column is
    /// empty. That is what makes an empty column a resource rather than a
    /// convenience, and it is the rule every serious implementation keeps.
    pub fn deal(&mut self) -> Report {
        if self.won {
            return Report::quiet("The game is over. Start a new one.");
        }
        if self.stock.is_empty() {
            return Report::quiet("The stock is empty.");
        }
        if !self.can_deal() {
            return Report::quiet("Fill every empty column before dealing.");
        }
        self.push_history();
        self.selection = None;
        for column in 0..COLUMNS {
            if let Some(card) = self.stock.pop() {
                self.tableau[column].cards.push(card);
                self.tableau[column].up = self.tableau[column].len() - 1;
            }
        }
        self.moves = self.moves.saturating_add(1);
        self.score = self.charge(self.score);
        self.started = true;
        let mut words = format!(
            "Dealt one card to every column. {} left.",
            self.deals_left()
        );
        if let Some(completed) = self.collect_runs() {
            words.push(' ');
            words.push_str(&completed);
        }
        if self.completed.len() == RUNS {
            self.won = true;
            words = self.result_words();
            return Report { won: true, words };
        }
        Report::quiet(words)
    }

    /// Whether the stock can be dealt right now.
    #[must_use]
    pub fn can_deal(&self) -> bool {
        !self.stock.is_empty() && !self.tableau.iter().any(|column| column.cards.is_empty())
    }

    /// Take back the last move, deal or automatic completion.
    ///
    /// The undo counts as an action in the standard table, and it is the one
    /// place this game's history is not a plain snapshot restore. The rule:
    /// an undo takes back what the action *earned* and keeps what it *cost*,
    /// then charges itself a point. So undoing a move that cost a point leaves
    /// the cost in place and adds one; undoing a completed run puts the run back
    /// and takes its hundred points away, which is what stops a player farming
    /// points by finishing the same run twice. "No score" turns all of it off.
    pub fn undo(&mut self) -> Report {
        let Some(snapshot) = self.history.pop() else {
            return Report::quiet("Nothing to undo.");
        };
        let earned = self.score;
        let restored = snapshot.score;
        self.restore(snapshot);
        self.selection = None;
        // The move counter goes back with the board: it counts the moves on the
        // board, and the undone one is not on it any more. The score is the one
        // thing an undo leaves changed, because the table charges for it.
        self.score = match self.scoring {
            Scoring::Standard => earned.min(restored).saturating_sub(1).max(0),
            Scoring::Off => self.score,
        };
        Report::quiet("Undid the last move.")
    }

    /// Suggest a move. The suggestion is a heuristic, not a solver: it prefers
    /// a move that completes a run, then one that turns over a hidden card, then
    /// an in-suit placement, and it would rather deal than open an empty column
    /// for nothing.
    #[must_use]
    pub fn hint(&self) -> Hint {
        if self.won {
            return Hint::None {
                words: "The game is over. Start a new one.".to_owned(),
            };
        }

        let mut best: Option<(i32, Selection, Pile, usize)> = None;
        let mut consider = |score: i32, selection: Selection, to: Pile, count: usize| {
            if best
                .as_ref()
                .is_none_or(|(best_score, ..)| score > *best_score)
            {
                best = Some((score, selection, to, count));
            }
        };

        for column in 0..COLUMNS {
            let pile = Pile::Tableau(column);
            let cards = &self.tableau[column];
            for index in cards.up..cards.len() {
                let Some(count) = self.run_len(pile, index) else {
                    continue;
                };
                let Some(card) = self.card_at(pile, index) else {
                    continue;
                };
                // Revealing a face-down card is the best thing a move can do
                // that is not a completed run.
                let mut base = 0;
                if index == cards.up && cards.up > 0 {
                    base += 600;
                }
                // A thirteen-card run is one card from home, and moving it onto
                // the one card that completes it is the move to find.
                if count == 13 {
                    base += 1_500;
                }
                for destination in 0..COLUMNS {
                    let to = Pile::Tableau(destination);
                    if to == pile || !self.accepts(to, card) {
                        continue;
                    }
                    let mut score = base;
                    match self.tableau[destination].top() {
                        Some(top) if self.card(top).suit == card.suit => score += 500,
                        Some(_) => score += 100,
                        None => score -= 400,
                    }
                    if count == 13 && self.tableau[destination].len() + count == 13 {
                        score += 600;
                    }
                    consider(score, Selection { pile, index }, to, count);
                }
            }
        }

        if let Some((score, selection, to, count)) = best {
            if score >= 0 {
                return Hint::Move {
                    from: selection.pile,
                    index: selection.index,
                    to,
                    count,
                    words: self.move_sentence(selection, to, count),
                };
            }
        }
        if !self.stock.is_empty() && self.can_deal() {
            return Hint::Deal {
                words: "Deal a card to every column.".to_owned(),
            };
        }
        if !self.stock.is_empty() {
            return Hint::None {
                words: "Fill an empty column, then deal.".to_owned(),
            };
        }
        if let Some((_, selection, to, count)) = best {
            return Hint::Move {
                from: selection.pile,
                index: selection.index,
                to,
                count,
                words: self.move_sentence(selection, to, count),
            };
        }
        Hint::None {
            words: "No moves left. Undo, or start a new game.".to_owned(),
        }
    }

    /// The value stored for this game, or `None` when there is nothing worth
    /// keeping.
    #[must_use]
    pub fn to_stored(&self) -> Option<String> {
        if !self.started && self.moves == 0 && self.elapsed == 0 && !self.won {
            return None;
        }
        let mut out = String::new();
        let _ = write!(
            out,
            "v1|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.difficulty.slug(),
            self.scoring.slug(),
            self.seed,
            self.score,
            self.moves,
            self.elapsed,
            u8::from(self.won),
            u8::from(self.started),
            self.encode_cards(&self.stock),
        );
        for column in &self.tableau {
            let _ = write!(out, "|{}:{}", column.up, self.encode_cards(&column.cards));
        }
        out.push('|');
        for suit in &self.completed {
            out.push(suit.code());
        }
        Some(out)
    }

    /// Read a stored game back, or `None` if anything about it is unreadable.
    ///
    /// A corrupt entry must never stop the app from starting, and a half-valid
    /// one must never put a card in two columns. Every difficulty holds several
    /// identical cards, so the check is a multiset one: each code is matched to
    /// the first *unused* copy in the deck, and at the end every card must have
    /// been placed exactly once.
    #[must_use]
    pub fn from_stored(raw: Option<&str>) -> Option<Self> {
        let raw = raw?;
        let fields: Vec<&str> = raw.split('|').collect();
        if fields.len() != STORED_FIELDS || fields[0] != "v1" {
            return None;
        }
        let difficulty = Difficulty::from_slug(fields[1])?;
        let scoring = Scoring::from_slug(fields[2])?;
        let seed = fields[3].parse::<u64>().ok()?;
        let score = fields[4].parse::<i32>().ok()?;
        let moves = fields[5].parse::<u32>().ok()?;
        let elapsed = fields[6].parse::<u32>().ok()?.min(ELAPSED_LIMIT);
        let won = match fields[7] {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        let started = match fields[8] {
            "0" => false,
            "1" => true,
            _ => return None,
        };

        let cards = deck(difficulty);
        let mut used = vec![false; cards.len()];
        let mut decode = |text: &str| decode_cards(text, &cards, &mut used);
        let stock = decode(fields[9])?;
        let mut tableau: [Column; COLUMNS] = std::array::from_fn(|_| Column::new());
        for (index, column) in tableau.iter_mut().enumerate() {
            let (up, pile) = fields[10 + index].split_once(':')?;
            let up = up.parse::<usize>().ok()?;
            let cards_in_pile = decode(pile)?;
            if up > cards_in_pile.len() {
                return None;
            }
            column.cards = cards_in_pile;
            column.up = up;
        }
        let completed_text = fields[STORED_FIELDS - 1];
        let mut completed = Vec::with_capacity(RUNS);
        for code in completed_text.chars() {
            completed.push(Suit::from_code(code)?);
        }
        if completed.len() > RUNS {
            return None;
        }
        // A completed run has left the board, so its thirteen cards are in no
        // pile — and the format stores the run as its suit, because a run *is*
        // king down to ace of one suit. So the cards it took are marked here,
        // and only after every pile has claimed its own: a pile may hold any
        // copy of a card, and which copy a run used is not a fact about the
        // game. Missing one means the save is inconsistent, not just incomplete.
        for suit in &completed {
            for rank in Rank::ALL.into_iter().rev() {
                let card = Card::new(rank, *suit);
                let index = (0..cards.len()).find(|&index| cards[index] == card && !used[index])?;
                used[index] = true;
            }
        }
        if !used.iter().all(|card| *card) {
            return None;
        }
        let mut game = Self {
            cards,
            stock,
            tableau,
            completed,
            difficulty,
            scoring,
            seed,
            score: score.max(0),
            moves,
            elapsed,
            won,
            started,
            history: Vec::new(),
            selection: None,
        };
        // `won` is derived, not trusted: a save that says so with four runs on
        // the board would otherwise lock the page on a victory screen.
        game.won = game.completed.len() == RUNS;
        Some(game)
    }

    /// A column's name and top card, for a screen reader.
    #[must_use]
    pub fn pile_label(&self, pile: Pile) -> String {
        let len = self.pile_len(pile);
        if len == 0 {
            return format!("{}, empty", pile.name());
        }
        if pile == Pile::Stock {
            return format!(
                "{}, {} cards, {} deals",
                pile.name(),
                len,
                self.deals_left()
            );
        }
        match self.card_at(pile, len - 1) {
            Some(top) => format!("{}, {} cards, top card {}", pile.name(), len, top.name()),
            None => format!("{}, {} cards", pile.name(), len),
        }
    }

    /// The faint mark an empty column shows.
    #[must_use]
    pub fn slot_mark(&self, pile: Pile) -> &'static str {
        if !self.pile(pile).is_empty() {
            return "";
        }
        match pile {
            Pile::Stock if self.stock.is_empty() => "—",
            Pile::Stock => "↻",
            Pile::Tableau(_) => "",
        }
    }

    /// The run beginning at `index`, if it can be picked up as a unit.
    ///
    /// A run is a descending sequence in **one suit**, ending at the column's
    /// top card: that is the rule every serious implementation keeps, and it is
    /// what makes a run a thing worth seeing.
    #[must_use]
    pub fn run_len(&self, pile: Pile, index: usize) -> Option<usize> {
        self.run_len_inner(pile, index)
    }

    /// Put the held run back without moving it.
    pub fn clear_selection(&mut self) -> Report {
        if self.selection.take().is_some() {
            Report::quiet("Put back.")
        } else {
            Report::quiet("")
        }
    }

    fn run_len_inner(&self, pile: Pile, index: usize) -> Option<usize> {
        let Pile::Tableau(column) = pile else {
            return None;
        };
        let cards = &self.tableau[column];
        if index >= cards.len() || !cards.is_face_up(index) {
            return None;
        }
        for position in index..cards.len().saturating_sub(1) {
            let upper = self.card(cards.cards[position]);
            let lower = self.card(cards.cards[position + 1]);
            if upper.rank != lower.rank.up()? || upper.suit != lower.suit {
                return None;
            }
        }
        Some(cards.len() - index)
    }

    fn card_at(&self, pile: Pile, index: usize) -> Option<Card> {
        self.pile(pile).get(index).map(|&card| self.card(card))
    }

    fn accepts(&self, pile: Pile, card: Card) -> bool {
        let Pile::Tableau(column) = pile else {
            return false;
        };
        match self.tableau[column].top() {
            None => true,
            Some(top) => self.card(top).rank.value() == card.rank.value() + 1,
        }
    }

    fn can_move(&self, selection: Selection, to: Pile) -> bool {
        if !self.can_pick(selection.pile, selection.index) {
            return false;
        }
        let Some(card) = self.card_at(selection.pile, selection.index) else {
            return false;
        };
        self.accepts(to, card)
    }

    fn apply(&mut self, selection: Selection, to: Pile) -> Report {
        let Some(count) = self.run_len_inner(selection.pile, selection.index) else {
            self.selection = None;
            return Report::quiet("That card cannot move.");
        };
        if to == selection.pile {
            self.selection = None;
            return Report::quiet("");
        }
        let Some(card) = self.card_at(selection.pile, selection.index) else {
            return Report::quiet("That card cannot move.");
        };
        if !self.accepts(to, card) {
            return Report::quiet("That card cannot go there.");
        }

        self.push_history();
        let Pile::Tableau(from) = selection.pile else {
            return Report::quiet("");
        };
        let Pile::Tableau(destination) = to else {
            return Report::quiet("");
        };
        let moved = self.tableau[from].cards.split_off(selection.index);
        self.tableau[destination].cards.extend(moved);
        let mut words = if count == 1 {
            format!("Moved the {} to {}.", card.name(), to.name())
        } else {
            format!(
                "Moved the {} and {} more to {}.",
                card.name(),
                count - 1,
                to.name()
            )
        };
        if self.reveal(from) {
            words.push_str(" Turned over a new card.");
        }

        self.moves = self.moves.saturating_add(1);
        self.score = self.charge(self.score);
        self.started = true;
        self.selection = None;

        if let Some(completed) = self.collect_runs() {
            words.push(' ');
            words.push_str(&completed);
        }
        if self.completed.len() == RUNS {
            self.won = true;
            words = self.result_words();
            return Report { won: true, words };
        }
        Report::quiet(words)
    }

    /// Turn over a column's newly exposed top card, reporting whether it did.
    fn reveal(&mut self, column: usize) -> bool {
        let column = &mut self.tableau[column];
        if !column.cards.is_empty() && column.up == column.cards.len() {
            column.up -= 1;
            true
        } else {
            false
        }
    }

    /// Lift every complete run off the board, and say what happened.
    ///
    /// A run is thirteen face-up cards from king down to ace in one suit. More
    /// than one can complete at once — a deal can finish two columns together —
    /// so this loops until a pass finds nothing.
    fn collect_runs(&mut self) -> Option<String> {
        let mut lifted: Vec<Suit> = Vec::new();
        loop {
            let mut found = None;
            for column in 0..COLUMNS {
                let column_cards = &self.tableau[column];
                let len = column_cards.len();
                if len < 13 || column_cards.up > len - 13 {
                    continue;
                }
                let start = len - 13;
                let suit = self.card(column_cards.cards[start]).suit;
                let complete = (start..len).all(|position| {
                    let card = self.card(column_cards.cards[position]);
                    card.suit == suit
                        && card.rank.value() == u8::try_from(len - position).unwrap_or(0)
                });
                if complete {
                    found = Some((column, suit));
                    break;
                }
            }
            let Some((column, suit)) = found else {
                break;
            };
            let pile = &mut self.tableau[column];
            pile.cards.truncate(pile.cards.len() - 13);
            if pile.up > pile.cards.len() {
                pile.up = pile.cards.len();
            }
            self.reveal(column);
            self.completed.push(suit);
            lifted.push(suit);
            if self.scoring == Scoring::Standard {
                self.score = self.score.saturating_add(100);
            }
        }
        if lifted.is_empty() {
            return None;
        }
        let names: Vec<String> = lifted
            .iter()
            .map(|suit| format!("{} of {}", Rank::King.name(), suit.name()))
            .collect();
        Some(format!(
            "Completed a run of {}!",
            names.join(" and a run of ")
        ))
    }

    /// Charge one point for an action, in the standard table. No score means no
    /// arithmetic at all.
    fn charge(&self, score: i32) -> i32 {
        match self.scoring {
            Scoring::Standard => (score - 1).max(0),
            Scoring::Off => score,
        }
    }

    fn push_history(&mut self) {
        if self.history.len() == HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.history.push(Snapshot {
            tableau: self.tableau.clone(),
            stock: self.stock.clone(),
            completed: self.completed.clone(),
            score: self.score,
            moves: self.moves,
            elapsed: self.elapsed,
            won: self.won,
        });
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.tableau = snapshot.tableau;
        self.stock = snapshot.stock;
        self.completed = snapshot.completed;
        self.score = snapshot.score;
        self.moves = snapshot.moves;
        self.elapsed = snapshot.elapsed;
        self.won = snapshot.won;
    }

    fn move_sentence(&self, selection: Selection, to: Pile, count: usize) -> String {
        let Some(card) = self.card_at(selection.pile, selection.index) else {
            return "Try another move.".to_owned();
        };
        if count == 1 {
            format!("Try the {} to {}.", card.name(), to.name())
        } else {
            format!(
                "Try the {} and {} more to {}.",
                card.name(),
                count - 1,
                to.name()
            )
        }
    }

    fn encode_cards(&self, cards: &[u16]) -> String {
        let mut out = String::with_capacity(cards.len() * 2);
        for &card in cards {
            out.push_str(&self.card(card).code());
        }
        out
    }
}

/// The deck for a difficulty: every rank of every suit in play, repeated as
/// many times as that difficulty needs.
///
/// Always 104 cards. One suit is eight copies of a single suit's thirteen
/// ranks; two suits are four copies of two; four suits are the ordinary pair of
/// decks.
#[must_use]
pub fn deck(difficulty: Difficulty) -> Vec<Card> {
    let mut cards = Vec::with_capacity(104);
    for _ in 0..difficulty.copies() {
        for &suit in difficulty.suits() {
            for rank in Rank::ALL {
                cards.push(Card::new(rank, suit));
            }
        }
    }
    cards
}

/// A pile parsed from a stored field.
///
/// Several copies of the same card are legal — one-suit Spider has eight aces of
/// spades — so a code is matched to the first copy that has not been placed
/// yet. A code that matches nothing left is a corrupt save, not a new card.
fn decode_cards(text: &str, cards: &[Card], used: &mut [bool]) -> Option<Vec<u16>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut pile = Vec::with_capacity(text.len() / 2);
    for chunk in text.as_bytes().chunks(2) {
        let code = std::str::from_utf8(chunk).ok()?;
        let card = Card::from_code(code)?;
        let index = (0..cards.len()).find(|&index| cards[index] == card && !used[index])?;
        used[index] = true;
        pile.push(u16::try_from(index).ok()?);
    }
    Some(pile)
}

/// `m:ss`, or `h:mm:ss` once the clock passes an hour.
#[must_use]
pub fn clock_text(seconds: u32) -> String {
    let displayed_seconds = seconds % 60;
    let minutes = (seconds / 60) % 60;
    let hours = seconds / 3_600;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{displayed_seconds:02}")
    } else {
        format!("{minutes}:{displayed_seconds:02}")
    }
}

/// Thousands separators, because a score is read at a glance.
#[must_use]
pub fn grouped(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let digits = value.abs().to_string();
    let mut out = String::with_capacity(digits.len() + 2);
    out.push_str(sign);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A small, deterministic generator. The seed is the deal, so a game can be
/// restarted, saved and shared without storing 104 cards.
fn shuffle(cards: &mut [u16], seed: u64) {
    let mut state = seed;
    for index in (1..cards.len()).rev() {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        let pick = usize::try_from(z % (index as u64 + 1)).unwrap_or(0);
        cards.swap(index, pick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_game(difficulty: Difficulty, scoring: Scoring) -> Game {
        let mut game = Game::new(difficulty, scoring, 7);
        game.stock.clear();
        for column in &mut game.tableau {
            column.cards.clear();
            column.up = 0;
        }
        game
    }

    /// The deck index of the `nth` copy of a card.
    fn id(game: &Game, rank: Rank, suit: Suit, nth: usize) -> u16 {
        let card = Card::new(rank, suit);
        game.cards
            .iter()
            .enumerate()
            .filter(|(_, known)| **known == card)
            .nth(nth)
            .map(|(index, _)| u16::try_from(index).expect("a deck index fits a u16"))
            .expect("the deck holds that copy")
    }

    fn put_column(game: &mut Game, column: usize, cards: &[u16], up: usize) {
        game.tableau[column].cards = cards.to_vec();
        game.tableau[column].up = up;
    }

    #[test]
    fn every_difficulty_deals_the_same_number_of_cards() {
        for difficulty in Difficulty::ALL {
            let game = Game::new(difficulty, Scoring::Standard, 3);
            assert_eq!(game.deck_len(), 104, "{difficulty:?}");
            assert_eq!(game.stock.len(), 50, "{difficulty:?}");
            assert_eq!(game.deals_left(), 5, "{difficulty:?}");
            let total: usize =
                game.tableau.iter().map(Column::len).sum::<usize>() + game.stock.len();
            assert_eq!(total, 104);
            for (index, column) in game.tableau.iter().enumerate() {
                assert_eq!(column.len(), if index < 4 { 6 } else { 5 });
                assert_eq!(column.up, column.len() - 1, "only the top card is face up");
            }
            // One suit is all spades; four suits holds all four.
            let suits: std::collections::BTreeSet<Suit> =
                game.cards.iter().map(|card| card.suit).collect();
            assert_eq!(suits.len(), difficulty.suit_count(), "{difficulty:?}");
        }
    }

    #[test]
    fn one_suit_deals_eight_of_each_card_and_four_suits_deals_two() {
        for difficulty in Difficulty::ALL {
            let game = Game::new(difficulty, Scoring::Standard, 1);
            let ace = Card::new(Rank::Ace, difficulty.suits()[0]);
            let copies = game.cards.iter().filter(|card| **card == ace).count();
            assert_eq!(copies, difficulty.copies(), "{difficulty:?}");
        }
    }

    #[test]
    fn the_same_seed_deals_the_same_game() {
        let one = Game::new(Difficulty::Hard, Scoring::Standard, 99);
        let two = Game::new(Difficulty::Hard, Scoring::Standard, 99);
        let other = Game::new(Difficulty::Hard, Scoring::Standard, 98);
        let shape = |game: &Game| {
            game.tableau
                .iter()
                .map(|column| column.cards.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&one), shape(&two));
        assert_ne!(shape(&one), shape(&other));
    }

    #[test]
    fn only_an_in_suit_descending_run_lifts_as_a_group() {
        let mut game = empty_game(Difficulty::Hard, Scoring::Standard);
        let suited = [
            id(&game, Rank::Nine, Suit::Spades, 0),
            id(&game, Rank::Eight, Suit::Spades, 0),
            id(&game, Rank::Seven, Suit::Spades, 0),
        ];
        put_column(&mut game, 0, &suited, 0);
        assert_eq!(game.run_len(Pile::Tableau(0), 0), Some(3));
        assert_eq!(game.run_len(Pile::Tableau(0), 1), Some(2));
        assert_eq!(game.run_len(Pile::Tableau(0), 2), Some(1));

        let mixed = [
            id(&game, Rank::Nine, Suit::Spades, 1),
            id(&game, Rank::Eight, Suit::Hearts, 0),
        ];
        put_column(&mut game, 1, &mixed, 0);
        assert_eq!(game.run_len(Pile::Tableau(1), 0), None, "mixed suits");
        assert_eq!(game.run_len(Pile::Tableau(1), 1), Some(1));

        // A gap in rank breaks it just as surely as a change of suit.
        let gap = [
            id(&game, Rank::Nine, Suit::Hearts, 1),
            id(&game, Rank::Six, Suit::Hearts, 0),
        ];
        put_column(&mut game, 2, &gap, 0);
        assert_eq!(game.run_len(Pile::Tableau(2), 0), None);
        // And a face-down card cannot be picked up at all.
        let hidden = [
            id(&game, Rank::King, Suit::Clubs, 0),
            id(&game, Rank::Queen, Suit::Clubs, 0),
        ];
        put_column(&mut game, 3, &hidden, 1);
        assert_eq!(game.run_len(Pile::Tableau(3), 0), None);
        assert_eq!(game.run_len(Pile::Tableau(3), 1), Some(1));
    }

    #[test]
    fn any_card_lies_on_the_next_rank_up_whatever_its_suit() {
        let mut game = empty_game(Difficulty::Hard, Scoring::Standard);
        let nine = [id(&game, Rank::Nine, Suit::Spades, 0)];
        put_column(&mut game, 0, &nine, 0);
        // Any suit, as long as the rank is one lower.
        assert!(game.accepts(Pile::Tableau(0), Card::new(Rank::Eight, Suit::Hearts)));
        assert!(game.accepts(Pile::Tableau(0), Card::new(Rank::Eight, Suit::Spades)));
        assert!(!game.accepts(Pile::Tableau(0), Card::new(Rank::Nine, Suit::Hearts)));
        assert!(!game.accepts(Pile::Tableau(0), Card::new(Rank::Seven, Suit::Hearts)));
        // An empty column takes anything.
        assert!(game.accepts(Pile::Tableau(1), Card::new(Rank::Seven, Suit::Hearts)));
    }

    #[test]
    fn a_completed_run_is_lifted_and_scores_a_hundred() {
        let mut game = empty_game(Difficulty::Easy, Scoring::Standard);
        let opening = game.score;
        let mut cards = Vec::new();
        for rank in Rank::ALL.into_iter().rev() {
            cards.push(id(&game, rank, Suit::Spades, 0));
        }
        // King down to two on the column, and the ace on top of another column
        // so the move that finishes the run is a real move.
        let ace = [cards[12]];
        put_column(&mut game, 0, &cards[..12], 0);
        put_column(&mut game, 1, &ace, 0);
        game.tap(Pile::Tableau(1), 0);
        let report = game.tap(Pile::Tableau(0), 12);
        assert!(report.words.contains("Completed a run"), "{}", report.words);
        assert_eq!(game.runs(), 1);
        assert_eq!(game.pile_len(Pile::Tableau(0)), 0);
        assert_eq!(game.score(), opening + 100 - 1);
        assert_eq!(game.completed_suits(), [Suit::Spades]);
    }

    #[test]
    fn a_run_is_not_lifted_while_a_card_of_another_suit_is_in_it() {
        let mut game = empty_game(Difficulty::Hard, Scoring::Standard);
        let mut cards = Vec::new();
        for (index, rank) in Rank::ALL.into_iter().rev().enumerate() {
            let suit = if index == 5 {
                Suit::Hearts
            } else {
                Suit::Spades
            };
            cards.push(id(&game, rank, suit, 0));
        }
        put_column(&mut game, 0, &cards, 0);
        assert_eq!(game.collect_runs(), None);
        assert_eq!(game.runs(), 0);
    }

    #[test]
    fn a_deal_gives_every_column_one_card_and_needs_no_empty_column() {
        let mut game = Game::new(Difficulty::Easy, Scoring::Standard, 5);
        assert!(game.can_deal());
        let before: Vec<usize> = game.tableau.iter().map(Column::len).collect();
        let report = game.deal();
        assert!(report.words.contains("Dealt"), "{}", report.words);
        for (index, column) in game.tableau.iter().enumerate() {
            assert_eq!(column.len(), before[index] + 1);
            assert!(
                column.is_face_up(column.len() - 1),
                "the new card is face up"
            );
        }
        assert_eq!(game.deals_left(), 4);

        // An empty column blocks the stock, and says so.
        game.tableau[3].cards.clear();
        game.tableau[3].up = 0;
        assert!(!game.can_deal());
        let refused = game.deal();
        assert!(refused.words.contains("Fill every empty column"));
    }

    #[test]
    fn the_stock_empties_after_five_deals() {
        let mut game = Game::new(Difficulty::Easy, Scoring::Standard, 11);
        for _ in 0..5 {
            game.deal();
        }
        assert_eq!(game.deals_left(), 0);
        assert!(game.stock.is_empty());
        let refused = game.deal();
        assert!(refused.words.contains("stock is empty"));
    }

    #[test]
    fn moving_a_run_reveals_the_card_underneath() {
        let mut game = empty_game(Difficulty::Easy, Scoring::Standard);
        let cards = [
            id(&game, Rank::King, Suit::Spades, 0),
            id(&game, Rank::Four, Suit::Spades, 0),
            id(&game, Rank::Three, Suit::Spades, 0),
        ];
        let five = [id(&game, Rank::Five, Suit::Spades, 0)];
        put_column(&mut game, 0, &cards, 1);
        put_column(&mut game, 1, &five, 0);
        assert!(!game.tableau[0].is_face_up(0));
        game.tap(Pile::Tableau(0), 1);
        let report = game.tap(Pile::Tableau(1), 1);
        assert!(report.words.contains("Turned over"), "{}", report.words);
        assert!(game.tableau[0].is_face_up(0));
        assert_eq!(game.pile_len(Pile::Tableau(1)), 3);
    }

    #[test]
    fn undo_restores_the_board_and_charges_for_itself() {
        let mut game = empty_game(Difficulty::Easy, Scoring::Standard);
        let four = [id(&game, Rank::Four, Suit::Spades, 0)];
        let five = [id(&game, Rank::Five, Suit::Spades, 0)];
        put_column(&mut game, 0, &four, 0);
        put_column(&mut game, 1, &five, 0);
        game.tap(Pile::Tableau(0), 0);
        game.tap(Pile::Tableau(1), 1);
        assert_eq!(game.pile_len(Pile::Tableau(0)), 0);
        assert_eq!(game.score(), 499, "500 less the move");
        let report = game.undo();
        assert!(report.words.contains("Undid"));
        assert_eq!(game.pile_len(Pile::Tableau(0)), 1);
        assert_eq!(
            game.score(),
            498,
            "the move's point stays, and the undo adds one of its own"
        );
        assert_eq!(game.moves(), 0, "the undone move is not on the board");
        game.undo();
        assert!(game.undo().words.contains("Nothing to undo"));
    }

    #[test]
    fn no_score_means_no_arithmetic_at_all() {
        let mut game = empty_game(Difficulty::Easy, Scoring::Off);
        let four = [id(&game, Rank::Four, Suit::Spades, 0)];
        let five = [id(&game, Rank::Five, Suit::Spades, 0)];
        put_column(&mut game, 0, &four, 0);
        put_column(&mut game, 1, &five, 0);
        game.tap(Pile::Tableau(0), 0);
        game.tap(Pile::Tableau(1), 1);
        assert_eq!(game.score(), 0);
        game.undo();
        assert_eq!(game.score(), 0);
        assert!(game.readout().score.is_none());
    }

    #[test]
    fn the_double_tap_prefers_the_same_suit() {
        // Two suits, because the choice being tested is between two suits.
        let mut game = empty_game(Difficulty::Hard, Scoring::Standard);
        let four = [id(&game, Rank::Four, Suit::Spades, 0)];
        let five = [id(&game, Rank::Five, Suit::Spades, 0)];
        let wrong = [id(&game, Rank::Five, Suit::Hearts, 0)];
        put_column(&mut game, 0, &four, 0);
        put_column(&mut game, 1, &five, 0);
        put_column(&mut game, 2, &wrong, 0);
        game.double_tap(Pile::Tableau(0), 0);
        assert_eq!(game.pile_len(Pile::Tableau(0)), 0);
        assert_eq!(game.pile_len(Pile::Tableau(1)), 2, "the same suit wins");
    }

    #[test]
    fn a_stored_game_round_trips_with_its_duplicates_intact() {
        for difficulty in Difficulty::ALL {
            let mut game = Game::dealt(difficulty, Scoring::Standard, 21);
            game.deal();
            game.deal();
            let stored = game.to_stored().expect("a played game is stored");
            let restored = Game::from_stored(Some(&stored)).expect("the save parses");
            assert_eq!(restored.difficulty(), difficulty);
            assert_eq!(restored.seed(), 21);
            let printed_cards = |game: &Game, cards: &[u16]| -> Vec<Card> {
                cards.iter().map(|&id| game.card(id)).collect()
            };
            assert_eq!(
                printed_cards(&restored, &restored.stock),
                printed_cards(&game, &game.stock)
            );
            assert_eq!(restored.score, game.score);
            assert_eq!(restored.deals_left(), game.deals_left());
            // Compared as printed cards, not as deck indexes: one-suit Spider
            // holds eight identical copies of every card, and which copy a pile
            // names is an implementation detail of the rebuild, not a fact
            // about the game.
            let printed = |game: &Game, column: usize| -> Vec<Card> {
                game.tableau[column]
                    .cards
                    .iter()
                    .map(|&id| game.card(id))
                    .collect()
            };
            for column in 0..COLUMNS {
                assert_eq!(
                    printed(&restored, column),
                    printed(&game, column),
                    "{difficulty:?} column {column}"
                );
                assert_eq!(restored.tableau[column].up, game.tableau[column].up);
            }
            assert_eq!(
                restored.to_stored().as_deref(),
                Some(stored.as_str()),
                "{difficulty:?} does not survive a round trip byte for byte"
            );
        }
    }

    /// The one case the format has to handle and a naive check misses: a run has
    /// been lifted, so thirteen cards are in no pile at all. The save stores the
    /// run as its suit, and loading has to account for the cards that suit took.
    #[test]
    fn a_save_with_a_completed_run_loads_and_keeps_its_score() {
        let mut game = empty_game(Difficulty::Medium, Scoring::Standard);
        let mut cards = Vec::new();
        for rank in Rank::ALL.into_iter().rev() {
            cards.push(id(&game, rank, Suit::Spades, 0));
        }
        let ace = [cards[12]];
        put_column(&mut game, 0, &cards[..12], 0);
        put_column(&mut game, 1, &ace, 0);
        // Every other card in the stock, so the board is a whole deck.
        let placed: Vec<u16> = cards.clone();
        game.stock = (0..game.cards.len() as u16)
            .filter(|id| !placed.contains(id))
            .collect();
        game.tap(Pile::Tableau(1), 0);
        let report = game.tap(Pile::Tableau(0), 12);
        assert!(report.words.contains("Completed a run"), "{}", report.words);
        assert_eq!(game.runs(), 1);

        let stored = game
            .to_stored()
            .expect("a game with a run lifted is stored");
        let restored = Game::from_stored(Some(&stored)).expect("and it loads again");
        assert_eq!(restored.runs(), 1);
        assert_eq!(restored.completed_suits(), [Suit::Spades]);
        assert_eq!(restored.score(), game.score());
        assert_eq!(restored.stock.len(), game.stock.len());
        assert_eq!(restored.to_stored().as_deref(), Some(stored.as_str()));
    }

    #[test]
    fn corrupt_or_incomplete_saves_are_refused() {
        let game = Game::dealt(Difficulty::Easy, Scoring::Standard, 4);
        let stored = game.to_stored().expect("stored");
        let duplicate = {
            let mut fields: Vec<String> = stored.split('|').map(str::to_owned).collect();
            // Copy a tableau card over the stock's first card: a duplicate in
            // one-suit Spider is legal, and this is the case that has to fail.
            let tableau_card = fields[10]
                .split_once(':')
                .map(|(_, cards)| cards[..2].to_owned())
                .unwrap_or_default();
            fields[9] = format!("{tableau_card}{}", &fields[9][2..]);
            fields.join("|")
        };
        let dropped = {
            let mut fields: Vec<String> = stored.split('|').map(str::to_owned).collect();
            fields[9] = fields[9].chars().skip(2).collect();
            fields.join("|")
        };
        let claims_a_run = {
            let mut fields: Vec<String> = stored.split('|').map(str::to_owned).collect();
            fields[STORED_FIELDS - 1] = "S".to_owned();
            fields.join("|")
        };
        for bad in [
            String::new(),
            "v1".to_owned(),
            stored.replace("v1|", "v2|"),
            stored.replace("|easy|", "|nonsense|"),
            stored.replace("|standard|4|", "|standard|x|"),
            duplicate,
            dropped,
            claims_a_run,
            format!("{stored}|extra"),
        ] {
            assert!(
                Game::from_stored(Some(&bad)).is_none(),
                "accepted a corrupt save: {bad}"
            );
        }
        assert!(Game::from_stored(None).is_none());
    }

    /// Every action the rules allow, checked from the outside: the deck is
    /// conserved, no face-up card escapes its run, and the score never goes
    /// below zero.
    fn invariants(game: &Game) {
        let mut seen = vec![0u8; game.deck_len()];
        for pile in Pile::ALL {
            for &id in game.pile(pile) {
                seen[id as usize] += 1;
            }
        }
        assert!(
            seen.iter().all(|count| *count == 1),
            "a card was lost or duplicated: {seen:?}"
        );
        let on_board: usize = game.tableau.iter().map(Column::len).sum();
        assert_eq!(
            on_board + game.stock.len() + game.runs() * 13,
            game.deck_len(),
            "the board, the stock and the completed runs must account for every card"
        );
        assert!(game.score() >= 0, "{}", game.score());
        // Every face-down card sits below every face-up one, and whatever a
        // column holds, its face-up stretch may be lifted from its top card.
        // A column is *not* descending all the way down: a deal puts an
        // arbitrary card on top of each of the ten, and that card is exactly
        // what the next move has to deal with.
        for column in 0..COLUMNS {
            let pile = Pile::Tableau(column);
            let len = game.pile_len(pile);
            let mut seen_face_up = false;
            for index in 0..len {
                let down = game.is_face_down(pile, index);
                assert!(
                    !(seen_face_up && down),
                    "column {column} has a face-down card above a face-up one at {index}"
                );
                seen_face_up |= !down;
            }
            if len > 0 && seen_face_up {
                assert!(
                    game.run_len(pile, len - 1).is_some(),
                    "the top card of column {column} is not liftable"
                );
            }
        }
    }

    /// A play-through of random legal actions over many deals.
    #[test]
    fn random_play_keeps_the_deck_conserved() {
        for seed in 0..12u64 {
            for difficulty in Difficulty::ALL {
                let mut game = Game::dealt(difficulty, Scoring::Standard, seed);
                invariants(&game);
                let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(7);
                for _ in 0..250 {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    let pick = usize::try_from(state >> 33).unwrap_or(0);
                    match pick % 10 {
                        0 => {
                            game.deal();
                        }
                        1 => {
                            game.undo();
                        }
                        2 => {
                            let column = (pick / 7) % COLUMNS;
                            let pile = Pile::Tableau(column);
                            let len = game.pile_len(pile);
                            if len > 0 {
                                let index = (pick / 3) % len;
                                game.double_tap(pile, index);
                            }
                        }
                        _ => {
                            let column = (pick / 11) % COLUMNS;
                            let pile = Pile::Tableau(column);
                            let len = game.pile_len(pile);
                            if len == 0 {
                                continue;
                            }
                            let index = (pick / 5) % len;
                            if game.run_len(pile, index).is_none() {
                                continue;
                            }
                            game.tap(pile, index);
                            if pick % 4 == 0 {
                                game.double_tap(pile, index);
                            } else {
                                let target = (pick / 13) % COLUMNS;
                                game.tap(
                                    Pile::Tableau(target),
                                    game.pile_len(Pile::Tableau(target)),
                                );
                            }
                        }
                    }
                    invariants(&game);
                    if game.won() {
                        break;
                    }
                }
            }
        }
    }

    /// A hint that says "do this" must actually be doable.
    #[test]
    fn every_hint_is_a_move_that_works() {
        for seed in 0..20u64 {
            for difficulty in [Difficulty::Easy, Difficulty::Hard] {
                let mut game = Game::dealt(difficulty, Scoring::Standard, seed);
                for _ in 0..40 {
                    match game.hint() {
                        Hint::Move {
                            from, index, to, ..
                        } => {
                            let before = game.pile_len(from);
                            let count = game.run_len(from, index).expect("a hinted run is a run");
                            game.tap(from, index);
                            game.tap(to, game.pile_len(to));
                            assert_eq!(
                                game.pile_len(from),
                                before - count,
                                "the hinted move did not happen"
                            );
                        }
                        Hint::Deal { .. } => {
                            assert!(game.can_deal(), "a deal hint that cannot be dealt");
                            game.deal();
                        }
                        Hint::None { .. } => break,
                    }
                    invariants(&game);
                }
            }
        }
    }

    #[test]
    fn options_round_trip_and_fall_back() {
        let options = Options {
            difficulty: Difficulty::Hard,
            scoring: Scoring::Off,
        };
        let stored = options.to_stored();
        assert_eq!(Options::from_stored(Some(&stored)), options);
        assert_eq!(Options::from_stored(Some("nonsense")), Options::default());
        assert_eq!(Options::from_stored(None), Options::default());
        assert!(options.summary().contains("four suits"));
    }

    #[test]
    fn the_clock_and_the_score_are_formatted_for_reading() {
        assert_eq!(clock_text(0), "0:00");
        assert_eq!(clock_text(61), "1:01");
        assert_eq!(clock_text(3_599), "59:59");
        assert_eq!(clock_text(3_600), "1:00:00");
        assert_eq!(grouped(1_204), "1,204");
        assert_eq!(grouped(500), "500");
    }

    #[test]
    fn an_untouched_deal_leaves_no_trace() {
        let game = Game::new(Difficulty::Medium, Scoring::Standard, 8);
        assert!(!game.started());
        assert_eq!(game.to_stored(), None);
        assert!(
            Game::dealt(Difficulty::Medium, Scoring::Standard, 8)
                .to_stored()
                .is_some(),
            "a deal the player asked for is worth keeping"
        );
    }
}
