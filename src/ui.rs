//! The browser layer: the only file that talks to the DOM.
//!
//! Every decision — which cards a run may contain, what completes a run, what
//! the score becomes, what can be undone — was made in [`crate::game`] and is
//! unit tested there. What is left here is element handles, geometry, listeners
//! and timers.
//!
//! Three rules keep this file from becoming the game:
//!
//! * **The board is built once and then only described.** The 104 cards and the
//!   eleven places are elements with stable identities; a render moves them and
//!   rewrites their description. Rebuilding the board would throw away focus and
//!   kill a drag in progress, and the CSS transition that makes a card fly to
//!   its column depends on the element surviving the move.
//! * **One listener per board, not one per card.** A click is read off the
//!   element it landed on, and the element says which pile and position it is.
//! * **The clock redraws the clock.** A passing second changes one line, so
//!   [`App::tick`] paints the facts and leaves the board alone.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    Document, Element, Event, HtmlButtonElement, HtmlInputElement, HtmlSelectElement,
    KeyboardEvent, MouseEvent, PointerEvent, Storage, Window,
};

use crate::game::{
    Difficulty, Game, Hint, Options, Pile, Report, Scoring, COLUMNS, OPTIONS_KEY, RUNS, STORAGE_KEY,
};

/// The app's whole mutable state, and the element handles it writes through.
type Shared = Rc<RefCell<App>>;

/// The scope this app's worker is registered for.
const SCOPE: &str = "./";

/// How often the clock is checked. It is a check, not a tick: the elapsed time
/// comes from the wall clock, so a tab the browser throttled still catches up.
const TICK_MS: i32 = 1_000;

/// How far a finger may drift, in CSS pixels, before a press becomes a drag.
const DRAG_SLOP: f64 = 6.0;

/// How close two taps must be, in milliseconds, to count as a double tap.
const DOUBLE_TAP_MS: f64 = 320.0;

/// How long a hint stays lit.
const HINT_MS: i32 = 2_600;

/// The deck's aspect ratio: height over width. Every measurement inside a face
/// is a multiple of the width, so this one number fixes the geometry.
const ASPECT: f64 = 1.45;

/// The gap between the bottom of one face-down card and the top of the next,
/// in card widths.
const DOWN_FAN: f64 = 0.13;

/// The same for face-up cards, which need to show their rank.
const UP_FAN: f64 = 0.30;

/// The gap between the top row and the columns, in card widths.
const ROW_GAP: f64 = 0.13;

/// Below this card width, the face switches to its compact form: the centre pip
/// goes, and the rank in the corner grows.
///
/// The number is a measurement rather than a preference. A rank's corner is
/// about a fifth of the card's width on the normal face; under roughly 60px that
/// corner is under 12px of type, which is where a printed card stops being
/// legible at arm's length. So the compact face exists exactly below it.
const COMPACT_BELOW: f64 = 60.0;

/// The smallest the cards may shrink to before the fan compresses instead.
///
/// Ten columns share the width, so this is already a small card on a phone; the
/// floor is what stops a long game from shrinking the whole board into a grid
/// of unreadable squares.
const MIN_SCALE: f64 = 0.56;

/// Start the app.
///
/// Runs automatically when the page's dynamic import resolves, because the
/// loader in `ui.html` is `import('./app.js').then(m => m.default())`.
///
/// # Errors
///
/// Fails only when there is no window, no document, or no board in the shell.
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let board = document
        .get_element_by_id("board")
        .ok_or_else(|| JsValue::from_str("no #board"))?;
    let wrap = board
        .parent_element()
        .ok_or_else(|| JsValue::from_str("no board wrap"))?;

    board.set_class_name("instant");
    let shared: Shared = Rc::new(RefCell::new(App::build(&window, &document, board, wrap)?));
    {
        let mut app = shared.borrow_mut();
        app.paint();
        if app.game.won() {
            app.show_win();
        }
    }
    bind(&window, &document, &shared);
    observe_resize(&window, &shared);
    start_clock(&window, &shared);
    release_instant_layout(&window, &shared);

    // By value: the future outlives this function, so it has to own what it
    // holds. The same rule the event closures follow.
    let offline = window.clone();
    spawn_local(keep_offline(offline));
    Ok(())
}

/// Everything one running app holds.
struct App {
    window: Window,
    document: Document,
    board: Element,
    wrap: Element,
    game: Game,
    options: Options,
    /// One button per physical card, indexed by the deck's own index. Spider
    /// holds up to 104 cards, several of them identical, so the index is the
    /// only thing that identifies one.
    cards: Vec<HtmlButtonElement>,
    /// One place per pile, in `Pile::ALL` order.
    slots: Vec<(Pile, HtmlButtonElement)>,
    /// The strip of completed runs, and the badge on the stock.
    runs: Option<Element>,
    badge: Option<Element>,
    geometry: Geometry,
    focus: Focus,
    drag: Option<Drag>,
    last_tap: Option<Tap>,
    hint: Option<Hint>,
    hint_token: u32,
    /// Fractional seconds measured but not yet handed to the game.
    pending: Cell<f64>,
    /// The elapsed time at which the saved game is written again.
    next_save: Cell<u32>,
    /// True until one frame after the first paint.
    ///
    /// The board carries `.instant` while it is set, which turns every card
    /// transition off: the deal arrives on the table instead of a hundred cards
    /// flying in from the top-left corner. It is a flag rather than a class the
    /// layout writes, because the density class lives on the same element and
    /// the two would otherwise overwrite each other.
    instant: Cell<bool>,
}

/// Where the keyboard is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Focus {
    pile: Pile,
    /// `None` is the empty column itself.
    index: Option<usize>,
}

/// The last tap, for recognising a double tap without relying on `dblclick`
/// (which a touch screen does not reliably send).
#[derive(Debug, Clone, Copy)]
struct Tap {
    pile: Pile,
    index: usize,
    at: f64,
}

/// A press that may become a drag.
#[derive(Debug, Clone)]
struct Drag {
    id: i32,
    pile: Pile,
    index: usize,
    start_x: f64,
    start_y: f64,
    x: f64,
    y: f64,
    active: bool,
    /// The cards lifted, in pile order.
    cards: Vec<u16>,
}

/// The board's layout for the current size, in board-local pixels.
#[derive(Debug, Clone)]
struct Geometry {
    cw: f64,
    ch: f64,
    /// A position per deck index.
    positions: Vec<(f64, f64)>,
    /// A position per pile, in `Pile::ALL` order.
    slots: Vec<(f64, f64)>,
    /// The completed-runs strip: left, top, width.
    runs: (f64, f64, f64),
}

impl Geometry {
    fn empty(deck: usize) -> Self {
        Self {
            cw: 0.0,
            ch: 0.0,
            positions: vec![(0.0, 0.0); deck],
            slots: vec![(0.0, 0.0); Pile::ALL.len()],
            runs: (0.0, 0.0, 0.0),
        }
    }
}

