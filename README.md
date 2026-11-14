# Spider Solitaire

Spider solitaire that works offline: one, two and four suit deals, hints,
unlimited undo and a full keyboard, in a small static web app. It installs as an
app, and after one visit it plays with the network off.

All of the rules, the scoring and the rendering are Rust; the page is a static
shell and `cargo build` writes the publishable site into `dist/`.

## Play

- Tap a card to pick up its run, then tap where it should go. Drag works too,
  and a double tap sends the run to the best column for it.
- Build every column downwards, in any suit. A **descending sequence in one
  suit** can be picked up and moved together, and a run of thirteen from king
  down to ace is lifted off the board. Eight of those win the game.
- **Deal** puts one card on every column, and is refused while any column is
  empty — that is the rule that makes an empty column a resource.
- **Undo** takes back anything. **Hint** lights the move worth making.
- The menu holds the two choices that define the deal, and both apply to the
  *next* one so that changing your mind never discards a game in progress:

  | Difficulty | Suits | What it means |
  |-----------|-------|---------------|
  | Easy      | one   | every card is a spade, so any descending group moves |
  | Medium    | two   | spades and hearts, four of each card |
  | Hard      | four  | the full two-deck game, and runs are worth earning |

  Scoring is **Standard** (500 to start, a point per move and per undo, 100 for
  each completed run) or **No score**.

- Keyboard: arrow keys move around the board, Enter picks up or drops, Space
  deals, `U` undoes, `H` hints, Escape puts a held run back.

## Build

Two steps, because there are two targets. Nothing generated is committed.

```bash
rustup target add wasm32-unknown-unknown
# the generator must be the version Cargo.lock records
cargo install wasm-bindgen-cli --locked \
  --version "$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;}' Cargo.lock)"

# 1. the site: compile to wasm and generate the JS bindings
cargo build --locked --lib --target wasm32-unknown-unknown --release
wasm-bindgen --target web --no-typescript --out-dir dist --out-name app \
  target/wasm32-unknown-unknown/release/spider_solitaire.wasm

# 2. the rest of the site
touch build.rs
cargo build --release --locked
```

That leaves a publishable `dist/`. **The order matters**: step 2 derives the
service worker's cache name from the wasm, so running it first would pin that to
the previous build. The `touch` matters too — `build.rs` writes into the source
tree, so cargo would otherwise skip the second build and copy nothing.

`cargo test --locked` needs none of the above.

## Layout

Four of these files are shared with
[Solitaire](https://github.com/bot-git-ai/solitaire), byte for byte:

    src/card.rs            the deck, the printed face and the stored code
    src/board.rs           where every card and pile sits, and what a point means
    src/board_ui.rs        the card elements, the painting and the gestures
    src/shell.rs           the header, the buttons, the menu, the clock, offline

And this game's own:

    src/game.rs            the rules, the score, the undo history, the words
    src/ui.rs              how this game fills a scene, and what a gesture means
    src/ui.html            the static shell
    src/style.css          the styles, including the card format
    src/service-worker.js  install, activate, fetch; one __VERSION__ placeholder
    build.rs               writes six of the nine files in dist/, derives the rest
    assets/icon.svg        the icon, committed and authoritative
    tests/shell.rs         invariants of the committed shell
    AGENTS.md              the decisions, and how to verify them

## Card format

Both games are built on the same board: `card.rs`, `board.rs`, `board_ui.rs`,
`shell.rs` and the `.card` block of the stylesheet are carried by both
repositories unchanged. An app supplies a `Rules` implementation and keeps its
rules to itself, so a fix to the board or to drag and drop is a fix in both
games — which is exactly why they were made one.

The two are separate repositories, so nothing enforces the copy; the rule is
that a change to a shared file belongs in both, and a test keeps the shared
files from acquiring one game's vocabulary.

## Licence

AGPL-3.0-only. See `LICENSE`.
