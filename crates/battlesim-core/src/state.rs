//! The state of a fight. Only a committed event changes it, through
//! [`State::apply`].

use crate::data::{
    ACTION, ActionDef, BONUS_ACTION, DEATH_SUCCESSES, Duration, EffectDef, INCAPACITATING, REACTION, Recharge,
    StatBlock, TargetRule, Until, conditions,
};
use crate::event::{Event, Purpose};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// A creature in the fight.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(pub u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Life {
    Alive,
    /// At 0 hit points and unconscious, making death saves.
    Down,
    Dead,
}

/// Where a creature stands. There is no grid: each side has a front line
/// and a back line.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Line {
    #[default]
    Front,
    Back,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Boundary {
    Start,
    End,
}

/// The turn boundary an effect ends on: the start or end of `who`'s turn
/// number `turn`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Expiry {
    pub who: Id,
    pub at: Boundary,
    pub turn: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectInstance {
    pub id: u32,
    pub def: Arc<EffectDef>,
    pub source: Option<Id>,
    pub expiry: Option<Expiry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Creature {
    pub id: Id,
    pub name: String,
    pub side: String,
    pub block: Arc<StatBlock>,
    pub hp: u32,
    pub temp_hp: u32,
    pub resources: BTreeMap<String, u32>,
    pub line: Line,
    pub effects: Vec<EffectInstance>,
    pub life: Life,
    pub initiative: Option<i32>,
    /// How many of its own turns the creature has started.
    pub turns_started: u32,
}

impl Creature {
    pub fn resource(&self, name: &str) -> u32 {
        self.resources.get(name).copied().unwrap_or(0)
    }

    pub fn has_condition(&self, name: &str) -> bool {
        self.effects.iter().any(|e| e.def.condition.as_deref() == Some(name) || e.def.name.as_deref() == Some(name))
    }

    /// Down with all three successes: no more death saves until hurt again.
    pub fn is_stable(&self) -> bool {
        self.life == Life::Down && self.resource(DEATH_SUCCESSES) == 0
    }
}

/// One creature as it enters the fight.
#[derive(Clone, Debug)]
pub struct Combatant {
    pub name: String,
    pub side: String,
    pub block: Arc<StatBlock>,
    pub line: Line,
    /// Starting hit points, when not full.
    pub hp: Option<u32>,
    /// Conditions already in place, by built-in name.
    pub conditions: Vec<String>,
    /// Pools that start at something other than full.
    pub resources: BTreeMap<String, u32>,
}

impl Combatant {
    pub fn new(name: &str, side: &str, block: &Arc<StatBlock>) -> Self {
        Combatant {
            name: name.to_string(),
            side: side.to_string(),
            block: block.clone(),
            line: Line::Front,
            hp: None,
            conditions: Vec::new(),
            resources: BTreeMap::new(),
        }
    }

    pub fn back(mut self) -> Self {
        self.line = Line::Back;
        self
    }

    pub fn hp(mut self, hp: u32) -> Self {
        self.hp = Some(hp);
        self
    }

    pub fn condition(mut self, name: &str) -> Self {
        self.conditions.push(name.to_string());
        self
    }

    pub fn resource(mut self, name: &str, amount: u32) -> Self {
        self.resources.insert(name.to_string(), amount);
        self
    }
}

/// Everything needed to start a fight.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    pub combatants: Vec<Combatant>,
    /// Pairs already in melee with each other, as indexes into `combatants`.
    pub engaged: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub creatures: Vec<Creature>,
    pub round: u32,
    /// Turn order, fixed when round one starts.
    pub order: Vec<Id>,
    /// Whose turn it is.
    pub current: Option<Id>,
    pub engagements: BTreeSet<(Id, Id)>,
    pub ended: bool,
    /// The side left standing, once the fight has ended. None is a draw.
    pub winner: Option<String>,
    next_effect: u32,
}

fn pair(a: Id, b: Id) -> (Id, Id) {
    (a.min(b), a.max(b))
}

impl State {
    pub fn new(setup: &Setup) -> Result<State, String> {
        let mut state = State {
            creatures: Vec::new(),
            round: 0,
            order: Vec::new(),
            current: None,
            engagements: BTreeSet::new(),
            ended: false,
            winner: None,
            next_effect: 0,
        };
        for (i, c) in setup.combatants.iter().enumerate() {
            c.block.validate()?;
            let mut resources = c.block.starting_resources();
            for (name, amount) in &c.resources {
                let max = c.block.max_resource(name).ok_or_else(|| format!("{}: unknown resource `{name}`", c.name))?;
                resources.insert(name.clone(), (*amount).min(max));
            }
            let id = Id(i as u32);
            state.creatures.push(Creature {
                id,
                name: c.name.clone(),
                side: c.side.clone(),
                block: c.block.clone(),
                hp: c.hp.unwrap_or(c.block.hp).min(c.block.hp),
                temp_hp: 0,
                resources,
                line: c.line,
                effects: Vec::new(),
                life: Life::Alive,
                initiative: None,
                turns_started: 0,
            });
            for name in &c.conditions {
                if !conditions().contains_key(name) {
                    return Err(format!("{}: unknown condition `{name}`", c.name));
                }
                let effect = Arc::new(EffectDef::condition(name));
                state.apply(&Event::EffectAttached { target: id, source: None, effect });
            }
        }
        for &(a, b) in &setup.engaged {
            if a.max(b) >= state.creatures.len() || a == b {
                return Err(format!("engaged pair ({a}, {b}) does not name two combatants"));
            }
            state.engagements.insert(pair(Id(a as u32), Id(b as u32)));
        }
        Ok(state)
    }