impl App {
    fn build(
        window: &Window,
        document: &Document,
        board: Element,
        wrap: Element,
    ) -> Result<Self, JsValue> {
        let stored_options = read(window, OPTIONS_KEY);
        let options = stored_options
            .as_deref()
            .map(|text| Options::from_stored(Some(text)));

        let stored_game = read(window, STORAGE_KEY);
        let game = Game::from_stored(stored_game.as_deref());
        let options = match (options, &game) {
            (Some(options), _) => options,
            (None, Some(game)) => Options {
                difficulty: game.difficulty(),
                scoring: game.scoring(),
            },
            (None, None) => Options::default(),
        };
        let game =
            game.unwrap_or_else(|| Game::new(options.difficulty, options.scoring, fresh_seed()));

        let mut app = Self {
            window: window.clone(),
            document: document.clone(),
            board,
            wrap,
            game,
            options,
            cards: Vec::new(),
            slots: Vec::new(),
            runs: None,
            badge: None,
            geometry: Geometry::empty(0),
            focus: Focus {
                pile: Pile::Stock,
                index: None,
            },
            drag: None,
            last_tap: None,
            hint: None,
            hint_token: 0,
            pending: Cell::new(0.0),
            next_save: Cell::new(10),
            instant: Cell::new(true),
        };
        app.next_save.set(app.game.elapsed().saturating_add(10));
        app.build_board()?;
        app.focus = app.default_focus();
        Ok(app)
    }

    /// Create the places, the runs strip, the badge and the deck, once.
    ///
    /// The markup is generated from the same card model the rules use, and
    /// nothing a reader typed reaches it: ranks, suits and fixed class names
    /// only, which is why it needs no escaping.
    fn build_board(&mut self) -> Result<(), JsValue> {
        for pile in Pile::ALL {
            let slot: HtmlButtonElement = self.document.create_element("button")?.dyn_into()?;
            slot.set_attribute("type", "button")?;
            slot.set_class_name("slot");
            slot.set_attribute("data-pile", &pile.slug())?;
            slot.set_attribute("aria-label", &self.game.pile_label(pile))?;
            self.board.append_child(&slot)?;
            self.slots.push((pile, slot));
        }

        // The eight runs, and the count that goes with them. Built here and
        // only described afterwards; the pips are decoration, so the whole strip
        // is one labelled image to a screen reader rather than eight unlabelled
        // boxes.
        let mut strip = String::from("<span class=\"runs-label\" id=\"runs-label\"></span>");
        for index in 0..RUNS {
            strip.push_str(&format!(
                "<span class=\"run-pip\" id=\"run-{index}\"></span>"
            ));
        }
        let runs = self.document.create_element("div")?;
        runs.set_class_name("runs");
        runs.set_attribute("role", "img")?;
        runs.set_attribute("aria-labelledby", "runs-label")?;
        runs.set_inner_html(&strip);
        self.board.append_child(&runs)?;
        self.runs = Some(runs);

        let badge = self.document.create_element("span")?;
        badge.set_class_name("badge");
        badge.set_attribute("aria-hidden", "true")?;
        self.board.append_child(&badge)?;
        self.badge = Some(badge);

        for id in 0..self.game.deck_len() {
            let card = self.game.card(u16::try_from(id).unwrap_or(0));
            let element: HtmlButtonElement = self.document.create_element("button")?.dyn_into()?;
            element.set_attribute("type", "button")?;
            element.set_class_name(&format!("card {}", card.suit.class()));
            element.set_attribute("data-card", &id.to_string())?;
            element.set_attribute("data-rank", card.rank.short())?;
            element.set_inner_html(&card_html(card));
            self.board.append_child(&element)?;
            self.cards.push(element);
        }
        Ok(())
    }

    // -- painting --------------------------------------------------------

    /// Draw the whole app from the state.
    fn paint(&mut self) {
        self.normalise_focus();
        self.geometry = self.compute_geometry();
        let _ = self.board.set_attribute(
            "style",
            &format!("--cw:{}px;--ch:{}px", self.geometry.cw, self.geometry.ch),
        );
        self.set_density();
        self.paint_cards();
        self.paint_slots();
        self.paint_runs();
        self.paint_facts();
        self.paint_tools();
        self.paint_menu();
        self.paint_won();
    }

    /// Switch the card face between its full and compact forms.
    ///
    /// One class on the board, written only when it changes — a render that
    /// rewrote it every time would invalidate style for all 104 cards on every
    /// tap, and nothing about their layout changes.
    fn set_density(&self) {
        let mut want = String::new();
        if self.instant.get() {
            want.push_str("instant");
        }
        if self.geometry.cw > 0.0 && self.geometry.cw < COMPACT_BELOW {
            if !want.is_empty() {
                want.push(' ');
            }
            want.push_str("compact");
        }
        if self.board.get_attribute("class").as_deref() != Some(want.as_str()) {
            self.board.set_class_name(&want);
        }
    }

    /// Let the cards move from now on. Called one frame after the first paint.
    fn settle(&self) {
        self.instant.set(false);
        self.set_density();
    }

    /// Compute where every card and place sits for the current box.
    ///
    /// Width decides the card — ten columns share it — and the height may shrink
    /// it further. The size is solved from the fan of the tallest column in the
    /// deal, with a floor: a long game shrinks the whole board rather than hiding
    /// every rank behind the next card, and only when the floor is reached does
    /// the fan itself compress.
    fn compute_geometry(&self) -> Geometry {
        let deck = self.cards.len();
        let width = f64::from(self.wrap.client_width());
        let height = f64::from(self.wrap.client_height());
        if width <= 1.0 || height <= 1.0 {
            return Geometry::empty(deck);
        }

        let margin = (width * 0.012).clamp(4.0, 10.0);
        let usable = (width - margin * 2.0).max(1.0);
        let gap = (usable * 0.006).clamp(2.0, 8.0);
        let columns = COLUMNS as f64;
        let width_card = (usable - gap * (columns - 1.0)) / columns;

        let (down_gaps, up_gaps) = self.longest_fan();
        let fan = down_gaps as f64 * DOWN_FAN + up_gaps as f64 * UP_FAN;
        let vertical = (height - margin * 2.0).max(1.0);
        let solve = vertical / (ASPECT * (2.0 + ROW_GAP + fan));
        let cw = width_card.min(solve).max(width_card * MIN_SCALE).max(6.0);
        let ch = cw * ASPECT;

        let total = cw * columns + gap * (columns - 1.0);
        let x0 = (width - total) / 2.0;
        let top_y = margin;
        let tableau_y = top_y + ch + (ch * ROW_GAP).max(4.0);
        let tableau_room = (height - tableau_y - ch - margin).max(0.0);
        let column_x = |column: usize| x0 + column as f64 * (cw + gap);

        let mut positions = vec![(0.0, 0.0); deck];
        let mut slots = vec![(0.0, 0.0); Pile::ALL.len()];

        for (slot_index, pile) in Pile::ALL.into_iter().enumerate() {
            let (x, y) = match pile {
                Pile::Stock => (column_x(0), top_y),
                Pile::Tableau(index) => (column_x(index), tableau_y),
            };
            slots[slot_index] = (x, y);

            let cards = self.game.pile(pile);
            match pile {
                Pile::Stock => {
                    // Every card in the stock shares one place: it is a packet,
                    // not a fan, and its size is said by the count on it.
                    for &id in cards {
                        positions[id as usize] = (x, y);
                    }
                }
                Pile::Tableau(_) => {
                    let up = first_face_up(&self.game, pile);
                    let weights: Vec<f64> = (1..cards.len())
                        .map(|position| {
                            if position - 1 < up {
                                ch * DOWN_FAN
                            } else {
                                ch * UP_FAN
                            }
                        })
                        .collect();
                    let natural: f64 = weights.iter().sum();
                    let first = if natural > tableau_room && natural > 0.0 {
                        tableau_room / natural
                    } else {
                        1.0
                    };
                    // Even a squeezed column keeps a readable edge on each card
                    // before the fan is compressed any further.
                    let mut gaps: Vec<f64> = weights
                        .iter()
                        .map(|weight| {
                            let floor = if (*weight - ch * DOWN_FAN).abs() < f64::EPSILON {
                                ch * 0.055
                            } else {
                                ch * 0.09
                            };
                            (weight * first).max(floor)
                        })
                        .collect();
                    let total: f64 = gaps.iter().sum();
                    if total > tableau_room && total > 0.0 {
                        let second = tableau_room / total;
                        for gap in &mut gaps {
                            *gap *= second;
                        }
                    }
                    let mut offset = 0.0;
                    for (position, &id) in cards.iter().enumerate() {
                        if position > 0 {
                            offset += gaps.get(position - 1).copied().unwrap_or(0.0);
                        }
                        positions[id as usize] = (x, y + offset);
                    }
                }
            }
        }

        // The strip of completed runs takes the nine columns the stock does not.
        let runs = (column_x(1), top_y, total - (cw + gap));

        Geometry {
            cw,
            ch,
            positions,
            slots,
            runs,
        }
    }

