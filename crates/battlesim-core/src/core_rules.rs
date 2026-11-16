//! The core D&D rules, written in Rust behind the same [`Rule`] interface
//! the data-driven traits use.
//!
//! [`Core`] holds the rules that respond to an event directly: how an attack
//! becomes a roll and a roll becomes a hit, what damage does to a creature at
//! 0 hit points, what a chosen action costs. [`CoreLate`] holds the rules
//! that move the fight along once everyone else has responded: the next
//! turn, the next round, the end.

use crate::data::{Change, DEATH_FAILURES, DEATH_SUCCESSES, EffectDef, EventKind, Node, Recharge};
use crate::dice::{Dice, Mode, RollSpec};
use crate::event::{Choice, Constraint, Event, EventId, Expect, Outcome, Purpose};
use crate::rule::{Before, Entry, Proposal, Rule, Scope, Step, View};
use crate::state::{Boundary, Id, Life, State};
use std::sync::{Arc, LazyLock};

pub struct Core;
pub struct CoreLate;

/// What must hold for an event to take effect. It is checked when the event
/// reaches the top of the stack and again when it would commit. An event
/// that fails it fizzles.
pub fn requirement(event: &Event, state: &State) -> Result<(), &'static str> {
    if state.ended {
        return Err("the fight is over");
    }
    let present = |id: &Id, why| if state.in_fight(*id) { Ok(()) } else { Err(why) };
    let holds = |test: bool, why| if test { Ok(()) } else { Err(why) };
    const TARGET_GONE: &str = "its target is no longer in the fight";
    const GONE: &str = "the creature is no longer in the fight";
    match event {
        Event::Damage { target, .. }
        | Event::Heal { target, .. }
        | Event::TempHp { target, .. }
        | Event::EffectAttached { target, .. } => present(target, TARGET_GONE),
        Event::EffectRemoved { target, effect, .. } => {
            holds(state.creature(*target).effects.iter().any(|e| e.id == *effect), "the effect is already gone")
        }
        Event::ResourceSpent { who, resource, n } => {
            holds(state.creature(*who).resource(resource) >= *n, "the resource is already spent")
        }
        Event::ResourceRestored { who, .. }
        | Event::Moved { who, .. }
        | Event::Died { who }
        | Event::TurnStarted { who } => present(who, GONE),
        Event::Engaged { a, b } => {
            present(a, GONE)?;
            present(b, GONE)?;
            holds(state.exposed(*a) || state.exposed(*b), "the two cannot reach each other")
        }
        Event::Disengaged { a, b } => holds(state.engaged(*a, *b), "the two are not engaged"),
        Event::Dropped { who } => {
            let c = state.creature(*who);
            holds(c.life == Life::Alive && c.hp == 0, "the creature is not at 0 hit points")
        }
        Event::ActionUsed { actor, .. } => holds(state.can_act(*actor), "the actor can no longer act"),
        Event::Attack { attacker, target, .. } => {
            holds(state.can_act(*attacker), "the attacker can no longer act")?;
            present(target, TARGET_GONE)
        }
        Event::Hit(o) | Event::Miss(o) => present(&o.target, TARGET_GONE),
        Event::Save { who, .. } | Event::SaveFailed { who, .. } | Event::SaveSucceeded { who, .. } => {
            present(who, GONE)
        }
        Event::DeathSave { who } => {
            let c = state.creature(*who);
            holds(c.life == Life::Down && !c.is_stable(), "the creature is not dying")
        }
        Event::TurnEnded { who } => holds(state.current == Some(*who), "it is not that creature's turn"),
        Event::FightStarted
        | Event::RoundStarted { .. }
        | Event::FightEnded { .. }
        | Event::Fizzled { .. }
        | Event::Rolled { .. }
        | Event::Chose { .. }
        | Event::Ac { .. } => Ok(()),
    }
}

/// Whether a roll hits. A natural 1 misses and a natural 20 hits, whatever
/// the numbers say.
fn lands(natural: u32, total: i32, ac: i32) -> bool {
    natural != 1 && (natural == 20 || total >= ac)
}

