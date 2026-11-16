//! Events, and the inputs the engine asks for.

use crate::data::{Ability, AttackNode, Change, EffectDef, EventKind, Role, SaveNode};
use crate::dice::RollSpec;
use crate::state::{Id, Line};
use std::sync::Arc;

/// A committed event's position in the log.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(pub u32);

/// Everything that can happen in a fight.
///
/// Mutations change state when they commit. Moments change nothing and exist
/// so rules have something to respond to. Structure events mark time.
/// `Rolled` and `Chose` are the only events that come from outside. `Ac` is a
/// question: it passes through before-rules and is never committed.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    // Mutations
    Damage {
        target: Id,
        /// The amount rolled. See [`Event::damage_dealt`] for what lands.
        amount: u32,
        kind: String,
        source: Option<Id>,
        crit: bool,
        resist: bool,
        vulnerable: bool,
        immune: bool,
        reduce: u32,
    },
    Heal {
        target: Id,
        amount: u32,
    },
    TempHp {
        target: Id,
        amount: u32,
    },
    ResourceSpent {
        who: Id,
        resource: String,
        n: u32,
    },
    ResourceRestored {
        who: Id,
        resource: String,
        n: u32,
    },
    EffectAttached {
        target: Id,
        source: Option<Id>,
        effect: Arc<EffectDef>,
    },
    EffectRemoved {
        target: Id,
        effect: u32,
        name: String,
    },
    Moved {
        who: Id,
        to: Line,
    },
    Engaged {
        a: Id,
        b: Id,
    },
    Disengaged {
        a: Id,
        b: Id,
    },
    Dropped {
        who: Id,
    },
    Died {
        who: Id,
    },

    // Moments
    ActionUsed {
        actor: Id,
        action: String,
        targets: Vec<Id>,
    },
    Attack {
        attacker: Id,
        target: Id,
        action: String,
        bonus: i32,
        advantage: bool,
        disadvantage: bool,
        melee: bool,
        /// What the attack does when it lands or misses.
        spec: Arc<AttackNode>,
    },
    Hit(Outcome),
    Miss(Outcome),
    Save {
        who: Id,
        ability: Ability,
        dc: u32,
        bonus: i32,
        source: Option<Id>,
        advantage: bool,
        disadvantage: bool,
        auto_fail: bool,
        spec: Arc<SaveNode>,
    },
    SaveFailed {
        save: EventId,
        who: Id,
        natural: u32,
        total: i32,
        dc: u32,
    },
    SaveSucceeded {
        save: EventId,
        who: Id,
        natural: u32,
        total: i32,
        dc: u32,
    },
    DeathSave {
        who: Id,
    },

    // Structure
    FightStarted,
    RoundStarted {
        round: u32,
    },
    TurnStarted {
        who: Id,
    },
    TurnEnded {
        who: Id,
    },
    FightEnded {
        winner: Option<String>,
    },
    /// A marker for a proposed event that never took effect.
    Fizzled {
        event: Box<Event>,
        reason: Fizzle,
    },

    // Inputs
    Rolled {
        spec: RollSpec,
        purpose: Purpose,
        value: u32,
    },
    Chose {
        actor: Id,
        prompt: Constraint,
        choice: Choice,
        /// True when it was the only valid option and the engine took it
        /// without asking.
        forced: bool,
    },

    // Query
    Ac {
        who: Id,
        value: i32,
    },
}

/// The result of an attack roll, shared by `Hit` and `Miss`. It repeats
/// what rules most often ask about the attack so they need not look it up.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub attack: EventId,
    pub attacker: Id,
    pub target: Id,
    pub action: String,
    pub natural: u32,
    pub total: i32,
    pub ac: i32,
    pub crit: bool,
    /// The roll was made with advantage.
    pub advantage: bool,
    /// The roll was made with disadvantage.
    pub disadvantage: bool,
    pub melee: bool,
}