    /// The fan of the deal's tallest column: face-down gaps, face-up gaps.
    fn longest_fan(&self) -> (usize, usize) {
        let mut best: (usize, usize, usize) = (0, 0, 0);
        for column in 0..COLUMNS {
            let pile = Pile::Tableau(column);
            let down = first_face_up(&self.game, pile);
            let up = self.game.pile_len(pile).saturating_sub(down);
            if down + up > best.0 {
                best = (down + up, down, up);
            }
        }
        let (_, down, up) = best;
        (down.saturating_sub(1), up.saturating_sub(1))
    }

    fn paint_cards(&mut self) {
        let mut locations: Vec<Option<(Pile, usize)>> = vec![None; self.cards.len()];
        for pile in Pile::ALL {
            for (index, &id) in self.game.pile(pile).iter().enumerate() {
                if let Some(slot) = locations.get_mut(id as usize) {
                    *slot = Some((pile, index));
                }
            }
        }

        let selection = self.game.selection();
        let focus_card = self.focus_card_id();
        let dragging: Vec<u16> = self
            .drag
            .as_ref()
            .filter(|drag| drag.active)
            .map(|drag| drag.cards.clone())
            .unwrap_or_default();

        for (id, location) in locations.iter().enumerate() {
            let Some((pile, index)) = *location else {
                // A card that left the board — a completed run, lifted to the
                // strip at the top — must stop being drawn. Its element stays,
                // because the deck is built once, but it is hidden rather than
                // left at the position of its last move.
                if let Some(element) = self.cards.get(id) {
                    element.set_class_name("card gone");
                    let _ = element.set_attribute("hidden", "");
                    let _ = element.set_attribute("aria-hidden", "true");
                    let _ = element.remove_attribute("aria-label");
                    // The pile and position go too. A hidden element cannot be
                    // clicked, but leaving the attributes behind leaves a ghost
                    // card that anything walking the board by name would still
                    // find — and "which column is this card in" must have one
                    // answer, which for a lifted card is "none".
                    let _ = element.remove_attribute("data-pile");
                    let _ = element.remove_attribute("data-index");
                }
                continue;
            };
            let Some(element) = self.cards.get(id) else {
                continue;
            };
            let _ = element.remove_attribute("hidden");
            let face_down = self.game.is_face_down(pile, index);
            let (x, y) = self
                .geometry
                .positions
                .get(id)
                .copied()
                .unwrap_or((0.0, 0.0));
            let card = self.game.card(u16::try_from(id).unwrap_or(0));
            let selected = selection.is_some_and(|held| held.pile == pile && index >= held.index);
            let hinted = match &self.hint {
                Some(Hint::Move {
                    from,
                    index: start,
                    count,
                    ..
                }) => *from == pile && index >= *start && index < *start + *count,
                _ => false,
            };
            let focused = focus_card == Some(u16::try_from(id).unwrap_or(0));
            let is_dragging = dragging.contains(&u16::try_from(id).unwrap_or(0));

            let mut classes = format!("card {}", card.suit.class());
            if face_down {
                classes.push_str(" down");
            }
            if selected {
                classes.push_str(" selected");
            }
            if hinted {
                classes.push_str(" hinted");
            }
            if focused {
                classes.push_str(" focused");
            }
            if is_dragging {
                classes.push_str(" dragging");
            }
            element.set_class_name(&classes);
            let _ = element.set_attribute(
                "style",
                &format!("left:{x}px;top:{y}px;z-index:{}", z_index(pile, index)),
            );
            let _ = element.set_attribute("data-pile", &pile.slug());
            let _ = element.set_attribute("data-index", &index.to_string());
            let _ = element.set_attribute("tabindex", if focused { "0" } else { "-1" });

            if face_down {
                if pile == Pile::Stock && focused {
                    let _ = element.remove_attribute("aria-hidden");
                    let _ = element.set_attribute(
                        "aria-label",
                        &format!("{}, deal.", self.game.pile_label(pile)),
                    );
                } else {
                    let _ = element.set_attribute("aria-hidden", "true");
                    let _ = element.remove_attribute("aria-label");
                }
            } else {
                let _ = element.remove_attribute("aria-hidden");
                let mut label = format!(
                    "{} in {}, card {} of {}.",
                    card.name(),
                    pile.name(),
                    index + 1,
                    self.game.pile_len(pile)
                );
                if selected {
                    label.push_str(" Selected.");
                }
                let _ = element.set_attribute("aria-label", &label);
            }
        }
    }

    fn paint_slots(&mut self) {
        for (slot_index, (pile, element)) in self.slots.iter().enumerate() {
            let (x, y) = self
                .geometry
                .slots
                .get(slot_index)
                .copied()
                .unwrap_or((0.0, 0.0));
            let empty = self.game.pile_len(*pile) == 0;
            let hinted = match &self.hint {
                Some(Hint::Move { to, .. }) => to == pile,
                Some(Hint::Deal { .. }) => *pile == Pile::Stock,
                _ => false,
            };
            let focused = self.focus.pile == *pile && self.focus.index.is_none();
            let mut classes = String::from("slot");
            if !empty {
                classes.push_str(" filled");
            }
            if hinted {
                classes.push_str(" hinted");
            }
            element.set_class_name(&classes);
            let _ = element.set_attribute("style", &format!("left:{x}px;top:{y}px;z-index:1"));
            let _ = element.set_attribute("data-index", &self.game.pile_len(*pile).to_string());
            let _ = element.set_attribute("tabindex", if focused { "0" } else { "-1" });
            let _ = element.set_attribute("aria-label", &self.game.pile_label(*pile));
            element.set_text_content(Some(self.game.slot_mark(*pile)));
        }

        if let Some(badge) = &self.badge {
            let deals = self.game.deals_left();
            let (x, y) = self.geometry.slots.first().copied().unwrap_or((0.0, 0.0));
            let offset = self.geometry.cw * 0.62;
            let _ = badge.set_attribute(
                "style",
                &format!(
                    "left:{}px;top:{}px;z-index:900",
                    x + offset,
                    y + offset * 0.35
                ),
            );
            let _ = badge.set_attribute("class", if deals == 0 { "badge gone" } else { "badge" });
            badge.set_text_content(Some(&format!("{deals}")));
        }
    }

