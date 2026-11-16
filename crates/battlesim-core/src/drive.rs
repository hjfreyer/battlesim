//! Proposers: things that supply the inputs the engine asks for.
//!
//! The engine does not care where an input comes from. A policy, a dice
//! roller, a recorded log and a test script are all the same to it. This
//! module has the simplest of each, enough to run a fight end to end.

use crate::dice::{Mode, RollSpec};
use crate::engine::{Engine, Invalid};
use crate::event::{Choice, Constraint, Expect, Input};
use crate::state::Id;

pub trait DiceSource {
    /// An answer to the roll, inside `spec.range()`.
    fn roll(&mut self, spec: &RollSpec) -> u32;
}

pub trait Player {
    /// Picks one of `options`, which is never empty.
    fn choose(&mut self, engine: &Engine, actor: Id, constraint: &Constraint, options: &[Choice]) -> Choice;
}

/// A small deterministic generator (SplitMix64). Good enough for dice, and
/// the same seed gives the same fight on every platform.
#[derive(Clone, Debug)]
pub struct SeededDice(u64);

impl SeededDice {
    pub fn new(seed: u64) -> Self {
        SeededDice(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A whole number from 0 up to but not including `n`.
    pub fn below(&mut self, n: u32) -> u32 {
        (((self.next() >> 32) * u64::from(n)) >> 32) as u32
    }

    fn die(&mut self, sides: u32) -> u32 {
        self.below(sides) + 1
    }
}

impl DiceSource for SeededDice {
    fn roll(&mut self, spec: &RollSpec) -> u32 {
        match spec {
            RollSpec::D20(Mode::Normal) => self.die(20),
            RollSpec::D20(Mode::Advantage) => self.die(20).max(self.die(20)),
            RollSpec::D20(Mode::Disadvantage) => self.die(20).min(self.die(20)),
            RollSpec::Sum(dice) => {
                let rolled: u32 = (0..dice.count).map(|_| self.die(dice.sides)).sum();
                (i64::from(rolled) + i64::from(dice.bonus)).max(0) as u32
            }
        }
    }
}

/// Always takes the first option: the first listed action at the first legal
/// target, and yes to every optional rule. Not a policy, just a way to make
/// a fight happen.
pub struct FirstOption;

impl Player for FirstOption {
    fn choose(&mut self, _: &Engine, _: Id, _: &Constraint, options: &[Choice]) -> Choice {
        options[0].clone()
    }
}

/// Picks uniformly among the options.
pub struct RandomPlayer(pub SeededDice);

impl Player for RandomPlayer {
    fn choose(&mut self, _: &Engine, _: Id, _: &Constraint, options: &[Choice]) -> Choice {
        options[self.0.below(options.len() as u32) as usize].clone()
    }
}

/// Runs the fight to its end, asking `player` for every choice and `dice`
/// for every roll.
pub fn run(engine: &mut Engine, player: &mut dyn Player, dice: &mut dyn DiceSource) -> Result<(), Invalid> {
    loop {
        let input = match engine.expects() {
            Expect::Done => return Ok(()),
            Expect::Roll { spec, .. } => Input::Rolled(dice.roll(spec)),
            Expect::Choice { actor, constraint } => {
                Input::Chose(player.choose(engine, *actor, constraint, &engine.options()))
            }
        };
        engine.push(input)?;
    }
}

/// Pushes recorded inputs into the engine in order.
pub fn replay(engine: &mut Engine, inputs: impl IntoIterator<Item = Input>) -> Result<(), Invalid> {
    inputs.into_iter().try_for_each(|input| engine.push(input))
}