/// Why a proposed event fizzled.
#[derive(Clone, Debug, PartialEq)]
pub enum Fizzle {
    /// The event's own requirement did not hold when its turn came.
    Requirement(&'static str),
    /// A rule cancelled it. The id is the event the cancelling effect ran
    /// under, when there is one.
    Cancelled(Option<EventId>),
}

/// What a requested roll is for. A core rule reads this when the roll
/// arrives, which is how a roll leads to its consequence without anything
/// waiting for a return value.
#[derive(Clone, Debug, PartialEq)]
pub enum Purpose {
    Initiative { who: Id },
    Attack { attack: EventId },
    Save { save: EventId },
    Damage { target: Id, kind: String, source: Option<Id>, crit: bool, half: bool },
    Heal { target: Id },
    TempHp { target: Id },
    DeathSave { who: Id },
    Recharge { who: Id, resource: String, min: u32 },
}

impl Purpose {
    /// For rolls that are nothing but an amount, the event that amount becomes.
    pub fn amount_event(&self, value: u32) -> Option<Event> {
        match self {
            Purpose::Damage { target, kind, source, crit, half } => Some(Event::Damage {
                target: *target,
                amount: if *half { value / 2 } else { value },
                kind: kind.clone(),
                source: *source,
                crit: *crit,
                resist: false,
                vulnerable: false,
                immune: false,
                reduce: 0,
            }),
            Purpose::Heal { target } => Some(Event::Heal { target: *target, amount: value }),
            Purpose::TempHp { target } => Some(Event::TempHp { target: *target, amount: value }),
            _ => None,
        }
    }
}

/// What the engine needs next.
#[derive(Clone, Debug, PartialEq)]
pub enum Expect {
    /// A decision by `actor`, limited by the constraint.
    Choice {
        actor: Id,
        constraint: Constraint,
    },
    Roll {
        spec: RollSpec,
        purpose: Purpose,
    },
    /// Nothing. The fight is over.
    Done,
}

/// The kind of decision being asked for. It limits the answer; it is not a
/// menu. [`crate::engine::Engine::options`] lists what satisfies it.
#[derive(Clone, Debug, PartialEq)]
pub enum Constraint {
    /// The next thing to do on a turn: an affordable action with legal
    /// targets, or ending the turn.
    Step,
    /// Whether to apply an optional rule to the pending event `on`.
    React { rule: String, on: Box<Event> },
    /// Who to aim one part of a larger action at.
    Target { action: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Act { action: String, targets: Vec<Id> },
    EndTurn,
    React(bool),
    Target(Id),
}

/// The two things that enter the engine from outside.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Chose(Choice),
    Rolled(u32),
}

impl Event {
    pub fn kind(&self) -> EventKind {
        match self {
            Event::Damage { .. } => EventKind::Damage,
            Event::Heal { .. } => EventKind::Heal,
            Event::TempHp { .. } => EventKind::TempHp,
            Event::ResourceSpent { .. } => EventKind::ResourceSpent,
            Event::ResourceRestored { .. } => EventKind::ResourceRestored,
            Event::EffectAttached { .. } => EventKind::EffectAttached,
            Event::EffectRemoved { .. } => EventKind::EffectRemoved,
            Event::Moved { .. } => EventKind::Moved,
            Event::Engaged { .. } => EventKind::Engaged,
            Event::Disengaged { .. } => EventKind::Disengaged,
            Event::Dropped { .. } => EventKind::Dropped,
            Event::Died { .. } => EventKind::Died,
            Event::ActionUsed { .. } => EventKind::ActionUsed,
            Event::Attack { .. } => EventKind::Attack,
            Event::Hit(_) => EventKind::Hit,
            Event::Miss(_) => EventKind::Miss,
            Event::Save { .. } => EventKind::Save,
            Event::SaveFailed { .. } => EventKind::SaveFailed,
            Event::SaveSucceeded { .. } => EventKind::SaveSucceeded,
            Event::DeathSave { .. } => EventKind::DeathSave,
            Event::FightStarted => EventKind::FightStarted,
            Event::RoundStarted { .. } => EventKind::RoundStarted,
            Event::TurnStarted { .. } => EventKind::TurnStarted,
            Event::TurnEnded { .. } => EventKind::TurnEnded,
            Event::FightEnded { .. } => EventKind::FightEnded,
            Event::Fizzled { .. } => EventKind::Fizzled,
            Event::Rolled { .. } => EventKind::Rolled,
            Event::Chose { .. } => EventKind::Chose,
            Event::Ac { .. } => EventKind::Ac,
        }
    }

    pub fn is_input(&self) -> bool {
        matches!(self, Event::Rolled { .. } | Event::Chose { .. })
    }

    pub fn is_structure(&self) -> bool {
        matches!(
            self,
            Event::FightStarted
                | Event::RoundStarted { .. }
                | Event::TurnStarted { .. }
                | Event::TurnEnded { .. }
                | Event::FightEnded { .. }
        )
    }

    /// Whoever is doing the thing: the attacker, the actor, the source of
    /// the damage.
    pub fn actor(&self) -> Option<Id> {
        match self {
            Event::ActionUsed { actor, .. } | Event::Chose { actor, .. } => Some(*actor),
            Event::Attack { attacker, .. } => Some(*attacker),
            Event::Hit(o) | Event::Miss(o) => Some(o.attacker),
            Event::Damage { source, .. } | Event::EffectAttached { source, .. } | Event::Save { source, .. } => *source,
            _ => None,
        }
    }