    /// Paint the strip of completed runs.
    ///
    /// One label and eight pips: a filled pip is a run already lifted, and it
    /// keeps the suit it was, so a player can see at a glance which ones are
    /// gone. The label is what a screen reader reads, and it says the count in
    /// words rather than leaving it to eight unlabelled boxes.
    fn paint_runs(&mut self) {
        let Some(runs) = &self.runs else {
            return;
        };
        let (left, top, width) = self.geometry.runs;
        let _ = runs.set_attribute(
            "style",
            &format!(
                "left:{left}px;top:{top}px;width:{width}px;height:{}px",
                self.geometry.ch
            ),
        );
        if let Some(label) = self.document.get_element_by_id("runs-label") {
            let done = self.game.runs();
            label.set_text_content(Some(&format!("{done} of {RUNS}")));
        }
        let completed = self.game.completed_suits().to_vec();
        for index in 0..RUNS {
            // Each pip has its own id, so the paint reaches it without walking
            // the tree: eight lookups by name is cheaper to read than a node
            // list walk, and it is the same hook the stylesheet uses.
            let Some(pip) = self.document.get_element_by_id(&format!("run-{index}")) else {
                continue;
            };
            let suit = completed.get(index).copied();
            let mut classes = String::from("run-pip");
            if let Some(suit) = suit {
                classes.push_str(" done ");
                classes.push_str(suit.class());
            }
            let _ = pip.set_attribute("class", &classes);
            let text = suit
                .map(|suit| suit.symbol().to_string())
                .unwrap_or_default();
            pip.set_text_content(Some(&text));
        }
        let _ = runs.set_attribute(
            "aria-label",
            &format!("{} of {RUNS} runs completed", self.game.runs()),
        );
    }

    fn paint_facts(&self) {
        let readout = self.game.readout();
        set_text(&self.document, "clock", &readout.clock);
        set_text(&self.document, "moves", &readout.moves);
        if let Some(score) = readout.score {
            set_text(&self.document, "score", &score);
            set_hidden(&self.document, "score-fact", false);
        } else {
            set_hidden(&self.document, "score-fact", true);
        }
    }

    fn paint_tools(&self) {
        let won = self.game.won();
        if let Some(undo) = button(&self.document, "undo") {
            undo.set_disabled(won || !self.game.can_undo());
        }
        if let Some(hint) = button(&self.document, "hint") {
            hint.set_disabled(won);
        }
        if let Some(deal) = button(&self.document, "deal") {
            deal.set_disabled(won || !self.game.can_deal());
        }
    }

    fn paint_menu(&self) {
        set_text(&self.document, "variant", self.game.variant());
        set_text(&self.document, "game-summary", &self.game.summary());
        set_text(
            &self.document,
            "difficulty-note",
            self.options.difficulty.sentence(),
        );
        set_text(
            &self.document,
            "scoring-note",
            self.options.scoring.sentence(),
        );
        set_select(&self.document, "difficulty", self.options.difficulty.slug());
        set_select(&self.document, "scoring", self.options.scoring.slug());
    }

    fn paint_won(&self) {
        let won = self.game.won();
        set_hidden(&self.document, "won", !won);
        if won {
            set_text(&self.document, "won-detail", &self.game.result_words());
        }
    }

    // -- actions ---------------------------------------------------------

    /// A tap (or a keyboard activation) on a card or a place.
    fn tap_at(&mut self, pile: Pile, index: usize) {
        self.clear_hint();
        if pile == Pile::Stock {
            self.last_tap = None;
            let report = self.game.deal();
            self.focus = self.default_focus();
            self.after(report);
            self.focus_element();
            return;
        }

        let now = js_sys::Date::now();
        let double = self.last_tap.is_some_and(|tap| {
            tap.pile == pile && tap.index == index && now - tap.at < DOUBLE_TAP_MS
        });
        self.last_tap = if double {
            None
        } else {
            Some(Tap {
                pile,
                index,
                at: now,
            })
        };
        let report = if double {
            self.game.double_tap(pile, index)
        } else {
            self.game.tap(pile, index)
        };
        self.focus = match self.game.selection() {
            Some(held) => Focus {
                pile: held.pile,
                index: Some(held.index),
            },
            None => self.top_focus(pile),
        };
        self.after(report);
        self.focus_element();
    }

    fn deal_action(&mut self) {
        self.clear_hint();
        let report = self.game.deal();
        self.focus = self.default_focus();
        self.after(report);
        self.focus_element();
    }

    fn undo_action(&mut self) {
        self.clear_hint();
        let report = self.game.undo();
        self.focus = self.top_focus(self.focus.pile);
        self.after(report);
        self.focus_element();
    }

    fn new_game(&mut self, restart: bool) {
        self.hint = None;
        self.last_tap = None;
        self.drag = None;
        self.game = if restart {
            self.game.restart()
        } else {
            Game::dealt(self.options.difficulty, self.options.scoring, fresh_seed())
        };
        self.focus = self.default_focus();
        self.next_save.set(self.game.elapsed().saturating_add(10));
        close_menu(&self.document);
        self.paint();
        let words = if restart {
            "Restarted the same deal.".to_owned()
        } else {
            format!("New deal. {}", self.options.summary())
        };
        self.say(&words);
        self.save_game();
        self.save_options();
        self.focus_element();
    }

    fn after(&mut self, report: Report) {
        if report.won {
            // The dialog carries the result; the line under the board says only
            // that the game is over, rather than printing the same sentence
            // twice on one screen.
            self.say("Game complete.");
        } else if !report.words.is_empty() {
            self.say(&report.words);
        }
        self.paint();
        if report.won {
            self.show_win();
        }
        self.save_game();
    }

    fn show_win(&mut self) {
        set_hidden(&self.document, "won", false);
        set_text(&self.document, "won-detail", &self.game.result_words());
        if let Some(new) = button(&self.document, "won-new") {
            let _ = new.focus();
        }
    }

    fn tick(&mut self, seconds: u32) {
        if self.game.won() {
            return;
        }
        self.game.tick(seconds);
        self.paint_facts();
        if self.game.started() && self.game.elapsed() >= self.next_save.get() {
            self.next_save.set(self.game.elapsed().saturating_add(10));
            self.save_game();
        }
    }

    // -- focus -----------------------------------------------------------

    fn default_focus(&self) -> Focus {
        if self.game.pile_len(Pile::Stock) > 0 {
            return Focus {
                pile: Pile::Stock,
                index: Some(self.game.pile_len(Pile::Stock) - 1),
            };
        }
        for column in 0..COLUMNS {
            let pile = Pile::Tableau(column);
            if first_face_up(&self.game, pile) < self.game.pile_len(pile) {
                return Focus {
                    pile,
                    index: Some(self.game.pile_len(pile) - 1),
                };
            }
        }
        Focus {
            pile: Pile::Tableau(0),
            index: None,
        }
    }

