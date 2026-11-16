//! Dice expressions and roll requests.
//!
//! The engine never rolls. It asks for a roll by describing it with a
//! [`RollSpec`], and whoever drives the engine answers with a number inside
//! the spec's range.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

/// A sum of identical dice plus a flat bonus, such as `3d6`, `1d8+4` or `5`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dice {
    pub count: u32,
    pub sides: u32,
    pub bonus: i32,
}

impl Dice {
    pub const fn new(count: u32, sides: u32, bonus: i32) -> Self {
        Dice { count, sides, bonus }
    }

    pub const fn flat(amount: i32) -> Self {
        Dice { count: 0, sides: 0, bonus: amount }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let err = || format!("`{text}` is not a dice expression; expected a form like `2d6`, `1d8+3` or `5`");
        let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        if t.is_empty() {
            return Err(err());
        }
        let split = t[1..].find(['+', '-']).map(|i| i + 1);
        let (dice, bonus) = match split {
            Some(i) => (&t[..i], Some(t[i..].parse::<i32>().map_err(|_| err())?)),
            None => (&t[..], None),
        };
        match dice.split_once('d') {
            Some((count, sides)) => {
                let count = if count.is_empty() { 1 } else { count.parse().map_err(|_| err())? };
                let sides: u32 = sides.parse().map_err(|_| err())?;
                if sides == 0 {
                    return Err(err());
                }
                Ok(Dice { count, sides, bonus: bonus.unwrap_or(0) })
            }
            None if bonus.is_none() => Ok(Dice::flat(dice.parse().map_err(|_| err())?)),
            None => Err(err()),
        }
    }

    /// True when the result does not depend on a roll.
    pub fn is_fixed(&self) -> bool {
        self.count == 0 || self.sides <= 1
    }

    pub fn min(&self) -> u32 {
        (i64::from(self.count) + i64::from(self.bonus)).max(0) as u32
    }

    pub fn max(&self) -> u32 {
        (i64::from(self.count) * i64::from(self.sides) + i64::from(self.bonus)).max(0) as u32
    }

    /// The same expression with twice the dice, as a critical hit rolls.
    pub fn doubled(&self) -> Dice {
        Dice { count: self.count * 2, ..*self }
    }
}

impl fmt::Display for Dice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.count == 0 {
            return write!(f, "{}", self.bonus);
        }
        write!(f, "{}d{}", self.count, self.sides)?;
        match self.bonus {
            0 => Ok(()),
            b if b > 0 => write!(f, "+{b}"),
            b => write!(f, "{b}"),
        }
    }
}

impl Serialize for Dice {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Dice {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = Dice;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a dice expression such as `2d6+3`, or a whole number")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Dice, E> {
                Dice::parse(v).map_err(E::custom)
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Dice, E> {
                i32::try_from(v).map(Dice::flat).map_err(|_| E::custom("number out of range"))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Dice, E> {
                i32::try_from(v).map(Dice::flat).map_err(|_| E::custom("number out of range"))
            }
        }
        d.deserialize_any(V)
    }
}

/// How many d20s a test rolls and which one it keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Advantage,
    Disadvantage,
}

impl Mode {
    /// Advantage and disadvantage cancel, however many sources each has.
    pub fn from_flags(advantage: bool, disadvantage: bool) -> Mode {
        match (advantage, disadvantage) {
            (true, false) => Mode::Advantage,
            (false, true) => Mode::Disadvantage,
            _ => Mode::Normal,
        }
    }
}

/// A roll the engine is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollSpec {
    /// A d20 test. The answer is the die that was kept, from 1 to 20.
    D20(Mode),
    /// A sum of dice. The answer is the total, bonus included.
    Sum(Dice),
}

impl RollSpec {
    /// The inclusive range a valid answer falls in.
    pub fn range(&self) -> (u32, u32) {
        match self {
            RollSpec::D20(_) => (1, 20),
            RollSpec::Sum(d) => (d.min(), d.max()),
        }
    }
}

impl fmt::Display for RollSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RollSpec::D20(Mode::Normal) => f.write_str("1d20"),
            RollSpec::D20(Mode::Advantage) => f.write_str("2d20 keep highest"),
            RollSpec::D20(Mode::Disadvantage) => f.write_str("2d20 keep lowest"),
            RollSpec::Sum(d) => d.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(Dice::parse("3d6"), Ok(Dice::new(3, 6, 0)));
        assert_eq!(Dice::parse("1d8+4"), Ok(Dice::new(1, 8, 4)));
        assert_eq!(Dice::parse("d4 - 1"), Ok(Dice::new(1, 4, -1)));
        assert_eq!(Dice::parse("5"), Ok(Dice::flat(5)));
        assert_eq!(Dice::parse("-2"), Ok(Dice::flat(-2)));
    }

    #[test]
    fn rejects_nonsense() {
        for bad in ["", "d", "2d", "2d0", "two", "1d6+", "5+3", "1d6+x"] {
            assert!(Dice::parse(bad).is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn displays_what_it_parsed() {
        for text in ["3d6", "1d8+4", "1d4-1", "5"] {
            assert_eq!(Dice::parse(text).unwrap().to_string(), text);
        }
    }

    #[test]
    fn ranges() {
        assert_eq!(RollSpec::Sum(Dice::new(1, 8, 4)).range(), (5, 12));
        assert_eq!(RollSpec::Sum(Dice::new(1, 4, -3)).range(), (0, 1));
        assert_eq!(RollSpec::D20(Mode::Advantage).range(), (1, 20));
        assert_eq!(Dice::new(2, 6, 1).doubled(), Dice::new(4, 6, 1));
    }
}