    /// Whoever it is done to. An action with several targets has none in
    /// particular.
    pub fn target(&self) -> Option<Id> {
        match self {
            Event::Damage { target, .. }
            | Event::Heal { target, .. }
            | Event::TempHp { target, .. }
            | Event::EffectAttached { target, .. }
            | Event::EffectRemoved { target, .. }
            | Event::Attack { target, .. } => Some(*target),
            Event::Hit(o) | Event::Miss(o) => Some(o.target),
            Event::Save { who, .. } | Event::SaveFailed { who, .. } | Event::SaveSucceeded { who, .. } => Some(*who),
            Event::ActionUsed { targets, .. } if targets.len() == 1 => Some(targets[0]),
            _ => None,
        }
    }

    pub fn source(&self) -> Option<Id> {
        match self {
            Event::Damage { source, .. } | Event::EffectAttached { source, .. } | Event::Save { source, .. } => *source,
            _ => None,
        }
    }

    /// The creature the event is about.
    pub fn subject(&self) -> Option<Id> {
        match self {
            Event::ResourceSpent { who, .. }
            | Event::ResourceRestored { who, .. }
            | Event::Moved { who, .. }
            | Event::Dropped { who }
            | Event::Died { who }
            | Event::DeathSave { who }
            | Event::TurnStarted { who }
            | Event::TurnEnded { who }
            | Event::Ac { who, .. } => Some(*who),
            Event::ActionUsed { actor, .. } | Event::Chose { actor, .. } => Some(*actor),
            Event::Attack { attacker, .. } => Some(*attacker),
            Event::Hit(o) | Event::Miss(o) => Some(o.attacker),
            _ => self.target(),
        }
    }

    /// Whether `id` plays `role` in this event.
    pub fn involves(&self, role: Role, id: Id) -> bool {
        match role {
            Role::Any => true,
            Role::Me => match self {
                Event::Engaged { a, b } | Event::Disengaged { a, b } => *a == id || *b == id,
                _ => self.subject() == Some(id),
            },
            Role::Attacker | Role::Actor => self.actor() == Some(id),
            Role::Source => self.source() == Some(id),
            Role::Target => match self {
                Event::ActionUsed { targets, .. } => targets.contains(&id),
                _ => self.target() == Some(id),
            },
        }
    }

    /// The action this event is part of, as the creature that owns it and
    /// the action's name.
    pub fn action_ref(&self) -> Option<(Id, &str)> {
        match self {
            Event::ActionUsed { actor, action, .. } => Some((*actor, action)),
            Event::Attack { attacker, action, .. } => Some((*attacker, action)),
            Event::Hit(o) | Event::Miss(o) => Some((o.attacker, &o.action)),
            _ => None,
        }
    }

    /// What a `Damage` event takes off its target once immunity, flat
    /// reduction, resistance and vulnerability are counted. Several sources
    /// of resistance count once, as the rules say.
    pub fn damage_dealt(&self) -> u32 {
        let Event::Damage { amount, resist, vulnerable, immune, reduce, .. } = self else { return 0 };
        if *immune {
            return 0;
        }
        let mut dealt = amount.saturating_sub(*reduce);
        if *resist {
            dealt /= 2;
        }
        if *vulnerable {
            dealt *= 2;
        }
        dealt
    }
}

impl Change {
    /// The event with this change made to it. Fields the event does not have
    /// are left alone.
    pub fn apply(&self, event: &Event) -> Event {
        let mut out = event.clone();
        let or = |flag: &mut bool, set: Option<bool>| {
            if let Some(v) = set {
                *flag = v;
            }
        };
        match &mut out {
            Event::Attack { advantage, disadvantage, bonus, .. } => {
                or(advantage, self.advantage);
                or(disadvantage, self.disadvantage);
                *bonus += self.bonus.unwrap_or(0);
            }
            Event::Save { advantage, disadvantage, bonus, auto_fail, .. } => {
                or(advantage, self.advantage);
                or(disadvantage, self.disadvantage);
                or(auto_fail, self.auto_fail);
                *bonus += self.bonus.unwrap_or(0);
            }
            Event::Hit(o) => or(&mut o.crit, self.crit),
            Event::Damage { resist, vulnerable, immune, reduce, .. } => {
                or(resist, self.resist);
                or(vulnerable, self.vulnerable);
                or(immune, self.immune);
                *reduce += self.reduce.unwrap_or(0);
            }
            Event::Ac { value, .. } => *value += self.bonus.unwrap_or(0),
            Event::ResourceSpent { n, .. } => {
                if let Some(v) = self.n {
                    *n = v;
                }
            }
            _ => {}
        }
        out
    }
}