    fn top_focus(&self, pile: Pile) -> Focus {
        let len = self.game.pile_len(pile);
        if len == 0 {
            return Focus { pile, index: None };
        }
        match pile {
            Pile::Tableau(_) if first_face_up(&self.game, pile) >= len => {
                Focus { pile, index: None }
            }
            _ => Focus {
                pile,
                index: Some(len - 1),
            },
        }
    }

    fn focus_card_id(&self) -> Option<u16> {
        let index = self.focus.index?;
        self.game.pile(self.focus.pile).get(index).copied()
    }

    fn focus_element(&self) {
        if let Some(id) = self.focus_card_id() {
            if let Some(card) = self.cards.get(id as usize) {
                let _ = card.focus();
            }
            return;
        }
        if let Some((_, slot)) = self.slots.iter().find(|(pile, _)| *pile == self.focus.pile) {
            let _ = slot.focus();
        }
    }

    fn normalise_focus(&mut self) {
        let valid = match self.focus.index {
            Some(index) => {
                index < self.game.pile_len(self.focus.pile)
                    && !self.game.is_face_down(self.focus.pile, index)
            }
            None => match self.focus.pile {
                Pile::Stock => self.game.pile_len(Pile::Stock) == 0,
                Pile::Tableau(_) => {
                    first_face_up(&self.game, self.focus.pile)
                        >= self.game.pile_len(self.focus.pile)
                }
            },
        };
        if !valid {
            self.focus = self.default_focus();
        }
    }

    fn move_focus(&mut self, delta: i32) {
        self.normalise_focus();
        let current = Pile::ALL
            .iter()
            .position(|pile| *pile == self.focus.pile)
            .unwrap_or(0);
        let count = i32::try_from(Pile::ALL.len()).unwrap_or(1);
        let next = (i32::try_from(current).unwrap_or(0) + delta).rem_euclid(count);
        let pile = Pile::ALL[usize::try_from(next).unwrap_or(0)];
        self.focus = self.top_focus(pile);
        self.paint();
        self.focus_element();
    }

    fn move_within(&mut self, delta: i32) {
        let Pile::Tableau(_) = self.focus.pile else {
            return;
        };
        let pile = self.focus.pile;
        let len = self.game.pile_len(pile);
        let up = first_face_up(&self.game, pile);
        if up >= len {
            return;
        }
        let current = self.focus.index.unwrap_or(len - 1).clamp(up, len - 1);
        let low = i32::try_from(up).unwrap_or(0);
        let high = i32::try_from(len - 1).unwrap_or(low);
        let next = (i32::try_from(current).unwrap_or(0) + delta).clamp(low, high);
        self.focus = Focus {
            pile,
            index: Some(usize::try_from(next).unwrap_or(0)),
        };
        self.paint();
        self.focus_element();
    }

    // -- pointer ---------------------------------------------------------

    fn pointer_down(&mut self, id: i32, pile: Pile, index: usize, x: f64, y: f64) {
        if self.game.won() {
            return;
        }
        self.clear_hint();
        self.focus = Focus {
            pile,
            index: (index < self.game.pile_len(pile)).then_some(index),
        };
        self.last_tap = None;
        self.drag = Some(Drag {
            id,
            pile,
            index,
            start_x: x,
            start_y: y,
            x,
            y,
            active: false,
            cards: Vec::new(),
        });
        self.paint();
    }

    fn pointer_moved(&mut self, id: i32, x: f64, y: f64) {
        let Some(mut drag) = self.drag.clone() else {
            return;
        };
        if drag.id != id || drag.pile == Pile::Stock {
            return;
        }
        drag.x = x;
        drag.y = y;
        if !drag.active {
            if (x - drag.start_x).hypot(y - drag.start_y) < DRAG_SLOP {
                return;
            }
            if self
                .game
                .selection()
                .is_some_and(|held| held.pile != drag.pile)
            {
                // A destination is already in hand; a press on another column
                // is a tap, not a second drag.
                return;
            }
            let Some(count) = self.game.run_len(drag.pile, drag.index) else {
                return;
            };
            let report = self.game.select(drag.pile, drag.index);
            if !report.words.is_empty() {
                self.say(&report.words);
            }
            drag.cards = self.game.pile(drag.pile)[drag.index..drag.index + count].to_vec();
            drag.active = true;
            self.drag = Some(drag.clone());
            self.paint();
        }
        self.drag = Some(drag.clone());
        self.drag_visual(&drag);
    }

    fn pointer_up(&mut self, id: i32, x: f64, y: f64) {
        if self.drag.as_ref().is_none_or(|drag| drag.id != id) {
            return;
        }
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.active {
            let to = self.drop_target(x, y);
            let report = match to {
                Some(to) => self.game.drop(to),
                None => self.game.clear_selection(),
            };
            self.focus = to.map_or(self.focus, |pile| self.top_focus(pile));
            self.after(report);
            return;
        }
        self.tap_at(drag.pile, drag.index);
    }

    fn pointer_cancelled(&mut self, id: i32) {
        if self.drag.as_ref().is_none_or(|drag| drag.id != id) {
            return;
        }
        self.drag = None;
        self.paint();
    }

    fn drag_visual(&self, drag: &Drag) {
        let dx = drag.x - drag.start_x;
        let dy = drag.y - drag.start_y;
        for (offset, &id) in drag.cards.iter().enumerate() {
            let Some(element) = self.cards.get(id as usize) else {
                continue;
            };
            let (x, y) = self
                .geometry
                .positions
                .get(id as usize)
                .copied()
                .unwrap_or((0.0, 0.0));
            let z = 2_000 + i32::try_from(offset).unwrap_or(0);
            let _ = element.set_attribute(
                "style",
                &format!("left:{}px;top:{}px;z-index:{z}", x + dx, y + dy),
            );
        }
    }

    fn drop_target(&self, x: f64, y: f64) -> Option<Pile> {
        let array = self
            .document
            .elements_from_point(f64_to_f32(x), f64_to_f32(y));
        for value in array.iter() {
            let Ok(element) = value.dyn_into::<Element>() else {
                continue;
            };
            if let Some(pile) = pile_of_element(&element) {
                if pile != Pile::Stock {
                    return Some(pile);
                }
            }
        }
        None
    }

    // -- hints and storage ----------------------------------------------

    fn clear_hint(&mut self) {
        self.hint = None;
    }

    fn say(&self, words: &str) {
        set_text(&self.document, "status", words);
    }

    fn save_game(&self) {
        let Some(storage) = storage(&self.window) else {
            return;
        };
        match self.game.to_stored() {
            Some(value) => {
                if let Err(error) = storage.set_item(STORAGE_KEY, &value) {
                    warn("could not save the game", &error);
                }
            }
            None => {
                let _ = storage.remove_item(STORAGE_KEY);
            }
        }
    }

    fn save_options(&self) {
        let Some(storage) = storage(&self.window) else {
            return;
        };
        if let Err(error) = storage.set_item(OPTIONS_KEY, &self.options.to_stored()) {
            warn("could not save the options", &error);
        }
    }

    fn menu_open(&self) -> bool {
        checkbox(&self.document, "menu").is_some_and(|toggle| toggle.checked())
    }
}

// -- the board's vocabulary ----------------------------------------------