/// Works out a pending attack outcome again against the target's armor class
/// as it is now. Returns the new event when the answer changed.
pub(crate) fn rederive(event: &Event, view: &View) -> Option<Event> {
    let (outcome, was_hit) = match event {
        Event::Hit(o) => (o, true),
        Event::Miss(o) => (o, false),
        _ => return None,
    };
    let ac = view.ac(outcome.target);
    if ac == outcome.ac {
        return None;
    }
    let mut next = Outcome { ac, ..outcome.clone() };
    Some(if lands(outcome.natural, outcome.total, ac) {
        if !was_hit {
            next.crit = outcome.natural == 20;
        }
        Event::Hit(next)
    } else {
        next.crit = false;
        Event::Miss(next)
    })
}

fn unconscious() -> Arc<EffectDef> {
    static EFFECT: LazyLock<Arc<EffectDef>> = LazyLock::new(|| Arc::new(EffectDef::condition("unconscious")));
    EFFECT.clone()
}

fn d20(mode: Mode, purpose: Purpose) -> Proposal {
    Proposal::Need(Expect::Roll { spec: RollSpec::D20(mode), purpose })
}

fn nodes(list: &[Node]) -> Vec<Step> {
    list.iter().cloned().map(Step::Node).collect()
}

/// Effects on anyone that end at this boundary of `who`'s turn.
fn expiring(state: &State, who: Id, at: Boundary) -> Vec<Proposal> {
    let turn = state.creature(who).turns_started;
    let mut out = Vec::new();
    for c in &state.creatures {
        for effect in &c.effects {
            if effect.expiry.is_some_and(|x| x.who == who && x.at == at && x.turn <= turn) {
                let name = effect.def.label().to_string();
                out.push(Proposal::Event(Event::EffectRemoved { target: c.id, effect: effect.id, name }));
            }
        }
    }
    out
}

impl Rule for Core {
    fn listens(&self, kind: EventKind) -> bool {
        use EventKind::*;
        matches!(
            kind,
            Attack | Damage | EffectAttached | Dropped // before
                | TurnStarted | TurnEnded | Chose | ActionUsed | Hit | Miss | Save | SaveFailed
                | SaveSucceeded | Rolled | Heal | ResourceSpent | DeathSave
        )
    }

    fn before(&self, event: &Event, view: &View) -> Before {
        let state = view.state;
        let changed = |change: Change| {
            let next = change.apply(event);
            if next == *event { Before::Pass } else { Before::Change(next) }
        };
        match event {
            // Shooting with an enemy in your face.
            Event::Attack { attacker, melee: false, .. }
                if state.engaged_with(*attacker).any(|o| state.is_enemy(*attacker, o) && state.can_act(o)) =>
            {
                changed(Change { disadvantage: Some(true), ..Change::default() })
            }
            Event::Damage { target, kind, .. } => {
                let defenses = &state.creature(*target).block.defenses;
                let has = |list: &[String]| list.contains(kind).then_some(true);
                changed(Change {
                    resist: has(&defenses.resist),
                    vulnerable: has(&defenses.vulnerable),
                    immune: has(&defenses.immune),
                    ..Change::default()
                })
            }
            Event::EffectAttached { target, effect, .. }
                if effect
                    .condition
                    .as_ref()
                    .is_some_and(|c| state.creature(*target).block.defenses.condition_immune.contains(c)) =>
            {
                Before::Cancel
            }
            // Most creatures die at 0 hit points instead of falling unconscious.
            Event::Dropped { who } if !state.creature(*who).block.death_saves => {
                Before::Replace(Event::Died { who: *who })
            }
            _ => Before::Pass,
        }
    }

