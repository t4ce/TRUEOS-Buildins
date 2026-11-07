//! Host execution of the production frame writer and renderer with native I/O.
#![no_std]
extern crate std;
#[macro_use]
extern crate alloc;
#[path = "buffered_output.rs"]
mod buffered_output;
#[path = "screen.rs"]
mod screen;
#[path = "terminal_output.rs"]
mod terminal_output;
