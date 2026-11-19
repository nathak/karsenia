//! The live view: what a chat is doing, as a web page in a browser (127.0.0.1, behind a token of its own). `derive` turns the events of a
//! chat into a feed of items (prompts, calls with their risk and guard marks, asks, turn ends), `mask` makes every text fit to leave the
//! app (secrets hidden, length cut) before it is put into an item, and `server` serves the page and the feed, read only.
//! Nothing here writes to the disk: the feed is kept in memory and dropped when the connection ends.
pub mod derive;
pub mod feed;
pub mod mask;
pub mod server;

pub use derive::Deriver;
pub use feed::{Item, Prompts};