    fn after(&self, entry: &Entry, view: &View) -> Vec<Proposal> {
        let state = view.state;
        let event = |e: Event| Proposal::Event(e);
        match &entry.event {
            Event::TurnStarted { who } => {
                let mut out = expiring(state, *who, Boundary::Start);
                let c = state.creature(*who);
                for (name, def) in &c.block.resources {
                    if let Recharge::Roll(min) = def.recharge
                        && c.resource(name) < def.max
                    {
                        let purpose = Purpose::Recharge { who: *who, resource: name.clone(), min };
                        out.push(Proposal::Need(Expect::Roll { spec: RollSpec::Sum(Dice::new(1, 6, 0)), purpose }));
                    }
                }
                out
            }
            Event::TurnEnded { who } => expiring(state, *who, Boundary::End),

            // A step on a turn. Declaring an action costs its action-economy
            // slot now. What the action itself spends is paid when it takes
            // effect, which is why a countered spell keeps its slot.
            Event::Chose { actor, choice: Choice::Act { action, targets }, .. } => {
                let Some(def) = state.creature(*actor).block.action(action) else { return Vec::new() };
                let mut steps = Vec::new();
                if let Some(pool) = def.cost.pool() {
                    steps.push(Step::Spend { who: *actor, resource: pool.to_string(), n: 1 });
                }
                steps.push(Step::Event(Event::ActionUsed {
                    actor: *actor,
                    action: action.clone(),
                    targets: targets.clone(),
                }));
                let again = Expect::Choice { actor: *actor, constraint: Constraint::Step };
                vec![Proposal::Steps(steps, Scope::of(*actor)), Proposal::Need(again)]
            }
            Event::Chose { actor, choice: Choice::EndTurn, .. } => vec![event(Event::TurnEnded { who: *actor })],

            Event::ActionUsed { actor, action, targets } => {
                let Some(def) = state.creature(*actor).block.action(action) else { return Vec::new() };
                // An action used in response to something keeps that
                // something in scope, so its effect can cancel it.
                let trigger = entry.scope.as_ref().and_then(|s| s.trigger.clone());
                let scope = |target: Option<Id>| Scope {
                    target,
                    action: Some(action.clone()),
                    trigger: trigger.clone(),
                    ..Scope::of(*actor)
                };
                let mut steps: Vec<Step> = def
                    .uses
                    .iter()
                    .map(|(resource, n)| Step::Spend { who: *actor, resource: resource.clone(), n: *n })
                    .collect();
                if targets.is_empty() {
                    steps.push(Step::Scoped(nodes(&def.effect), scope(None)));
                }
                steps.extend(targets.iter().map(|t| Step::Scoped(nodes(&def.effect), scope(Some(*t)))));
                vec![Proposal::Steps(steps, scope(None))]
            }

            Event::Attack { attacker, target, melee, advantage, disadvantage, .. } => {
                let mut out = Vec::new();
                if *melee && !state.engaged(*attacker, *target) {
                    out.push(event(Event::Engaged { a: *attacker, b: *target }));
                }
                out.push(d20(Mode::from_flags(*advantage, *disadvantage), Purpose::Attack { attack: entry.id }));
                out
            }
            Event::Hit(o) => outcome_effect(o, true, view),
            Event::Miss(o) => outcome_effect(o, false, view),

            Event::Save { who, dc, advantage, disadvantage, auto_fail, .. } => {
                if *auto_fail {
                    return vec![event(Event::SaveFailed { save: entry.id, who: *who, natural: 0, total: 0, dc: *dc })];
                }
                vec![d20(Mode::from_flags(*advantage, *disadvantage), Purpose::Save { save: entry.id })]
            }
            Event::SaveFailed { save, who, .. } => save_effect(*save, *who, true, view),
            Event::SaveSucceeded { save, who, .. } => save_effect(*save, *who, false, view),

            Event::Rolled { purpose, value, .. } => rolled(purpose, *value, view),

            Event::Damage { target, crit, .. } => {
                let c = state.creature(*target);
                match c.life {
                    Life::Alive if c.hp == 0 => vec![event(Event::Dropped { who: *target })],
                    // Damage at 0 hit points is a failed death save, two on a
                    // critical hit. A stable creature starts dying again.
                    Life::Down if entry.event.damage_dealt() > 0 => {
                        let mut out = Vec::new();
                        let mut left = c.resource(DEATH_FAILURES);
                        if c.is_stable() {
                            for pool in [DEATH_SUCCESSES, DEATH_FAILURES] {
                                out.push(event(Event::ResourceRestored { who: *target, resource: pool.into(), n: 3 }));
                            }
                            left = 3;
                        }
                        let n = if *crit { 2 } else { 1 }.min(left);
                        if n > 0 {
                            out.push(event(Event::ResourceSpent { who: *target, resource: DEATH_FAILURES.into(), n }));
                        }
                        out
                    }
                    _ => Vec::new(),
                }
            }
            Event::Dropped { who } => {
                vec![event(Event::EffectAttached { target: *who, source: None, effect: unconscious() })]
            }
            // Healing a creature at 0 hit points wakes it and clears its
            // death saves.
            Event::Heal { target, .. } => {
                let c = state.creature(*target);
                if c.life != Life::Alive || !c.block.death_saves {
                    return Vec::new();
                }
                let mut out = Vec::new();
                let asleep =
                    c.effects.iter().find(|e| e.source.is_none() && e.def.condition.as_deref() == Some("unconscious"));
                if let Some(effect) = asleep {
                    let name = effect.def.label().to_string();
                    out.push(event(Event::EffectRemoved { target: *target, effect: effect.id, name }));
                }
                for pool in [DEATH_SUCCESSES, DEATH_FAILURES] {
                    if c.resource(pool) < 3 {
                        out.push(event(Event::ResourceRestored { who: *target, resource: pool.into(), n: 3 }));
                    }
                }
                out
            }
            Event::ResourceSpent { who, resource, .. }
                if resource == DEATH_FAILURES && state.creature(*who).resource(DEATH_FAILURES) == 0 =>
            {
                vec![event(Event::Died { who: *who })]
            }
            Event::DeathSave { who } => vec![d20(Mode::Normal, Purpose::DeathSave { who: *who })],
            _ => Vec::new(),
        }
    }
}