fn card_html(card: crate::card::Card) -> String {
    let rank = card.rank.short();
    let suit = card.suit.symbol();
    format!(
        "<span class=\"face\">\
           <span class=\"corner tl\"><span class=\"rank\">{rank}</span><span class=\"suit\">{suit}</span></span>\
           <span class=\"pip\">{suit}</span>\
           <span class=\"corner br\"><span class=\"rank\">{rank}</span><span class=\"suit\">{suit}</span></span>\
         </span>\
         <span class=\"back\"></span>"
    )
}

fn z_index(pile: Pile, index: usize) -> i32 {
    let base = i32::try_from(
        Pile::ALL
            .iter()
            .position(|known| *known == pile)
            .unwrap_or(0),
    )
    .unwrap_or(0);
    20 + base * 40 + i32::try_from(index).unwrap_or(0)
}

/// The first face-up card in a column, or its length when every card is down.
fn first_face_up(game: &Game, pile: Pile) -> usize {
    let len = game.pile_len(pile);
    (0..len)
        .find(|&index| !game.is_face_down(pile, index))
        .unwrap_or(len)
}

/// The pile an element belongs to, walking up to the card or place that names
/// it. A dragging card is not a drop target, so its whole subtree is skipped.
fn pile_of_element(element: &Element) -> Option<Pile> {
    if element
        .get_attribute("class")
        .is_some_and(|classes| classes.split_whitespace().any(|class| class == "dragging"))
    {
        return None;
    }
    element
        .get_attribute("data-pile")
        .and_then(|slug| Pile::from_slug(&slug))
        .or_else(|| {
            element
                .parent_element()
                .and_then(|parent| pile_of_element(&parent))
        })
}

/// The pile and position a click or press landed on.
fn target_of(event: &Event) -> Option<(Pile, usize)> {
    let mut element = event.target()?.dyn_into::<Element>().ok()?;
    loop {
        if let Some(slug) = element.get_attribute("data-pile") {
            let pile = Pile::from_slug(&slug)?;
            let index = element
                .get_attribute("data-index")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            return Some((pile, index));
        }
        element = element.parent_element()?;
    }
}

fn f64_to_f32(value: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    let rounded = value.round();
    rounded.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32
}

// -- binding -------------------------------------------------------------

fn bind(window: &Window, document: &Document, shared: &Shared) {
    bind_board(document, shared);
    bind_tools(window, document, shared);
    bind_menu(window, document, shared);
    bind_keys(window, document, shared);
}

fn bind_board(document: &Document, shared: &Shared) {
    let Some(board) = document.get_element_by_id("board") else {
        warn("the shell has no #board", &JsValue::from_str("missing"));
        return;
    };

    let down = {
        let app = Rc::clone(shared);
        Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
            if event.button() != 0 {
                return;
            }
            let Some(target) = event
                .target()
                .and_then(|target| target.dyn_into::<Element>().ok())
            else {
                return;
            };
            let Some((pile, index)) = target_of(&event) else {
                return;
            };
            event.prevent_default();
            // Capture, so a finger that leaves the board mid-drag keeps sending
            // its moves here. A browser that refuses — a synthetic event with no
            // real pointer behind it — is not a problem: the listener is on the
            // board and the card never leaves it.
            let _ = target.set_pointer_capture(event.pointer_id());
            let mut app = app.borrow_mut();
            app.pointer_down(
                event.pointer_id(),
                pile,
                index,
                f64::from(event.client_x()),
                f64::from(event.client_y()),
            );
        })
    };
    listen(&board, "pointerdown", down.as_ref().unchecked_ref());
    down.forget();

    let moved = {
        let app = Rc::clone(shared);
        Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
            app.borrow_mut().pointer_moved(
                event.pointer_id(),
                f64::from(event.client_x()),
                f64::from(event.client_y()),
            );
        })
    };
    listen(&board, "pointermove", moved.as_ref().unchecked_ref());
    moved.forget();

    let up = {
        let app = Rc::clone(shared);
        Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
            app.borrow_mut().pointer_up(
                event.pointer_id(),
                f64::from(event.client_x()),
                f64::from(event.client_y()),
            );
        })
    };
    listen(&board, "pointerup", up.as_ref().unchecked_ref());
    up.forget();

    let cancel = {
        let app = Rc::clone(shared);
        Closure::<dyn FnMut(PointerEvent)>::new(move |event: PointerEvent| {
            app.borrow_mut().pointer_cancelled(event.pointer_id());
        })
    };
    listen(&board, "pointercancel", cancel.as_ref().unchecked_ref());
    cancel.forget();

    // A keyboard activation arrives as a click with no pointer detail. Mouse and
    // touch clicks are ignored here: their gesture was already handled by the
    // pointer listeners, and handling both would move every card twice.
    let click = {
        let app = Rc::clone(shared);
        Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            if event.detail() != 0 {
                return;
            }
            let Some((pile, index)) = target_of(&event) else {
                return;
            };
            event.prevent_default();
            app.borrow_mut().tap_at(pile, index);
        })
    };
    listen(&board, "click", click.as_ref().unchecked_ref());
    click.forget();
}

