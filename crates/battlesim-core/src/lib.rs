//! An event-and-rule engine for simulating D&D fights.
//!
//! The engine is built from five ideas: state, events, an explicit stack,
//! rules, and inputs. Only a committed event changes state. Rules respond to
//! events and propose more. The engine itself is passive: it says what input
//! it needs with [`Engine::expects`] and takes it with [`Engine::push`].
//! Policies, dice and replays all sit outside, in whatever supplies inputs.
//!
//! ```
//! use battlesim_core::{Combatant, Engine, Setup, StatBlock, drive};
//! use std::sync::Arc;
//!
//! let wolf = Arc::new(StatBlock::from_yaml("
//! name: Wolf
//! ac: 12
//! hp: 11
//! actions:
//!   - name: Bite
//!     target: melee
//!     effect:
//!       - attack: {bonus: 4, on_hit: [{damage: {dice: 1d6+2, type: piercing}}]}
//! ").unwrap());
//!
//! let setup = Setup {
//!     combatants: vec![Combatant::new("Grey", "north", &wolf), Combatant::new("Red", "south", &wolf)],
//!     ..Setup::default()
//! };
//! let mut engine = Engine::new(&setup).unwrap();
//! drive::run(&mut engine, &mut drive::FirstOption, &mut drive::SeededDice::new(7)).unwrap();
//! assert!(engine.state().ended);
//! ```

pub mod core_rules;
pub mod data;
pub mod dice;
pub mod drive;
pub mod engine;
pub mod event;
pub mod expr;
pub mod rule;
pub mod state;
pub mod trace;
pub mod traits;

pub use data::StatBlock;
pub use dice::{Dice, Mode, RollSpec};
pub use engine::{Config, Engine, Fault, Invalid};
pub use event::{Choice, Constraint, Event, EventId, Expect, Fizzle, Input, Purpose};
pub use rule::{Before, Entry, Log, Proposal, Rule, Scope, Step, View};
pub use state::{Combatant, Id, Life, Line, Setup, State};