/// What a roll leads to, read from the purpose recorded when it was asked for.
fn rolled(purpose: &Purpose, value: u32, view: &View) -> Vec<Proposal> {
    let state = view.state;
    let event = |e: Event| vec![Proposal::Event(e)];
    match purpose {
        Purpose::Initiative { .. } => Vec::new(),
        Purpose::Attack { attack } => {
            let Event::Attack { attacker, target, action, bonus, advantage, disadvantage, melee, .. } =
                &view.log[*attack].event
            else {
                return Vec::new();
            };
            let outcome = Outcome {
                attack: *attack,
                attacker: *attacker,
                target: *target,
                action: action.clone(),
                natural: value,
                total: value as i32 + bonus,
                ac: view.ac(*target),
                crit: value == 20,
                advantage: *advantage && !*disadvantage,
                disadvantage: *disadvantage && !*advantage,
                melee: *melee,
            };
            event(if lands(value, outcome.total, outcome.ac) { Event::Hit(outcome) } else { Event::Miss(outcome) })
        }
        Purpose::Save { save } => {
            let Event::Save { who, dc, bonus, .. } = &view.log[*save].event else { return Vec::new() };
            let (save, who, natural, total, dc) = (*save, *who, value, value as i32 + bonus, *dc);
            event(if total >= dc as i32 {
                Event::SaveSucceeded { save, who, natural, total, dc }
            } else {
                Event::SaveFailed { save, who, natural, total, dc }
            })
        }
        Purpose::DeathSave { who } => {
            let c = state.creature(*who);
            let spend = |pool: &str, n: u32| {
                let n = n.min(c.resource(pool));
                if n == 0 {
                    return Vec::new();
                }
                event(Event::ResourceSpent { who: *who, resource: pool.to_string(), n })
            };
            match value {
                20 => event(Event::Heal { target: *who, amount: 1 }),
                1 => spend(DEATH_FAILURES, 2),
                10.. => spend(DEATH_SUCCESSES, 1),
                _ => spend(DEATH_FAILURES, 1),
            }
        }
        Purpose::Recharge { who, resource, min } => {
            if value < *min {
                return Vec::new();
            }
            let n = state.max_resource(*who, resource);
            event(Event::ResourceRestored { who: *who, resource: resource.clone(), n })
        }
        Purpose::Damage { .. } | Purpose::Heal { .. } | Purpose::TempHp { .. } => {
            purpose.amount_event(value).map(Proposal::Event).into_iter().collect()
        }
    }
}