fn bind_tools(window: &Window, document: &Document, shared: &Shared) {
    let window = window.clone();
    if let Some(undo) = button(document, "undo") {
        let app = Rc::clone(shared);
        let closure = Closure::<dyn FnMut()>::new(move || {
            app.borrow_mut().undo_action();
        });
        listen(&undo, "click", closure.as_ref().unchecked_ref());
        closure.forget();
    }
    if let Some(hint) = button(document, "hint") {
        let app = Rc::clone(shared);
        let closure = Closure::<dyn FnMut()>::new(move || {
            show_hint(&app, &window);
        });
        listen(&hint, "click", closure.as_ref().unchecked_ref());
        closure.forget();
    }
    if let Some(deal) = button(document, "deal") {
        let app = Rc::clone(shared);
        let closure = Closure::<dyn FnMut()>::new(move || {
            app.borrow_mut().deal_action();
        });
        listen(&deal, "click", closure.as_ref().unchecked_ref());
        closure.forget();
    }
    if let Some(new) = button(document, "won-new") {
        let app = Rc::clone(shared);
        let closure = Closure::<dyn FnMut()>::new(move || {
            app.borrow_mut().new_game(false);
        });
        listen(&new, "click", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

fn bind_menu(_window: &Window, document: &Document, shared: &Shared) {
    if let Some(toggle) = checkbox(document, "menu") {
        let app = Rc::clone(shared);
        let boxed = toggle.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let document = app.borrow().document.clone();
            set_hidden(&document, "menu-panel", !boxed.checked());
        });
        listen(&toggle, "change", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    if let Some(select) = select(document, "difficulty") {
        let app = Rc::clone(shared);
        let handle = select.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let value = handle.value();
            let mut app = app.borrow_mut();
            app.options.difficulty = Difficulty::from_slug(&value).unwrap_or(Difficulty::Medium);
            app.save_options();
            app.paint_menu();
            app.say("Difficulty set. It applies to the next deal.");
        });
        listen(&select, "change", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    if let Some(select) = select(document, "scoring") {
        let app = Rc::clone(shared);
        let handle = select.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let value = handle.value();
            let mut app = app.borrow_mut();
            app.options.scoring = Scoring::from_slug(&value).unwrap_or(Scoring::Standard);
            app.save_options();
            app.paint_menu();
            app.say("Scoring set. It applies to the next deal.");
        });
        listen(&select, "change", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    if let Some(new) = button(document, "new") {
        let app = Rc::clone(shared);
        let closure = Closure::<dyn FnMut()>::new(move || {
            app.borrow_mut().new_game(false);
        });
        listen(&new, "click", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    if let Some(restart) = button(document, "restart") {
        let app = Rc::clone(shared);
        let closure = Closure::<dyn FnMut()>::new(move || {
            app.borrow_mut().new_game(true);
        });
        listen(&restart, "click", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

fn bind_keys(window: &Window, document: &Document, shared: &Shared) {
    let window = window.clone();
    let shared = Rc::clone(shared);
    let closure = {
        let app = Rc::clone(&shared);
        Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
            if is_form_control(event.target()) {
                return;
            }
            let mut app = app.borrow_mut();
            if app.menu_open() {
                if event.key() == "Escape" {
                    close_menu(&app.document);
                    event.prevent_default();
                }
                return;
            }
            let key = event.key();
            let handled = match key.as_str() {
                "ArrowLeft" => {
                    app.move_focus(-1);
                    true
                }
                "ArrowRight" => {
                    app.move_focus(1);
                    true
                }
                "ArrowUp" => {
                    app.move_within(-1);
                    true
                }
                "ArrowDown" => {
                    app.move_within(1);
                    true
                }
                " " | "Spacebar" => {
                    app.deal_action();
                    true
                }
                "Escape" => {
                    let report = app.game.clear_selection();
                    app.paint();
                    if !report.words.is_empty() {
                        app.say(&report.words);
                    }
                    true
                }
                "u" | "U" => {
                    app.undo_action();
                    true
                }
                "d" | "D" => {
                    app.deal_action();
                    true
                }
                "h" | "H" => {
                    drop(app);
                    show_hint(&shared, &window);
                    true
                }
                "z" | "Z" if event.ctrl_key() || event.meta_key() => {
                    app.undo_action();
                    true
                }
                _ => false,
            };
            if handled {
                event.prevent_default();
            }
        })
    };
    listen(document, "keydown", closure.as_ref().unchecked_ref());
    closure.forget();
}

fn is_form_control(target: Option<web_sys::EventTarget>) -> bool {
    let Some(target) = target.and_then(|target| target.dyn_into::<Element>().ok()) else {
        return false;
    };
    matches!(
        target.tag_name().as_str(),
        "INPUT" | "SELECT" | "TEXTAREA" | "OPTION"
    )
}

fn show_hint(shared: &Shared, window: &Window) {
    let mut app = shared.borrow_mut();
    if app.game.won() {
        return;
    }
    let hint = app.game.hint();
    if let Hint::None { .. } = &hint {
        app.say(hint.words());
        return;
    }
    app.say(hint.words());
    app.hint = Some(hint);
    app.hint_token = app.hint_token.wrapping_add(1);
    let token = app.hint_token;
    app.paint();
    drop(app);

    let reused = Rc::clone(shared);
    let closure = Closure::<dyn FnMut()>::new(move || {
        let mut app = reused.borrow_mut();
        if app.hint_token == token {
            app.hint = None;
            app.paint();
        }
    });
    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
        closure.as_ref().unchecked_ref(),
        HINT_MS,
    );
    closure.forget();
}

fn close_menu(document: &Document) {
    if let Some(toggle) = checkbox(document, "menu") {
        toggle.set_checked(false);
    }
    set_hidden(document, "menu-panel", true);
}

// -- small DOM helpers ---------------------------------------------------

fn element(document: &Document, id: &str) -> Option<Element> {
    document.get_element_by_id(id)
}

fn button(document: &Document, id: &str) -> Option<HtmlButtonElement> {
    element(document, id)?.dyn_into().ok()
}

fn checkbox(document: &Document, id: &str) -> Option<HtmlInputElement> {
    element(document, id)?.dyn_into().ok()
}

fn select(document: &Document, id: &str) -> Option<HtmlSelectElement> {
    element(document, id)?.dyn_into().ok()
}

fn set_text(document: &Document, id: &str, text: &str) {
    if let Some(element) = element(document, id) {
        element.set_text_content(Some(text));
    }
}

fn set_hidden(document: &Document, id: &str, hidden: bool) {
    if let Some(element) = element(document, id) {
        if hidden {
            let _ = element.set_attribute("hidden", "");
        } else {
            let _ = element.remove_attribute("hidden");
        }
    }
}

fn set_select(document: &Document, id: &str, value: &str) {
    if let Some(select) = select(document, id) {
        if select.value() != value {
            select.set_value(value);
        }
    }
}

fn listen(target: &web_sys::EventTarget, event: &str, callback: &js_sys::Function) {
    if let Err(error) = target.add_event_listener_with_callback(event, callback) {
        warn("could not bind an event", &error);
    }
}

fn storage(window: &Window) -> Option<Storage> {
    window.local_storage().ok().flatten()
}

fn read(window: &Window, key: &str) -> Option<String> {
    storage(window)?.get_item(key).ok().flatten()
}

fn fresh_seed() -> u64 {
    let now = js_sys::Date::now();
    let random = js_sys::Math::random();
    now.to_bits() ^ random.to_bits().rotate_left(17)
}

fn warn(context: &str, error: &JsValue) {
    web_sys::console::warn_2(&JsValue::from_str(context), error);
}

fn observe_resize(window: &Window, shared: &Shared) {
    let Some(wrap) = window
        .document()
        .and_then(|document| document.get_element_by_id("board"))
        .and_then(|board| board.parent_element())
    else {
        return;
    };
    let app = Rc::clone(shared);
    let callback = Closure::<dyn FnMut()>::new(move || {
        app.borrow_mut().paint();
    });
    match web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()) {
        Ok(observer) => {
            observer.observe(&wrap);
            // The observer lives exactly as long as the page, and the page is
            // the app: dropping it here would stop every future resize.
            std::mem::forget(observer);
        }
        Err(error) => warn("could not watch the board size", &error),
    }
    callback.forget();
}

/// Let the cards move from now on.
///
/// The board is built with `.instant`, which turns every transition off, so the
/// first paint puts the whole deal on the table instead of animating a hundred
/// cards out of the top-left corner. One frame later the class comes off and
/// moves travel.
fn release_instant_layout(window: &Window, shared: &Shared) {
    let app = Rc::clone(shared);
    let closure = Closure::<dyn FnMut()>::new(move || {
        // The density class is not a transition and must survive this: the
        // compact face is what the first paint chose for this size, and clearing
        // it here would put the big centre pips back on a board too small for
        // them.
        app.borrow().settle();
    });
    let _ = window.request_animation_frame(closure.as_ref().unchecked_ref());
    closure.forget();
}

fn start_clock(window: &Window, shared: &Shared) {
    let last = Rc::new(Cell::new(js_sys::Date::now()));
    let app = Rc::clone(shared);
    let closure = Closure::<dyn FnMut()>::new(move || {
        let now = js_sys::Date::now();
        let delta = ((now - last.get()) / 1_000.0).max(0.0);
        last.set(now);
        let mut app = app.borrow_mut();
        let pending = app.pending.get() + delta;
        let whole = pending.floor();
        app.pending.set(pending - whole);
        if whole >= 1.0 {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let seconds = whole.min(f64::from(u32::MAX)) as u32;
            app.tick(seconds);
        }
    });
    match window.set_interval_with_callback_and_timeout_and_arguments_0(
        closure.as_ref().unchecked_ref(),
        TICK_MS,
    ) {
        Ok(_) => closure.forget(),
        Err(error) => warn("could not start the clock", &error),
    }
}

// -- offline -------------------------------------------------------------

/// Register the service worker, and release any stale registration.
///
/// Deliberately silent: the worker is a background concern, and an app that
/// stays usable without it is the point. Registration is awaited, but never
/// before the app is interactive: offline support is not worth a blank screen.
async fn keep_offline(window: Window) {
    let container = window.navigator().service_worker();
    let options = web_sys::RegistrationOptions::new();
    options.set_scope(SCOPE);
    let registered =
        JsFuture::from(container.register_with_options("./service-worker.js", &options)).await;
    let registered = match registered {
        Ok(value) => value,
        Err(error) => {
            warn("service worker registration failed", &error);
            return;
        }
    };
    if let Ok(registered) = registered.dyn_into::<web_sys::ServiceWorkerRegistration>() {
        nudge_worker_update(&registered).await;
    }
    release_stale_registrations(&window).await;
}

/// Ask this app's worker to take over as soon as it can, because the page that
/// asks reloads as the handover happens. The worker never swaps itself in
/// unasked.
async fn nudge_worker_update(registration: &web_sys::ServiceWorkerRegistration) {
    let Some(window) = web_sys::window() else {
        return;
    };
    watch_for_controller_change(&window);

    if let Some(worker) = registration.waiting() {
        if worker.state() == web_sys::ServiceWorkerState::Installed {
            ask_worker_in(&worker);
            return;
        }
    }

    let installing = match registration.installing() {
        Some(worker) => Some(worker),
        None => update_found(registration).await,
    };
    let Some(worker) = installing else {
        return;
    };
    if wait_for_state_change(&worker).await.is_err() {
        warn("could not wait for the worker to install", &JsValue::NULL);
        return;
    }
    if worker.state() == web_sys::ServiceWorkerState::Installed {
        ask_worker_in(&worker);
    }
}

fn ask_worker_in(worker: &web_sys::ServiceWorker) {
    worker.post_message(&JsValue::from_str("SKIP_WAITING")).ok();
}

fn watch_for_controller_change(window: &web_sys::Window) {
    thread_local! {
        static WATCHING: Cell<bool> = const { Cell::new(false) };
    }
    if WATCHING.with(Cell::get) {
        return;
    }
    WATCHING.with(|watching| watching.set(true));

    let container = window.navigator().service_worker();
    // Whether this page was already being served by a worker. It matters
    // exactly once, on the first visit ever: the worker claims the page as it
    // activates, which fires `controllerchange` — but nothing stale was
    // replaced, so there is nothing to reload for, and reloading would only
    // cost the reader a second start.
    let replacing = container.controller().is_some();
    let handler = Closure::<dyn FnMut(JsValue)>::new(move |_event: JsValue| {
        if !replacing {
            return;
        }
        if let Some(window) = web_sys::window() {
            window.location().reload().ok();
        }
    });
    let _ = container
        .add_event_listener_with_callback("controllerchange", handler.as_ref().unchecked_ref());
    handler.forget();
}

async fn update_found(
    registration: &web_sys::ServiceWorkerRegistration,
) -> Option<web_sys::ServiceWorker> {
    let event = JsFuture::from(js_sys::Promise::new(&mut |resolve, _reject| {
        let resolved = Closure::<dyn FnMut(JsValue)>::new(move |value: JsValue| {
            resolve.call1(&JsValue::NULL, &value).ok();
        });
        let _ = registration
            .add_event_listener_with_callback("updatefound", resolved.as_ref().unchecked_ref());
        resolved.forget();
    }))
    .await
    .ok()?;
    let event = event.dyn_into::<web_sys::Event>().ok()?;
    let announced = event
        .target()?
        .dyn_into::<web_sys::ServiceWorkerRegistration>()
        .ok()?;
    announced.installing()
}

async fn wait_for_state_change(worker: &web_sys::ServiceWorker) -> Result<(), JsValue> {
    JsFuture::from(js_sys::Promise::new(&mut |resolve, _reject| {
        let resolved = Closure::<dyn FnMut(JsValue)>::new(move |value: JsValue| {
            resolve.call1(&JsValue::NULL, &value).ok();
        });
        let _ = worker
            .add_event_listener_with_callback("statechange", resolved.as_ref().unchecked_ref());
        resolved.forget();
    }))
    .await
    .map(|_| ())
}

async fn release_stale_registrations(window: &Window) {
    let container = window.navigator().service_worker();
    let Ok(registrations) = JsFuture::from(container.get_registrations()).await else {
        return;
    };
    let Ok(array) = registrations.dyn_into::<js_sys::Array>() else {
        return;
    };

    let Ok(home) = window.location().href() else {
        return;
    };
    let Ok(ours) = web_sys::Url::new_with_base(&home, "./") else {
        return;
    };
    let ours = ours.href();

    for entry in array.iter() {
        let Ok(registration) = entry.dyn_into::<web_sys::ServiceWorkerRegistration>() else {
            continue;
        };
        let scope = registration.scope();
        if ours.starts_with(&scope) {
            continue;
        }
        let script = registration
            .active()
            .map(|worker| worker.script_url())
            .unwrap_or_default();
        if script_belongs_to_app(&script, &ours) {
            match registration.unregister() {
                Ok(promise) => {
                    let _ = JsFuture::from(promise).await;
                }
                Err(error) => warn("stale service worker could not be released", &error),
            }
        }
    }
}

fn script_belongs_to_app(script: &str, ours: &str) -> bool {
    match web_sys::Url::new(script) {
        Ok(url) => url.href().strip_suffix("service-worker.js") == Some(ours),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_is_marked_up_from_its_own_rank_and_suit() {
        let html = card_html(crate::card::Card::new(
            crate::card::Rank::King,
            crate::card::Suit::Clubs,
        ));
        assert!(html.contains("K"));
        assert!(html.contains("♣"));
        assert!(html.contains("class=\"face\""));
        assert!(html.contains("class=\"back\""));
    }

    #[test]
    fn the_pile_names_round_trip_through_the_dom_tokens() {
        for pile in Pile::ALL {
            assert_eq!(Pile::from_slug(&pile.slug()), Some(pile));
        }
        assert_eq!(Pile::from_slug("tableau-10"), None);
        assert_eq!(Pile::from_slug("foundation-0"), None);
    }

    #[test]
    fn later_columns_and_lower_cards_stack_above_earlier_ones() {
        assert!(z_index(Pile::Tableau(0), 3) > z_index(Pile::Tableau(0), 2));
        assert!(z_index(Pile::Tableau(9), 0) > z_index(Pile::Tableau(0), 20));
    }

    #[test]
    fn a_coordinate_is_clamped_rather_than_wrapped() {
        assert_eq!(f64_to_f32(3.6), 4.0);
        assert_eq!(f64_to_f32(-1.2), -1.0);
        assert_eq!(f64_to_f32(1e12), f32::MAX);
    }
}