    pub fn creature(&self, id: Id) -> &Creature {
        &self.creatures[id.0 as usize]
    }

    fn creature_mut(&mut self, id: Id) -> &mut Creature {
        &mut self.creatures[id.0 as usize]
    }

    pub fn name(&self, id: Id) -> &str {
        &self.creature(id).name
    }

    pub fn in_fight(&self, id: Id) -> bool {
        self.creature(id).life != Life::Dead
    }

    /// Able to take actions and reactions: alive and not incapacitated.
    pub fn can_act(&self, id: Id) -> bool {
        let c = self.creature(id);
        c.life == Life::Alive && !INCAPACITATING.iter().any(|name| c.has_condition(name))
    }

    pub fn is_enemy(&self, a: Id, b: Id) -> bool {
        self.creature(a).side != self.creature(b).side
    }

    pub fn engaged(&self, a: Id, b: Id) -> bool {
        self.in_fight(a) && self.in_fight(b) && self.engagements.contains(&pair(a, b))
    }

    /// Everyone still in the fight that `id` is in melee with.
    pub fn engaged_with(&self, id: Id) -> impl Iterator<Item = Id> + '_ {
        self.creatures.iter().map(|c| c.id).filter(move |&other| self.engaged(id, other))
    }

    /// Reachable in melee: in the front line, or with nobody left standing
    /// in front to screen.
    pub fn exposed(&self, id: Id) -> bool {
        let c = self.creature(id);
        c.line == Line::Front
            || !self.creatures.iter().any(|o| o.side == c.side && o.line == Line::Front && o.life == Life::Alive)
    }

    pub fn max_resource(&self, id: Id, name: &str) -> u32 {
        self.creature(id).block.max_resource(name).unwrap_or(0)
    }

    pub fn can_pay(&self, id: Id, uses: &BTreeMap<String, u32>) -> bool {
        let c = self.creature(id);
        uses.iter().all(|(name, n)| c.resource(name) >= *n)
    }

    /// Able to declare the action and pay for it when it takes effect.
    pub fn affordable(&self, id: Id, action: &ActionDef) -> bool {
        action.cost.pool().is_none_or(|pool| self.creature(id).resource(pool) >= 1) && self.can_pay(id, &action.uses)
    }

    pub fn legal_targets(&self, actor: Id, rule: TargetRule) -> Vec<Id> {
        let pick = |test: &dyn Fn(Id) -> bool| -> Vec<Id> {
            self.creatures.iter().map(|c| c.id).filter(|&id| self.in_fight(id) && test(id)).collect()
        };
        match rule {
            TargetRule::None => Vec::new(),
            TargetRule::Me => vec![actor],
            TargetRule::Melee => pick(&|id| self.is_enemy(actor, id) && self.exposed(id)),
            TargetRule::Ranged => pick(&|id| self.is_enemy(actor, id)),
            TargetRule::Ally => pick(&|id| !self.is_enemy(actor, id)),
        }
    }

    /// The next creature in turn order still in the fight, and whether
    /// reaching it wraps into a new round. `after` None means the top.
    pub fn next_turn(&self, after: Option<Id>) -> Option<(Id, bool)> {
        let n = self.order.len();
        let start = match after {
            Some(id) => self.order.iter().position(|&o| o == id)? + 1,
            None => 0,
        };
        (start..start + n).map(|i| (self.order[i % n], i >= n)).find(|&(id, _)| self.in_fight(id))
    }

    /// Sides that still have someone standing.
    pub fn standing_sides(&self) -> BTreeSet<&str> {
        self.creatures.iter().filter(|c| c.life == Life::Alive).map(|c| c.side.as_str()).collect()
    }

    /// Where a creature sits in turn order, for ordering its rules.
    pub fn order_position(&self, id: Id) -> u32 {
        self.order.iter().position(|&o| o == id).map_or(id.0, |p| p as u32)
    }

    /// The built-in effect of a committed event. This is the only place the
    /// state changes.
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Damage { target, .. } => {
                let dealt = event.damage_dealt();
                let c = self.creature_mut(*target);
                let absorbed = dealt.min(c.temp_hp);
                c.temp_hp -= absorbed;
                c.hp = c.hp.saturating_sub(dealt - absorbed);
            }
            Event::Heal { target, amount } => {
                let c = self.creature_mut(*target);
                c.hp = (c.hp + amount).min(c.block.hp);
                if c.life == Life::Down && c.hp > 0 {
                    c.life = Life::Alive;
                }
            }
            Event::TempHp { target, amount } => {
                let c = self.creature_mut(*target);
                c.temp_hp = c.temp_hp.max(*amount);
            }
            Event::ResourceSpent { who, resource, n } => {
                if let Some(pool) = self.creature_mut(*who).resources.get_mut(resource) {
                    *pool = pool.saturating_sub(*n);
                }
            }
            Event::ResourceRestored { who, resource, n } => {
                let max = self.max_resource(*who, resource);
                if let Some(pool) = self.creature_mut(*who).resources.get_mut(resource) {
                    *pool = pool.saturating_add(*n).min(max);
                }
            }
            Event::EffectAttached { target, source, effect } => {
                let expiry = self.expiry(effect.duration, *source);
                let id = self.next_effect;
                self.next_effect += 1;
                let c = self.creature_mut(*target);
                // The same effect twice does not stack; the new one replaces the old.
                c.effects.retain(|e| e.def.label() != effect.label());
                c.effects.push(EffectInstance { id, def: effect.clone(), source: *source, expiry });
            }
            Event::EffectRemoved { target, effect, .. } => {
                self.creature_mut(*target).effects.retain(|e| e.id != *effect);
            }
            Event::Moved { who, to } => self.creature_mut(*who).line = *to,
            Event::Engaged { a, b } => {
                self.engagements.insert(pair(*a, *b));
            }
            Event::Disengaged { a, b } => {
                self.engagements.remove(&pair(*a, *b));
            }
            Event::Dropped { who } => self.creature_mut(*who).life = Life::Down,
            Event::Died { who } => {
                let c = self.creature_mut(*who);
                c.life = Life::Dead;
                c.hp = 0;
            }
            Event::RoundStarted { round } => {
                self.round = *round;
                if *round == 1 {
                    let mut order: Vec<&Creature> = self.creatures.iter().collect();
                    order.sort_by_key(|c| (-c.initiative.unwrap_or(0), -(c.block.abilities.dex as i32), c.id));
                    self.order = order.into_iter().map(|c| c.id).collect();
                }
            }
            Event::TurnStarted { who } => {
                self.current = Some(*who);
                self.creature_mut(*who).turns_started += 1;
                // Per-turn pools refill here rather than through one
                // ResourceRestored event each, which would triple the log.
                for c in &mut self.creatures {
                    let own = c.id == *who;
                    let block = c.block.clone();
                    for (name, pool) in &mut c.resources {
                        let refills = match block.resources.get(name).map(|def| def.recharge) {
                            Some(Recharge::AnyTurn) => true,
                            Some(Recharge::OwnTurn) => own,
                            Some(_) => false,
                            None => own && [ACTION, BONUS_ACTION, REACTION].contains(&name.as_str()),
                        };
                        if refills {
                            *pool = block.max_resource(name).unwrap_or(*pool);
                        }
                    }
                }
            }
            Event::TurnEnded { .. } => self.current = None,
            Event::FightEnded { winner } => {
                self.ended = true;
                self.winner = winner.clone();
            }
            Event::Rolled { purpose: Purpose::Initiative { who }, value, .. } => {
                let c = self.creature_mut(*who);
                c.initiative = Some(*value as i32 + c.block.initiative_bonus());
            }
            Event::ActionUsed { .. }
            | Event::Attack { .. }
            | Event::Hit { .. }
            | Event::Miss { .. }
            | Event::Save { .. }
            | Event::SaveFailed { .. }
            | Event::SaveSucceeded { .. }
            | Event::DeathSave { .. }
            | Event::FightStarted
            | Event::Fizzled { .. }
            | Event::Rolled { .. }
            | Event::Chose { .. }
            | Event::Ac { .. } => {}
        }
    }

    fn expiry(&self, duration: Duration, source: Option<Id>) -> Option<Expiry> {
        let Duration::Until(until) = duration else { return None };
        let next = |id: Id| self.creature(id).turns_started + 1;
        match until {
            Until::StartOfNextTurn => source.map(|who| Expiry { who, at: Boundary::Start, turn: next(who) }),
            Until::EndOfNextTurn => source.map(|who| Expiry { who, at: Boundary::End, turn: next(who) }),
            Until::EndOfTurn => {
                self.current.map(|who| Expiry { who, at: Boundary::End, turn: self.creature(who).turns_started })
            }
        }
    }
}