/// Runs an attack's `on_hit` or `on_miss` in the scope the attack was made in.
fn outcome_effect(outcome: &Outcome, hit: bool, view: &View) -> Vec<Proposal> {
    let entry = &view.log[outcome.attack];
    let (Event::Attack { spec, .. }, Some(scope)) = (&entry.event, &entry.scope) else { return Vec::new() };
    let list = if hit { &spec.on_hit } else { &spec.on_miss };
    if list.is_empty() {
        return Vec::new();
    }
    let block = &view.state.creature(outcome.attacker).block;
    let inherit = block.action(&outcome.action).and_then(|a| a.damage_type()).map(str::to_string);
    let scope = Scope { crit: hit && outcome.crit, inherit, ..(**scope).clone() };
    vec![Proposal::Steps(nodes(list), scope)]
}

/// Runs a save's `on_fail` or `on_success` with the saver as the target.
fn save_effect(save: EventId, who: Id, failed: bool, view: &View) -> Vec<Proposal> {
    let entry = &view.log[save];
    let (Event::Save { spec, .. }, Some(scope)) = (&entry.event, &entry.scope) else { return Vec::new() };
    let list = if failed { &spec.on_fail } else { &spec.on_success };
    if list.is_empty() {
        return Vec::new();
    }
    vec![Proposal::Steps(nodes(list), Scope { target: Some(who), ..(**scope).clone() })]
}

impl Rule for CoreLate {
    fn listens(&self, kind: EventKind) -> bool {
        use EventKind::*;
        matches!(kind, FightStarted | RoundStarted | TurnStarted | TurnEnded | Died | Dropped)
    }

    fn after(&self, entry: &Entry, view: &View) -> Vec<Proposal> {
        let state = view.state;
        let event = |e: Event| vec![Proposal::Event(e)];
        let draw = || event(Event::FightEnded { winner: None });
        match &entry.event {
            Event::FightStarted => {
                let mut out: Vec<Proposal> =
                    state.creatures.iter().map(|c| d20(Mode::Normal, Purpose::Initiative { who: c.id })).collect();
                out.push(Proposal::Event(Event::RoundStarted { round: 1 }));
                out
            }
            Event::RoundStarted { round } => match state.next_turn(None) {
                Some((who, _)) if *round <= view.config.max_rounds => event(Event::TurnStarted { who }),
                _ => draw(),
            },
            Event::TurnStarted { who } => {
                let c = state.creature(*who);
                let mut out = Vec::new();
                if c.life == Life::Down && !c.is_stable() {
                    out.push(Proposal::Event(Event::DeathSave { who: *who }));
                }
                // The engine turns this into the end of the turn for a
                // creature that cannot act.
                out.push(Proposal::Need(Expect::Choice { actor: *who, constraint: Constraint::Step }));
                out
            }
            Event::TurnEnded { who } => match state.next_turn(Some(*who)) {
                Some((_, true)) => event(Event::RoundStarted { round: state.round + 1 }),
                Some((next, false)) => event(Event::TurnStarted { who: next }),
                None => draw(),
            },
            // The fight ends when at most one side has anyone standing. A
            // side whose members are all down has lost, even if some of
            // them are still making death saves.
            Event::Died { .. } | Event::Dropped { .. } => {
                let standing = state.standing_sides();
                if standing.len() > 1 {
                    return Vec::new();
                }
                event(Event::FightEnded { winner: standing.first().map(|side| side.to_string()) })
            }
            _ => Vec::new(),
        }
    }
}
